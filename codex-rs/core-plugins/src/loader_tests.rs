use super::*;
use crate::manifest::load_plugin_manifest;
use crate::setup_install::plugin_content_fingerprint;
use crate::setup_install::write_setup_completion_marker;
use crate::store::ForegroundPluginInstallRequest;
use crate::test_support::write_file;
use codex_config::ConfigLayerEntry;
use codex_config::ConfigLayerSource;
use codex_config::ConfigRequirements;
use codex_config::ConfigRequirementsToml;
use codex_core_skills::loader::MAX_CONCURRENT_ROOT_SCANS;
use codex_plugin::PluginId;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

fn user_config_path(temp_dir: &TempDir, file_name: &str) -> AbsolutePathBuf {
    AbsolutePathBuf::from_absolute_path(temp_dir.path().join(file_name))
        .expect("test user config path should be absolute")
}

fn user_layer(path: AbsolutePathBuf, config: &str) -> ConfigLayerEntry {
    ConfigLayerEntry::new(
        ConfigLayerSource::User {
            file: path,
            profile: None,
        },
        toml::from_str(config).expect("user config toml"),
    )
}

#[tokio::test]
async fn configured_setup_plugin_without_completion_marker_loads_no_capabilities() {
    let temp_dir = TempDir::new().expect("tempdir");
    let plugin_root = temp_dir
        .path()
        .join("plugins/cache/test/pending-setup/1.0.0");
    write_file(
        &plugin_root.join(".codex-plugin/plugin.json"),
        r#"{
  "name": "pending-setup",
  "setup": { "command": ["node", "./setup.js"] },
  "skills": ["./skills"],
  "mcpServers": "./.mcp.json",
  "apps": "./.app.json",
  "hooks": "./hooks.json"
}"#,
    );
    write_file(
        &plugin_root.join("skills/example/SKILL.md"),
        "---\nname: example\ndescription: example skill\n---\n",
    );
    write_file(
        &plugin_root.join(".mcp.json"),
        r#"{"mcpServers":{"example":{"command":"echo"}}}"#,
    );
    write_file(
        &plugin_root.join(".app.json"),
        r#"{"apps":{"example":{"id":"connector_example"}}}"#,
    );
    write_file(
        &plugin_root.join("hooks.json"),
        r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"echo setup"}]}]}}"#,
    );
    let stack = ConfigLayerStack::new(
        vec![user_layer(
            user_config_path(&temp_dir, "config.toml"),
            "[plugins.\"pending-setup@test\"]\nenabled = true\n",
        )],
        ConfigRequirements::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("valid config layer stack");

    let plugins = load_plugins_from_layer_stack(
        &stack,
        RemoteInstalledPluginsSnapshot::default(),
        &PluginStore::new(temp_dir.path().to_path_buf()),
        /*plugin_skill_snapshots*/ None,
        Some(Product::Codex),
        /*remote_global_catalog_active*/ false,
        Arc::new(Semaphore::new(MAX_CONCURRENT_ROOT_SCANS)),
    )
    .await;

    let plugin = plugins
        .iter()
        .find(|plugin| plugin.config_name == "pending-setup@test")
        .expect("configured plugin");
    assert!(!plugin.is_active());
    assert!(
        plugin
            .error
            .as_deref()
            .is_some_and(|error| error.contains("setup completion marker is missing"))
    );
    assert!(plugin.skill_roots.is_empty());
    assert!(plugin.mcp_servers.is_empty());
    assert!(plugin.apps.is_empty());
    assert!(plugin.hook_sources.is_empty());
}

#[tokio::test]
async fn setup_load_fails_closed_while_cache_mutation_is_in_progress() {
    let temp_dir = TempDir::new().expect("tempdir");
    let store = PluginStore::new(temp_dir.path().to_path_buf());
    let plugin_id = PluginId::new("sample".to_string(), "test".to_string()).unwrap();
    let completed_source = temp_dir.path().join("completed-source");
    write_file(
        &completed_source.join(".codex-plugin/plugin.json"),
        r#"{
  "name": "sample",
  "version": "1.0.0",
  "setup": { "command": ["node", "./setup.js"] },
  "skills": ["./skills"]
}"#,
    );
    write_file(
        &completed_source.join("skills/completed/SKILL.md"),
        "---\nname: completed\ndescription: completed\n---\n",
    );
    let completed = store
        .install_from_foreground(ForegroundPluginInstallRequest {
            source_path: AbsolutePathBuf::try_from(completed_source).unwrap(),
            plugin_id: plugin_id.clone(),
            plugin_version: None,
            fallback_manifest_contents: None,
        })
        .unwrap();
    let fingerprint = plugin_content_fingerprint(completed.installed_path.as_path()).unwrap();
    write_setup_completion_marker(completed.installed_path.as_path(), &fingerprint).unwrap();

    let stack = ConfigLayerStack::new(
        vec![user_layer(
            user_config_path(&temp_dir, "config.toml"),
            "[plugins.\"sample@test\"]\nenabled = true\n",
        )],
        ConfigRequirements::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("valid config layer stack");
    let install_lock = store.acquire_mutation_lock(&plugin_id).unwrap();
    let store_for_load = store.clone();
    let load = tokio::spawn(async move {
        load_plugins_from_layer_stack(
            &stack,
            RemoteInstalledPluginsSnapshot::default(),
            &store_for_load,
            /*plugin_skill_snapshots*/ None,
            Some(Product::Codex),
            /*remote_global_catalog_active*/ false,
            Arc::new(Semaphore::new(MAX_CONCURRENT_ROOT_SCANS)),
        )
        .await
    });
    let plugins = tokio::time::timeout(std::time::Duration::from_secs(/*secs*/ 1), load)
        .await
        .expect("plugin loading must not wait for a cache writer")
        .unwrap();
    let plugin = plugins
        .iter()
        .find(|plugin| plugin.config_name == "sample@test")
        .expect("configured plugin");
    assert!(!plugin.is_active());
    assert!(
        plugin
            .error
            .as_deref()
            .is_some_and(|error| error.contains("plugin cache mutation is in progress"))
    );
    assert!(plugin.skill_roots.is_empty());
    drop(install_lock);
}

#[test]
fn configured_plugins_from_stack_merges_user_layers() {
    let temp_dir = TempDir::new().expect("tempdir");
    let stack = ConfigLayerStack::new(
        vec![
            user_layer(
                user_config_path(&temp_dir, "config.toml"),
                "[plugins.base]\nenabled = true\n",
            ),
            user_layer(
                user_config_path(&temp_dir, "work.config.toml"),
                "[plugins.profile]\nenabled = false\n",
            ),
        ],
        ConfigRequirements::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("valid config layer stack");

    let plugins = configured_plugins_from_stack(&stack, temp_dir.path());

    assert_eq!(
        plugins,
        HashMap::from([
            (
                "base".to_string(),
                PluginConfig {
                    enabled: true,
                    mcp_servers: HashMap::new(),
                },
            ),
            (
                "profile".to_string(),
                PluginConfig {
                    enabled: false,
                    mcp_servers: HashMap::new(),
                },
            ),
        ])
    );
}

#[tokio::test]
async fn hooks_only_scope_shares_plugin_resolution_without_loading_other_capabilities() {
    let temp_dir = TempDir::new().expect("tempdir");
    let plugin_root = temp_dir.path().join("plugins/cache/test/valid/local");
    write_file(
        &plugin_root.join(".codex-plugin/plugin.json"),
        r#"{"name":"valid"}"#,
    );
    write_file(
        &plugin_root.join("skills/example/SKILL.md"),
        "---\nname: example\ndescription: example skill\n---\n",
    );
    write_file(
        &plugin_root.join(".mcp.json"),
        r#"{"mcpServers":{"example":{"command":"echo"}}}"#,
    );
    write_file(
        &plugin_root.join(".app.json"),
        r#"{"apps":{"example":{"id":"connector_example"}}}"#,
    );
    write_file(
        &plugin_root.join("hooks/hooks.json"),
        r#"{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "echo startup"
          }
        ]
      }
    ]
  }
}"#,
    );

    let disabled_root = temp_dir.path().join("plugins/cache/test/disabled/local");
    write_file(
        &disabled_root.join(".codex-plugin/plugin.json"),
        r#"{"name":"disabled"}"#,
    );
    write_file(
        &disabled_root.join("hooks/hooks.json"),
        r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"echo disabled"}]}]}}"#,
    );

    let malformed_root = temp_dir.path().join("plugins/cache/test/malformed/local");
    write_file(
        &malformed_root.join(".codex-plugin/plugin.json"),
        "not valid json",
    );

    let warning_root = temp_dir.path().join("plugins/cache/test/warning/local");
    write_file(
        &warning_root.join(".codex-plugin/plugin.json"),
        r#"{"name":"warning"}"#,
    );
    write_file(&warning_root.join("hooks/hooks.json"), "not valid json");

    let stack = ConfigLayerStack::new(
        vec![user_layer(
            user_config_path(&temp_dir, "config.toml"),
            r#"
[plugins."valid@test"]
enabled = true

[plugins."disabled@test"]
enabled = false

[plugins.invalid]
enabled = true

[plugins."malformed@test"]
enabled = true

[plugins."missing@test"]
enabled = true

[plugins."warning@test"]
enabled = true
"#,
        )],
        ConfigRequirements::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("valid config layer stack");
    let store = PluginStore::new(temp_dir.path().to_path_buf());

    let full = load_plugins_from_layer_stack(
        &stack,
        RemoteInstalledPluginsSnapshot::default(),
        &store,
        /*plugin_skill_snapshots*/ None,
        Some(Product::Codex),
        /*remote_global_catalog_active*/ false,
        Arc::new(Semaphore::new(MAX_CONCURRENT_ROOT_SCANS)),
    )
    .await;
    let hooks_only = load_plugins_from_layer_stack_with_scope(
        &stack,
        HashMap::new(),
        &store,
        /*remote_global_catalog_active*/ false,
        PluginLoadScope::HooksOnly,
    )
    .await;

    let validation_state = |plugins: &[LoadedPlugin<McpServerConfig>]| {
        plugins
            .iter()
            .map(|plugin| {
                (
                    plugin.config_name.clone(),
                    plugin.enabled,
                    plugin.root.clone(),
                    plugin.error.clone(),
                    plugin.hook_sources.clone(),
                    plugin.hook_load_warnings.clone(),
                )
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(validation_state(&hooks_only), validation_state(&full));

    let full_valid = full
        .iter()
        .find(|plugin| plugin.config_name == "valid@test")
        .expect("full load should include valid plugin");
    assert!(full_valid.manifest_name.is_some());
    assert!(!full_valid.skill_roots.is_empty());
    assert!(!full_valid.mcp_servers.is_empty());
    assert!(!full_valid.apps.is_empty());

    let hooks_only_valid = hooks_only
        .iter()
        .find(|plugin| plugin.config_name == "valid@test")
        .expect("hooks-only load should include valid plugin");
    assert_eq!(hooks_only_valid.manifest_name, None);
    assert!(hooks_only_valid.skill_roots.is_empty());
    assert!(hooks_only_valid.mcp_servers.is_empty());
    assert!(hooks_only_valid.apps.is_empty());
}

#[test]
fn curated_plugin_cache_version_shortens_full_git_sha() {
    assert_eq!(
        curated_plugin_cache_version("0123456789abcdef0123456789abcdef01234567"),
        "01234567"
    );
}

#[test]
fn curated_plugin_cache_version_preserves_non_git_sha_versions() {
    assert_eq!(
        curated_plugin_cache_version("export-backup"),
        "export-backup"
    );
    assert_eq!(curated_plugin_cache_version("0123456"), "0123456");
}

fn plugin_id() -> PluginId {
    PluginId::parse("demo-plugin@test-marketplace").expect("plugin id")
}

fn plugin_root() -> (tempfile::TempDir, AbsolutePathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let plugin_root =
        AbsolutePathBuf::try_from(tmp.path().join("demo-plugin")).expect("plugin root");
    fs::create_dir_all(plugin_root.join(".codex-plugin")).expect("create manifest dir");
    fs::create_dir_all(plugin_root.join("hooks")).expect("create hooks dir");
    (tmp, plugin_root)
}

fn write_manifest(plugin_root: &AbsolutePathBuf, manifest: &str) {
    fs::write(plugin_root.join(".codex-plugin/plugin.json"), manifest).expect("write manifest");
}

fn write_hook_file(plugin_root: &AbsolutePathBuf, relative_path: &str, event: &str, command: &str) {
    fs::write(
        plugin_root.join(relative_path),
        format!(
            r#"{{
  "hooks": {{
    "{event}": [
      {{
        "hooks": [{{ "type": "command", "command": "{command}" }}]
      }}
    ]
  }}
}}"#
        ),
    )
    .expect("write hooks");
}

fn load_sources(plugin_root: &AbsolutePathBuf) -> (Vec<PluginHookSource>, Vec<String>) {
    let manifest = load_plugin_manifest(plugin_root.as_path()).expect("manifest");
    let plugin_data_root = AbsolutePathBuf::try_from(
        plugin_root
            .as_path()
            .parent()
            .expect("plugin root parent")
            .join("plugin-data"),
    )
    .expect("plugin data root");
    load_plugin_hooks(
        plugin_root,
        &plugin_id(),
        &plugin_data_root,
        &manifest.paths,
    )
}

fn assert_sources(sources: &[PluginHookSource], expected_relative_paths: &[&str]) {
    assert_eq!(
        sources
            .iter()
            .map(|source| source.plugin_id.clone())
            .collect::<Vec<_>>(),
        vec![plugin_id(); expected_relative_paths.len()]
    );
    assert_eq!(
        sources
            .iter()
            .map(|source| source.source_relative_path.as_str())
            .collect::<Vec<_>>(),
        expected_relative_paths
    );
    assert_eq!(
        sources
            .iter()
            .map(|source| source.hooks.handler_count())
            .collect::<Vec<_>>(),
        vec![1; expected_relative_paths.len()]
    );
}

#[test]
fn load_plugin_hooks_discovers_default_hooks_file() {
    let (_tmp, plugin_root) = plugin_root();
    write_manifest(&plugin_root, r#"{ "name": "demo-plugin" }"#);
    fs::write(
        plugin_root.join("hooks/hooks.json"),
        r#"{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "Bash",
        "hooks": [{ "type": "command", "command": "echo default" }]
      }
    ]
  }
}"#,
    )
    .expect("write hooks");

    let (sources, warnings) = load_sources(&plugin_root);

    assert_eq!(warnings, Vec::<String>::new());
    assert_sources(&sources, &["hooks/hooks.json"]);
}

#[test]
fn load_plugin_hooks_supports_manifest_hook_path() {
    let (_tmp, plugin_root) = plugin_root();
    write_manifest(
        &plugin_root,
        r#"{
  "name": "demo-plugin",
  "hooks": "./hooks/one.json"
}"#,
    );
    write_hook_file(&plugin_root, "hooks/one.json", "PreToolUse", "echo one");

    let (sources, warnings) = load_sources(&plugin_root);

    assert_eq!(warnings, Vec::<String>::new());
    assert_sources(&sources, &["hooks/one.json"]);
}

#[test]
fn load_plugin_hooks_manifest_paths_replace_default_hooks_file() {
    let (_tmp, plugin_root) = plugin_root();
    write_manifest(
        &plugin_root,
        r#"{
  "name": "demo-plugin",
  "hooks": ["./hooks/one.json", "./hooks/two.json"]
}"#,
    );
    write_hook_file(
        &plugin_root,
        "hooks/hooks.json",
        "PreToolUse",
        "echo ignored",
    );
    write_hook_file(&plugin_root, "hooks/one.json", "PreToolUse", "echo one");
    write_hook_file(&plugin_root, "hooks/two.json", "PostToolUse", "echo two");

    let (sources, warnings) = load_sources(&plugin_root);

    assert_eq!(warnings, Vec::<String>::new());
    assert_sources(&sources, &["hooks/one.json", "hooks/two.json"]);
}

#[test]
fn load_plugin_hooks_supports_inline_manifest_hooks() {
    let (_tmp, plugin_root) = plugin_root();
    write_manifest(
        &plugin_root,
        r#"{
  "name": "demo-plugin",
  "hooks": {
    "hooks": {
      "SessionStart": [
        {
          "matcher": "startup",
          "hooks": [{ "type": "command", "command": "echo inline" }]
        }
      ]
    }
  }
}"#,
    );

    let (sources, warnings) = load_sources(&plugin_root);

    assert_eq!(warnings, Vec::<String>::new());
    assert_sources(&sources, &["plugin.json#hooks[0]"]);
}

#[test]
fn load_plugin_hooks_reports_invalid_hook_file() {
    let (_tmp, plugin_root) = plugin_root();
    write_manifest(&plugin_root, r#"{ "name": "demo-plugin" }"#);
    fs::write(plugin_root.join("hooks/hooks.json"), "{ not-json").expect("write invalid hooks");

    let (sources, warnings) = load_sources(&plugin_root);

    assert_eq!(sources, Vec::<PluginHookSource>::new());
    assert_eq!(
        warnings,
        vec![format!(
            "failed to parse plugin hooks config {}: key must be a string at line 1 column 3",
            plugin_root.join("hooks/hooks.json").display()
        )]
    );
}

#[test]
fn load_plugin_hooks_supports_inline_manifest_hook_list() {
    let (_tmp, plugin_root) = plugin_root();
    write_manifest(
        &plugin_root,
        r#"{
  "name": "demo-plugin",
  "hooks": [
    {
      "hooks": {
        "SessionStart": [
          {
            "hooks": [{ "type": "command", "command": "echo inline one" }]
          }
        ]
      }
    },
    {
      "hooks": {
        "Stop": [
          {
            "hooks": [{ "type": "command", "command": "echo inline two" }]
          }
        ]
      }
    }
  ]
}"#,
    );

    let (sources, warnings) = load_sources(&plugin_root);

    assert_eq!(warnings, Vec::<String>::new());
    assert_sources(&sources, &["plugin.json#hooks[0]", "plugin.json#hooks[1]"]);
}

#[test]
fn materialize_git_subdir_uses_sparse_checkout() {
    let codex_home = tempfile::tempdir().expect("create codex home");
    let repo = tempfile::tempdir().expect("create git repo");
    let plugin_dir = repo.path().join("plugins/toolkit");
    fs::create_dir_all(&plugin_dir).expect("create plugin directory");
    fs::create_dir_all(repo.path().join("plugins/other")).expect("create other plugin");
    fs::write(plugin_dir.join("marker.txt"), "toolkit").expect("write plugin marker");
    fs::write(repo.path().join("plugins/other/marker.txt"), "other").expect("write other marker");
    fs::write(repo.path().join("root.txt"), "root").expect("write root marker");

    run_git(&["init"], Some(repo.path())).expect("init git repo");
    run_git(
        &["config", "user.email", "test@example.com"],
        Some(repo.path()),
    )
    .expect("configure git email");
    run_git(&["config", "user.name", "Test User"], Some(repo.path())).expect("configure git name");
    run_git(&["add", "."], Some(repo.path())).expect("stage git repo");
    run_git(&["commit", "-m", "init"], Some(repo.path())).expect("commit git repo");
    let sha = run_git_output(&["rev-parse", "HEAD"], Some(repo.path())).expect("resolve commit");

    let materialized = materialize_marketplace_plugin_source(
        codex_home.path(),
        &MarketplacePluginSource::Git {
            url: repo.path().display().to_string(),
            path: Some("plugins/toolkit".to_string()),
            ref_name: None,
            sha: Some(sha),
        },
    )
    .expect("materialize git source");

    assert_eq!(
        plugin_dir.file_name(),
        materialized.path.as_path().file_name()
    );
    assert!(materialized.path.as_path().join("marker.txt").is_file());
    let checkout_root = materialized
        .path
        .as_path()
        .parent()
        .and_then(Path::parent)
        .expect("materialized path should be nested under checkout root");
    assert!(!checkout_root.join("root.txt").exists());
    assert!(!checkout_root.join("plugins/other/marker.txt").exists());
}

#[test]
fn materialize_git_source_rejects_sha_that_resolves_to_hostile_default_branch() {
    let codex_home = tempfile::tempdir().expect("create codex home");
    let repo = tempfile::tempdir().expect("create git repo");
    run_git(&["init"], Some(repo.path())).expect("init git repo");
    run_git(
        &["config", "user.email", "test@example.com"],
        Some(repo.path()),
    )
    .expect("configure git email");
    run_git(&["config", "user.name", "Test User"], Some(repo.path())).expect("configure git name");

    fs::write(repo.path().join("marker.txt"), "benign").expect("write benign marker");
    run_git(&["add", "."], Some(repo.path())).expect("stage git repo");
    run_git(&["commit", "-m", "benign"], Some(repo.path())).expect("commit benign revision");
    let benign_sha =
        run_git_output(&["rev-parse", "HEAD"], Some(repo.path())).expect("resolve commit A");

    fs::write(repo.path().join("marker.txt"), "malicious").expect("write malicious marker");
    run_git(&["add", "."], Some(repo.path())).expect("stage malicious revision");
    run_git(&["commit", "-m", "malicious"], Some(repo.path())).expect("commit malicious revision");
    let malicious_sha =
        run_git_output(&["rev-parse", "HEAD"], Some(repo.path())).expect("resolve commit B");
    run_git(&["branch", "-m", &benign_sha], Some(repo.path()))
        .expect("name default branch after commit A");

    let err = materialize_marketplace_plugin_source(
        codex_home.path(),
        &MarketplacePluginSource::Git {
            url: repo.path().display().to_string(),
            path: None,
            ref_name: None,
            sha: Some(benign_sha.clone()),
        },
    )
    .expect_err("hostile default branch must not satisfy SHA pinning");

    assert_eq!(
        err,
        format!("checked out Git SHA {malicious_sha} does not match requested SHA {benign_sha}")
    );
}
