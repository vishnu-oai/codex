use super::*;

#[derive(Clone, Copy)]
pub(super) enum SetupInstallPolicy {
    Reject,
    AllowForegroundFirstInstall,
}

pub(crate) struct ForegroundPluginInstallRequest {
    pub source_path: AbsolutePathBuf,
    pub plugin_id: PluginId,
    pub plugin_version: Option<String>,
    pub fallback_manifest_contents: Option<String>,
}

impl PluginStore {
    /// Installs a package selected by an explicit foreground `plugin add` without activating it.
    ///
    /// The caller must verify that no configured plugin entry exists for this plugin. This method
    /// may replace an inert cache left by an earlier declined or failed setup attempt.
    #[cfg(test)]
    pub(crate) fn install_from_foreground(
        &self,
        request: ForegroundPluginInstallRequest,
    ) -> Result<PluginInstallResult, PluginStoreError> {
        let install_lock = self.acquire_mutation_lock(&request.plugin_id)?;
        self.install_from_foreground_with_lock(request, &install_lock)
    }

    pub(crate) fn install_from_foreground_with_lock(
        &self,
        request: ForegroundPluginInstallRequest,
        install_lock: &ForegroundPluginInstallLock,
    ) -> Result<PluginInstallResult, PluginStoreError> {
        let ForegroundPluginInstallRequest {
            source_path,
            plugin_id,
            plugin_version,
            fallback_manifest_contents,
        } = request;
        let manifest = fallback_manifest_contents
            .as_deref()
            .map(InstallManifest::Fallback)
            .unwrap_or(InstallManifest::OnDisk);
        let manifest = resolve_install_manifest(source_path.as_path(), manifest);
        let plugin_version = match plugin_version {
            Some(plugin_version) => plugin_version,
            None => plugin_version_for_install_manifest(source_path.as_path(), manifest)?,
        };
        validate_plugin_version_segment(&plugin_version).map_err(PluginStoreError::Invalid)?;
        self.install_with_version_and_manifest_locked(
            source_path,
            plugin_id,
            plugin_version,
            manifest,
            SetupInstallPolicy::AllowForegroundFirstInstall,
            install_lock,
        )
    }

    pub(super) fn validate_setup_install(
        &self,
        plugin_id: &PluginId,
        candidate_manifest: &PluginManifest,
        setup_policy: SetupInstallPolicy,
    ) -> Result<(), PluginStoreError> {
        let plugin_key = plugin_id.as_key();
        let existing_root = self.active_plugin_root(plugin_id);
        let existing_declares_setup = existing_root
            .as_ref()
            .and_then(|root| load_plugin_manifest(root.as_path()))
            .is_some_and(|manifest| manifest.setup.is_some());
        if matches!(
            setup_policy,
            SetupInstallPolicy::AllowForegroundFirstInstall
        ) {
            if candidate_manifest.setup.is_some()
                && existing_root.is_some()
                && !existing_declares_setup
            {
                return Err(PluginStoreError::SetupCommandUpdateUnsupported { plugin_key });
            }
            return Ok(());
        }

        if existing_root.is_some()
            && (existing_declares_setup || candidate_manifest.setup.is_some())
        {
            return Err(PluginStoreError::SetupCommandUpdateUnsupported { plugin_key });
        }
        if candidate_manifest.setup.is_some() {
            return Err(PluginStoreError::SetupCommandRequiresForeground { plugin_key });
        }
        Ok(())
    }

    pub(super) fn validate_staged_install(
        &self,
        plugin_id: &PluginId,
        expected_manifest: &PluginManifest,
        staged_root: &Path,
        setup_policy: SetupInstallPolicy,
    ) -> Result<(), PluginStoreError> {
        let staged_manifest = plugin_manifest_for_source(staged_root, InstallManifest::OnDisk)?;
        validate_plugin_manifest_name(plugin_id, &staged_manifest)?;
        if staged_manifest.version != expected_manifest.version {
            let expected_version = expected_manifest.version.as_deref().unwrap_or("<none>");
            let staged_version = staged_manifest.version.as_deref().unwrap_or("<none>");
            return Err(PluginStoreError::Invalid(format!(
                "plugin.json version changed while staging plugin `{}`: expected `{expected_version}`, found `{staged_version}`",
                plugin_id.as_key()
            )));
        }
        if staged_manifest.setup != expected_manifest.setup {
            return Err(PluginStoreError::Invalid(format!(
                "plugin setup command changed while staging plugin `{}`",
                plugin_id.as_key()
            )));
        }
        self.validate_setup_install(plugin_id, &staged_manifest, setup_policy)
    }
}
