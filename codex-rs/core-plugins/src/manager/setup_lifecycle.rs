use super::*;
use crate::setup_install::ForegroundPluginInstallLock;
use crate::setup_install::plugin_content_fingerprint;
use crate::setup_install::write_setup_completion_marker;
use crate::store::ForegroundPluginInstallRequest;
use crate::store::plugin_version_for_source;
use crate::store::plugin_version_for_source_with_fallback_manifest;
use codex_config::CONFIG_TOML_FILE;

/// Result of an explicit foreground plugin installation.
#[derive(Debug)]
pub enum ForegroundPluginInstallOutcome {
    Installed(PluginInstallOutcome),
    SetupRequired(PendingPluginSetup),
}

/// Inert installed package whose setup command must succeed before activation.
#[derive(Debug)]
pub struct PendingPluginSetup {
    pub plugin_id: PluginId,
    pub plugin_version: String,
    pub installed_path: AbsolutePathBuf,
    pub data_path: AbsolutePathBuf,
    pub setup: codex_plugin::manifest::PluginManifestSetup,
    pub auth_policy: MarketplacePluginAuthPolicy,
    cache_version: String,
    content_fingerprint: String,
    install_lock: ForegroundPluginInstallLock,
}

impl PluginsManager {
    /// Installs from an explicit CLI request, returning setup work instead of activating it.
    pub async fn install_plugin_from_foreground_cli(
        &self,
        config_layer_stack: &ConfigLayerStack,
        request: PluginInstallRequest,
    ) -> Result<ForegroundPluginInstallOutcome, PluginInstallError> {
        let resolved = self.resolve_installable_plugin(config_layer_stack, &request)?;
        let plugin_id = resolved.plugin_id.clone();
        let codex_home = self.codex_home.clone();
        let plugin_id_for_lock = plugin_id.clone();
        let install_lock = tokio::task::spawn_blocking(move || {
            ForegroundPluginInstallLock::acquire(&codex_home, &plugin_id_for_lock)
        })
        .await
        .map_err(PluginInstallError::join)??;
        let configured_plugin_from_stack =
            configured_plugins_from_stack(config_layer_stack, self.codex_home.as_path())
                .get(&plugin_id.as_key())
                .cloned();
        let configured_plugin = configured_plugin_from_stack.or_else(|| {
            user_plugin_config_from_disk(self.codex_home.as_path(), &plugin_id.as_key())
        });
        let cached = self.store.is_installed(&plugin_id);
        let configured_state = configured_plugin
            .as_ref()
            .map(|config| (config.enabled, cached));
        let result = if configured_plugin.is_some() {
            // Normal Store installs acquire this same lock internally. Release the foreground
            // guard before entering that path to avoid recursively locking the same file.
            drop(install_lock);
            self.install_resolved_plugin(resolved)
                .await
                .map(ForegroundPluginInstallOutcome::Installed)
        } else {
            self.install_resolved_plugin_from_foreground(resolved, install_lock)
                .await
        };
        let result = match (result, configured_state) {
            (
                Err(PluginInstallError::Store(
                    PluginStoreError::SetupCommandRequiresForeground { .. }
                    | PluginStoreError::SetupCommandUpdateUnsupported { .. },
                )),
                Some((enabled, cached)),
            ) if !enabled || !cached => Err(PluginInstallError::ConfiguredSetupReinstallRequired {
                plugin_name: plugin_id.plugin_name.clone(),
                marketplace_name: plugin_id.marketplace_name.clone(),
                state: if cached {
                    "disabled"
                } else {
                    "missing its cache"
                },
            }),
            (result, _) => result,
        };
        match result {
            Ok(outcome) => Ok(outcome),
            Err(err) => {
                self.track_plugin_install_failed(
                    &plugin_id,
                    plugin_install_error_type(&err),
                    err.sub_error_type(),
                    err.to_string(),
                );
                Err(err)
            }
        }
    }

    /// Verifies that a pending setup still refers to the exact materialized package.
    pub async fn validate_pending_plugin_setup(
        &self,
        pending: &PendingPluginSetup,
    ) -> Result<(), PluginInstallError> {
        let store = self.store.clone();
        let plugin_id_for_validation = pending.plugin_id.clone();
        let cache_version_for_validation = pending.cache_version.clone();
        let installed_path_for_validation = pending.installed_path.clone();
        let setup_for_validation = pending.setup.clone();
        let content_fingerprint = pending.content_fingerprint.clone();
        tokio::task::spawn_blocking(move || {
            let active_root = store
                .active_plugin_root(&plugin_id_for_validation)
                .ok_or_else(|| {
                    PluginStoreError::Invalid(format!(
                        "pending setup plugin `{}` is no longer installed",
                        plugin_id_for_validation.as_key()
                    ))
                })?;
            if active_root != installed_path_for_validation
                || store
                    .active_plugin_version(&plugin_id_for_validation)
                    .as_deref()
                    != Some(cache_version_for_validation.as_str())
            {
                return Err(PluginStoreError::Invalid(format!(
                    "pending setup plugin `{}` changed before activation",
                    plugin_id_for_validation.as_key()
                )));
            }
            let setup = load_plugin_manifest(active_root.as_path())
                .and_then(|manifest| manifest.setup);
            if setup.as_ref() != Some(&setup_for_validation) {
                return Err(PluginStoreError::Invalid(format!(
                    "pending setup command for plugin `{}` changed before activation",
                    plugin_id_for_validation.as_key()
                )));
            }
            if plugin_content_fingerprint(active_root.as_path())? != content_fingerprint {
                return Err(PluginStoreError::Invalid(format!(
                    "pending setup package for plugin `{}` changed before activation; setup must leave PLUGIN_ROOT unchanged",
                    plugin_id_for_validation.as_key()
                )));
            }
            Ok(())
        })
        .await
        .map_err(PluginInstallError::join)??;
        Ok(())
    }

    /// Activates an unchanged pending package after its setup command succeeds.
    pub async fn activate_plugin_after_setup(
        &self,
        pending: PendingPluginSetup,
    ) -> Result<PluginInstallOutcome, PluginInstallError> {
        self.validate_pending_plugin_setup(&pending).await?;
        let installed_path_for_marker = pending.installed_path.clone();
        let content_fingerprint_for_marker = pending.content_fingerprint.clone();
        tokio::task::spawn_blocking(move || {
            write_setup_completion_marker(
                installed_path_for_marker.as_path(),
                &content_fingerprint_for_marker,
            )
        })
        .await
        .map_err(PluginInstallError::join)??;
        let PendingPluginSetup {
            plugin_id,
            plugin_version,
            installed_path,
            data_path: _,
            setup: _,
            auth_policy,
            cache_version,
            content_fingerprint: _,
            install_lock,
        } = pending;

        let mut outcome = self
            .activate_store_install(
                StorePluginInstallResult {
                    plugin_id,
                    plugin_version: cache_version,
                    installed_path,
                },
                auth_policy,
            )
            .await?;
        outcome.plugin_version = plugin_version;
        drop(install_lock);
        Ok(outcome)
    }

    async fn install_resolved_plugin_from_foreground(
        &self,
        resolved: ResolvedMarketplacePlugin,
        install_lock: ForegroundPluginInstallLock,
    ) -> Result<ForegroundPluginInstallOutcome, PluginInstallError> {
        let auth_policy = resolved.policy.authentication;
        let plugin_version =
            if is_openai_curated_marketplace_name(&resolved.plugin_id.marketplace_name) {
                let curated_plugin_version = read_curated_plugins_sha(self.codex_home.as_path())
                    .ok_or_else(|| {
                        PluginStoreError::Invalid(
                            "local curated marketplace sha is not available".to_string(),
                        )
                    })?;
                Some(curated_plugin_cache_version(&curated_plugin_version))
            } else {
                None
            };
        let store = self.store.clone();
        let codex_home = self.codex_home.clone();
        let fallback_manifest_contents = resolved
            .manifest_fallback
            .contents_if_has_metadata()
            .map(str::to_string);
        let (result, display_version, setup, content_fingerprint, install_lock) =
            tokio::task::spawn_blocking(move || {
                let materialized =
                    materialize_marketplace_plugin_source(codex_home.as_path(), &resolved.source)
                        .map_err(PluginStoreError::Invalid)?;
                let display_version = match plugin_version.as_ref() {
                    Some(plugin_version) => plugin_version.clone(),
                    None => match fallback_manifest_contents.as_deref() {
                        Some(contents) => plugin_version_for_source_with_fallback_manifest(
                            materialized.path.as_path(),
                            contents,
                        )?,
                        None => plugin_version_for_source(materialized.path.as_path())?,
                    },
                };
                let result = store.install_from_foreground_with_lock(
                    ForegroundPluginInstallRequest {
                        source_path: materialized.path,
                        plugin_id: resolved.plugin_id,
                        plugin_version,
                        fallback_manifest_contents,
                    },
                    &install_lock,
                )?;
                let setup = load_plugin_manifest(result.installed_path.as_path())
                    .and_then(|manifest| manifest.setup);
                let content_fingerprint = setup
                    .as_ref()
                    .map(|_| plugin_content_fingerprint(result.installed_path.as_path()))
                    .transpose()?;
                Ok::<_, PluginStoreError>((
                    result,
                    display_version,
                    setup,
                    content_fingerprint,
                    install_lock,
                ))
            })
            .await
            .map_err(PluginInstallError::join)??;

        let Some(setup) = setup else {
            let outcome = self.activate_store_install(result, auth_policy).await?;
            drop(install_lock);
            return Ok(ForegroundPluginInstallOutcome::Installed(outcome));
        };
        let content_fingerprint = content_fingerprint.ok_or_else(|| {
            PluginStoreError::Invalid("setup package fingerprint is missing".to_string())
        })?;
        let data_path = self.store.plugin_setup_data_root(&result.plugin_id);
        Ok(ForegroundPluginInstallOutcome::SetupRequired(
            PendingPluginSetup {
                plugin_id: result.plugin_id,
                plugin_version: display_version,
                installed_path: result.installed_path,
                data_path,
                setup,
                auth_policy,
                cache_version: result.plugin_version,
                content_fingerprint,
                install_lock,
            },
        ))
    }
}

fn user_plugin_config_from_disk(codex_home: &Path, plugin_key: &str) -> Option<PluginConfig> {
    // Another CLI process can activate this plugin while this process waits on the setup install
    // lock. Consult the base user file after acquiring the lock so a stale layer stack cannot turn
    // that activation into a second first-install attempt.
    let contents = std::fs::read_to_string(codex_home.join(CONFIG_TOML_FILE)).ok()?;
    let config = toml::from_str::<toml::Value>(&contents).ok()?;
    config
        .get("plugins")?
        .get(plugin_key)?
        .clone()
        .try_into()
        .ok()
}

/// Setup plan for an already activated plugin, held under its mutation lock.
#[derive(Debug)]
pub struct InstalledPluginSetup {
    pub plugin_id: PluginId,
    pub installed_path: AbsolutePathBuf,
    pub data_path: AbsolutePathBuf,
    pub setup: codex_plugin::manifest::PluginManifestSetup,
    content_fingerprint: String,
    install_lock: ForegroundPluginInstallLock,
}

impl PluginsManager {
    /// Resolves a completed plugin setup without exposing an unlocked package.
    pub async fn prepare_installed_plugin_setup(
        &self,
        config_layer_stack: &ConfigLayerStack,
        plugin_id: PluginId,
    ) -> Result<InstalledPluginSetup, PluginInstallError> {
        let configured =
            configured_plugins_from_stack(config_layer_stack, self.codex_home.as_path());
        if !configured
            .get(&plugin_id.as_key())
            .is_some_and(|plugin| plugin.enabled)
        {
            return Err(PluginStoreError::Invalid(format!(
                "plugin `{}` is not installed and enabled",
                plugin_id.as_key()
            ))
            .into());
        }
        let codex_home = self.codex_home.clone();
        let plugin_id_for_lock = plugin_id.clone();
        let install_lock = tokio::task::spawn_blocking(move || {
            ForegroundPluginInstallLock::acquire(&codex_home, &plugin_id_for_lock)
        })
        .await
        .map_err(PluginInstallError::join)??;
        let installed_path = self.store.active_plugin_root(&plugin_id).ok_or_else(|| {
            PluginStoreError::Invalid(format!("plugin `{}` is not installed", plugin_id.as_key()))
        })?;
        let setup = load_plugin_manifest(installed_path.as_path())
            .and_then(|manifest| manifest.setup)
            .ok_or_else(|| {
                PluginStoreError::Invalid(format!(
                    "plugin `{}` does not declare setup commands",
                    plugin_id.as_key()
                ))
            })?;
        crate::setup_install::validate_setup_completion_marker(installed_path.as_path())?;
        let content_fingerprint = plugin_content_fingerprint(installed_path.as_path())?;
        Ok(InstalledPluginSetup {
            data_path: self.store.plugin_setup_data_root(&plugin_id),
            plugin_id,
            installed_path,
            setup,
            content_fingerprint,
            install_lock,
        })
    }

    /// Verifies that rerun setup did not replace or mutate its approved package.
    pub async fn finish_installed_plugin_setup(
        &self,
        pending: InstalledPluginSetup,
    ) -> Result<(), PluginInstallError> {
        let InstalledPluginSetup {
            plugin_id,
            installed_path,
            data_path: _,
            setup,
            content_fingerprint,
            install_lock,
        } = pending;
        let store = self.store.clone();
        tokio::task::spawn_blocking(move || {
            install_lock.validate_plugin_id(&plugin_id)?;
            if store.active_plugin_root(&plugin_id).as_ref() != Some(&installed_path) {
                return Err(PluginStoreError::Invalid(format!(
                    "plugin `{}` changed while setup was running",
                    plugin_id.as_key()
                )));
            }
            let current_setup =
                load_plugin_manifest(installed_path.as_path()).and_then(|manifest| manifest.setup);
            if current_setup.as_ref() != Some(&setup) {
                return Err(PluginStoreError::Invalid(format!(
                    "plugin `{}` setup changed while it was running",
                    plugin_id.as_key()
                )));
            }
            if plugin_content_fingerprint(installed_path.as_path())? != content_fingerprint {
                return Err(PluginStoreError::Invalid(format!(
                    "plugin `{}` setup modified PLUGIN_ROOT; write generated files to PLUGIN_DATA",
                    plugin_id.as_key()
                )));
            }
            write_setup_completion_marker(installed_path.as_path(), &content_fingerprint)
        })
        .await
        .map_err(PluginInstallError::join)??;
        Ok(())
    }
}
