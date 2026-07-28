use anyhow::Result;
use codex_config::CONFIG_TOML_FILE;
use codex_config::MarketplaceConfigUpdate;
use codex_config::record_user_marketplace;
use codex_utils_absolute_path::canonicalize_existing_preserving_symlinks;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::path::Path;
use tempfile::TempDir;

const MARKETPLACE_HEADER: &str = "MARKETPLACE";
const MARKETPLACE_LIST_HEADER: &str = "MARKETPLACE  ROOT";

fn marketplace_list_row(marketplace_name: &str, root: &Path) -> String {
    format!(
        "{marketplace_name:<width$}  {}",
        root.display(),
        width = MARKETPLACE_HEADER.len()
    )
}

fn codex_command(codex_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(codex_utils_cargo_bin::cargo_bin("codex")?);
    cmd.env("CODEX_HOME", codex_home);
    cmd.env("HOME", codex_home);
    Ok(cmd)
}

fn codex_command_in(codex_home: &Path, current_dir: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = codex_command(codex_home)?;
    cmd.current_dir(current_dir);
    Ok(cmd)
}

fn configured_local_marketplace(source: &str) -> MarketplaceConfigUpdate<'_> {
    MarketplaceConfigUpdate {
        last_updated: "2026-05-06T00:00:00Z",
        last_revision: None,
        source_type: "local",
        source,
        ref_name: None,
        sparse_paths: &[],
    }
}

fn write_plugins_enabled_config(codex_home: &Path) -> Result<()> {
    std::fs::write(
        codex_home.join(CONFIG_TOML_FILE),
        r#"[features]
plugins = true
"#,
    )?;
    Ok(())
}

fn write_marketplace_source_with_manifest(source: &Path, marketplace_manifest: &str) -> Result<()> {
    std::fs::create_dir_all(source.join(".agents").join("plugins"))?;
    std::fs::create_dir_all(source.join("plugins").join("sample").join(".codex-plugin"))?;
    std::fs::write(
        source
            .join(".agents")
            .join("plugins")
            .join("marketplace.json"),
        marketplace_manifest,
    )?;
    std::fs::write(
        source
            .join("plugins")
            .join("sample")
            .join(".codex-plugin")
            .join("plugin.json"),
        r#"{"name":"sample","version":"1.2.3","description":"Sample plugin"}"#,
    )?;
    Ok(())
}

fn write_marketplace_source(source: &Path) -> Result<()> {
    write_marketplace_source_with_manifest(
        source,
        r#"{
  "name": "debug",
  "plugins": [
    {
      "name": "sample",
      "source": {
        "source": "local",
        "path": "./plugins/sample"
      }
    }
  ]
}"#,
    )
}

fn write_marketplace_source_with_explicit_empty_products(source: &Path) -> Result<()> {
    write_marketplace_source_with_manifest(
        source,
        r#"{
  "name": "debug",
  "plugins": [
    {
      "name": "sample",
      "source": {
        "source": "local",
        "path": "./plugins/sample"
      },
      "policy": {
        "products": []
      }
    }
  ]
}"#,
    )
}

fn setup_local_marketplace() -> Result<(TempDir, TempDir)> {
    let codex_home = TempDir::new()?;
    let source = TempDir::new()?;
    write_plugins_enabled_config(codex_home.path())?;
    write_marketplace_source(source.path())?;
    let source_path = source.path().to_string_lossy().into_owned();
    record_user_marketplace(
        codex_home.path(),
        "debug",
        &configured_local_marketplace(&source_path),
    )?;
    Ok((codex_home, source))
}

fn customer_plugin_setup() -> serde_json::Value {
    json!({
        "inputs": [
            {
                "id": "project_root",
                "type": "directory",
                "prompt": "Project directory",
                "env": "CUSTOMER_PROJECT_ROOT"
            },
            {
                "id": "api_key",
                "type": "secret",
                "prompt": "API key",
                "env": "CUSTOMER_API_KEY"
            }
        ],
        "commands": [
            {
                "name": "configure project",
                "command": ["python3", "./scripts/configure.py"]
            },
            {
                "name": "verify connection",
                "command": ["python3", "./scripts/verify.py"]
            }
        ]
    })
}

fn setup_customer_plugin_marketplace(
    setup: serde_json::Value,
    feature_enabled: bool,
) -> Result<(TempDir, TempDir)> {
    let (codex_home, source) = setup_local_marketplace()?;
    if feature_enabled {
        let config_path = codex_home.path().join(CONFIG_TOML_FILE);
        let config = std::fs::read_to_string(&config_path)?;
        std::fs::write(
            config_path,
            config.replace("plugins = true\n", "plugins = true\nplugin_setup = true\n"),
        )?;
    }

    let plugin_root = source.path().join("plugins/sample");
    let manifest = json!({
        "name": "sample",
        "version": "1.2.3",
        "description": "Sample customer integration",
        "setup": setup
    });
    std::fs::write(
        plugin_root.join(".codex-plugin/plugin.json"),
        serde_json::to_string_pretty(&manifest)?,
    )?;
    let scripts = plugin_root.join("scripts");
    std::fs::create_dir_all(&scripts)?;
    std::fs::write(
        scripts.join("configure.py"),
        r#"import json
import os
from pathlib import Path

state_path = Path(os.environ["PLUGIN_DATA"]) / "setup-state.json"
previous = json.loads(state_path.read_text()) if state_path.exists() else {}
state = {
    "project_root": os.environ.get("CUSTOMER_PROJECT_ROOT"),
    "has_api_key": bool(os.environ.get("CUSTOMER_API_KEY")),
    "plugin_root": os.environ["PLUGIN_ROOT"],
    "legacy_plugin_root": os.environ["CLAUDE_PLUGIN_ROOT"],
    "legacy_plugin_data": os.environ["CLAUDE_PLUGIN_DATA"],
    "run_count": previous.get("run_count", 0) + 1,
    "steps": ["configure project"],
}
state_path.write_text(json.dumps(state))
"#,
    )?;
    std::fs::write(
        scripts.join("verify.py"),
        r#"import json
import os
from pathlib import Path

state_path = Path(os.environ["PLUGIN_DATA"]) / "setup-state.json"
state = json.loads(state_path.read_text())
if not state["has_api_key"]:
    raise SystemExit("API key is missing")
state["steps"].append("verify connection")
state_path.write_text(json.dumps(state))
"#,
    )?;
    Ok((codex_home, source))
}

fn setup_unconfigured_local_marketplace() -> Result<(TempDir, TempDir)> {
    let codex_home = TempDir::new()?;
    let source = TempDir::new()?;
    write_plugins_enabled_config(codex_home.path())?;
    write_marketplace_source(source.path())?;
    Ok((codex_home, source))
}

fn setup_local_marketplace_with_explicit_empty_products() -> Result<(TempDir, TempDir)> {
    let codex_home = TempDir::new()?;
    let source = TempDir::new()?;
    write_plugins_enabled_config(codex_home.path())?;
    write_marketplace_source_with_explicit_empty_products(source.path())?;
    let source_path = source.path().to_string_lossy().into_owned();
    record_user_marketplace(
        codex_home.path(),
        "debug",
        &configured_local_marketplace(&source_path),
    )?;
    Ok((codex_home, source))
}

fn setup_configured_marketplace_without_manifest() -> Result<(TempDir, TempDir)> {
    let codex_home = TempDir::new()?;
    let source = TempDir::new()?;
    write_plugins_enabled_config(codex_home.path())?;
    let source_path = source.path().to_string_lossy().into_owned();
    record_user_marketplace(
        codex_home.path(),
        "debug",
        &configured_local_marketplace(&source_path),
    )?;
    Ok((codex_home, source))
}

fn setup_configured_marketplace_with_malformed_manifest() -> Result<(TempDir, TempDir)> {
    let codex_home = TempDir::new()?;
    let source = TempDir::new()?;
    write_plugins_enabled_config(codex_home.path())?;
    std::fs::create_dir_all(source.path().join(".agents").join("plugins"))?;
    std::fs::write(
        source
            .path()
            .join(".agents")
            .join("plugins")
            .join("marketplace.json"),
        "{not valid json",
    )?;
    let source_path = source.path().to_string_lossy().into_owned();
    record_user_marketplace(
        codex_home.path(),
        "debug",
        &configured_local_marketplace(&source_path),
    )?;
    Ok((codex_home, source))
}

fn setup_local_marketplace_with_implicit_system_roots() -> Result<(TempDir, TempDir, TempDir)> {
    let (codex_home, source) = setup_local_marketplace()?;

    let bundled_root = codex_home
        .path()
        .join(".tmp")
        .join("bundled-marketplaces")
        .join("openai-bundled");
    std::fs::create_dir_all(&bundled_root)?;
    let bundled_source = bundled_root.display().to_string();
    record_user_marketplace(
        codex_home.path(),
        "openai-bundled",
        &configured_local_marketplace(&bundled_source),
    )?;

    let cache_home = TempDir::new()?;
    let runtime_root = cache_home
        .path()
        .join(".cache")
        .join("codex-runtimes")
        .join("codex-primary-runtime")
        .join("plugins")
        .join("openai-primary-runtime");
    std::fs::create_dir_all(&runtime_root)?;
    let runtime_source = runtime_root.display().to_string();
    record_user_marketplace(
        codex_home.path(),
        "openai-primary-runtime",
        &configured_local_marketplace(&runtime_source),
    )?;

    Ok((codex_home, source, cache_home))
}

fn setup_custom_marketplace_under_implicit_system_root() -> Result<(TempDir, std::path::PathBuf)> {
    let codex_home = TempDir::new()?;
    write_plugins_enabled_config(codex_home.path())?;

    let custom_root = codex_home
        .path()
        .join(".tmp")
        .join("bundled-marketplaces")
        .join("custom-marketplace");
    std::fs::create_dir_all(&custom_root)?;
    let custom_source = custom_root.display().to_string();
    record_user_marketplace(
        codex_home.path(),
        "custom-marketplace",
        &configured_local_marketplace(&custom_source),
    )?;

    Ok((codex_home, custom_root))
}

fn remove_installed_plugin_config(codex_home: &Path, plugin_key: &str) -> Result<()> {
    let config_path = codex_home.join(CONFIG_TOML_FILE);
    let plugin_header = format!("[plugins.\"{plugin_key}\"]");
    let config = std::fs::read_to_string(&config_path)?;
    let mut rewritten = Vec::new();
    let mut skipping = false;

    for line in config.lines() {
        if line == plugin_header {
            skipping = true;
            continue;
        }
        if skipping && line.starts_with('[') {
            skipping = false;
        }
        if !skipping {
            rewritten.push(line);
        }
    }

    std::fs::write(config_path, format!("{}\n", rewritten.join("\n")))?;
    Ok(())
}

fn setup_configured_local_marketplace_with_missing_source() -> Result<TempDir> {
    let codex_home = TempDir::new()?;
    std::fs::write(
        codex_home.path().join(CONFIG_TOML_FILE),
        r#"[features]
plugins = true

[marketplaces.debug]
source_type = "local"
"#,
    )?;
    Ok(codex_home)
}

fn setup_configured_local_marketplace_with_invalid_name() -> Result<TempDir> {
    let codex_home = TempDir::new()?;
    std::fs::write(
        codex_home.path().join(CONFIG_TOML_FILE),
        r#"[features]
plugins = true

[marketplaces."bad/name"]
source_type = "local"
source = "/tmp/debug"
"#,
    )?;
    Ok(codex_home)
}

fn assert_configured_marketplace_snapshot_failure(
    assert: assert_cmd::assert::Assert,
    source: &Path,
    detail: &str,
) {
    assert
        .failure()
        .stderr(contains(
            "failed to load configured marketplace snapshot(s):",
        ))
        .stderr(contains("`debug`"))
        .stderr(contains(source.display().to_string()))
        .stderr(contains(detail));
}

fn assert_marketplace_failure(
    assert: assert_cmd::assert::Assert,
    marketplace_name: &str,
    source: &Path,
    detail: &str,
) {
    assert
        .failure()
        .stderr(contains("failed to load marketplace(s):"))
        .stderr(contains(format!("`{marketplace_name}`")))
        .stderr(contains(source.display().to_string()))
        .stderr(contains(detail));
}

#[tokio::test]
async fn marketplace_list_shows_configured_marketplace_names() -> Result<()> {
    let (codex_home, source) = setup_local_marketplace()?;
    let expected_row = marketplace_list_row("debug", source.path());

    codex_command(codex_home.path())?
        .args(["plugin", "marketplace", "list"])
        .assert()
        .success()
        .stdout(contains(MARKETPLACE_LIST_HEADER))
        .stdout(contains(&expected_row))
        .stdout(contains("\t").not());

    Ok(())
}

#[tokio::test]
async fn marketplace_list_json_prints_configured_marketplaces() -> Result<()> {
    let (codex_home, source) = setup_local_marketplace()?;
    let source_path = source.path().display().to_string();

    let assert = codex_command(codex_home.path())?
        .args(["plugin", "marketplace", "list", "--json"])
        .assert()
        .success();
    let stdout = assert.get_output().stdout.as_slice();
    let actual: serde_json::Value = serde_json::from_slice(stdout)?;

    assert_eq!(
        actual,
        json!({
            "marketplaces": [
                {
                    "name": "debug",
                    "root": source_path,
                    "marketplaceSource": {
                        "sourceType": "local",
                        "source": source_path,
                    },
                },
            ],
        })
    );

    Ok(())
}

#[tokio::test]
async fn marketplace_list_json_includes_configured_git_marketplace_source() -> Result<()> {
    let codex_home = TempDir::new()?;
    let marketplace_root = codex_home
        .path()
        .join(".tmp")
        .join("marketplaces")
        .join("debug");
    write_plugins_enabled_config(codex_home.path())?;
    write_marketplace_source(&marketplace_root)?;
    let update = MarketplaceConfigUpdate {
        last_updated: "2026-06-04T08:39:49Z",
        last_revision: Some("abc123"),
        source_type: "git",
        source: "https://example.com/acme/agent-skills.git",
        ref_name: None,
        sparse_paths: &[],
    };
    record_user_marketplace(codex_home.path(), "debug", &update)?;
    let normalized_root = canonicalize_existing_preserving_symlinks(&marketplace_root)?;

    let assert = codex_command(codex_home.path())?
        .args(["plugin", "marketplace", "list", "--json"])
        .assert()
        .success();
    let stdout = assert.get_output().stdout.as_slice();
    let actual: serde_json::Value = serde_json::from_slice(stdout)?;

    assert_eq!(
        actual,
        json!({
            "marketplaces": [
                {
                    "name": "debug",
                    "root": normalized_root.display().to_string(),
                    "marketplaceSource": {
                        "sourceType": "git",
                        "source": "https://example.com/acme/agent-skills.git",
                    },
                },
            ],
        })
    );

    Ok(())
}

#[tokio::test]
async fn marketplace_list_json_keys_configured_source_by_root() -> Result<()> {
    let codex_home = TempDir::new()?;
    let home = TempDir::new()?;
    let marketplace_root = codex_home
        .path()
        .join(".tmp")
        .join("marketplaces")
        .join("debug");
    write_plugins_enabled_config(codex_home.path())?;
    write_marketplace_source(home.path())?;
    write_marketplace_source(&marketplace_root)?;
    let update = MarketplaceConfigUpdate {
        last_updated: "2026-06-04T08:39:49Z",
        last_revision: Some("abc123"),
        source_type: "git",
        source: "https://example.com/acme/agent-skills.git",
        ref_name: None,
        sparse_paths: &[],
    };
    record_user_marketplace(codex_home.path(), "debug", &update)?;
    let normalized_root = canonicalize_existing_preserving_symlinks(&marketplace_root)?;

    let assert = codex_command(codex_home.path())?
        .env("HOME", home.path())
        .args(["plugin", "marketplace", "list", "--json"])
        .assert()
        .success();
    let stdout = assert.get_output().stdout.as_slice();
    let actual: serde_json::Value = serde_json::from_slice(stdout)?;

    assert_eq!(
        actual,
        json!({
            "marketplaces": [
                {
                    "name": "debug",
                    "root": home.path().display().to_string(),
                },
                {
                    "name": "debug",
                    "root": normalized_root.display().to_string(),
                    "marketplaceSource": {
                        "sourceType": "git",
                        "source": "https://example.com/acme/agent-skills.git",
                    },
                },
            ],
        })
    );

    Ok(())
}

#[tokio::test]
async fn marketplace_list_includes_home_marketplace_when_present() -> Result<()> {
    let codex_home = TempDir::new()?;
    let home = TempDir::new()?;
    write_marketplace_source(home.path())?;
    write_plugins_enabled_config(codex_home.path())?;
    let expected_row = marketplace_list_row("debug", home.path());

    codex_command(codex_home.path())?
        .env("HOME", home.path())
        .args(["plugin", "marketplace", "list"])
        .assert()
        .success()
        .stdout(contains(MARKETPLACE_LIST_HEADER))
        .stdout(contains(&expected_row))
        .stdout(contains("\t").not());

    Ok(())
}

#[tokio::test]
async fn marketplace_list_includes_root_when_plugins_are_filtered_out() -> Result<()> {
    let (codex_home, source) = setup_local_marketplace_with_explicit_empty_products()?;
    let expected_row = marketplace_list_row("debug", source.path());

    codex_command(codex_home.path())?
        .args(["plugin", "marketplace", "list"])
        .assert()
        .success()
        .stdout(contains(MARKETPLACE_LIST_HEADER))
        .stdout(contains(&expected_row));

    Ok(())
}

#[tokio::test]
async fn marketplace_list_fails_when_configured_marketplace_snapshot_is_missing() -> Result<()> {
    let (codex_home, source) = setup_configured_marketplace_without_manifest()?;

    assert_marketplace_failure(
        codex_command(codex_home.path())?
            .args(["plugin", "marketplace", "list"])
            .assert(),
        "debug",
        source.path(),
        "marketplace root does not contain a supported manifest",
    );

    Ok(())
}

#[tokio::test]
async fn marketplace_list_fails_when_configured_marketplace_name_is_invalid() -> Result<()> {
    let codex_home = setup_configured_local_marketplace_with_invalid_name()?;

    assert_marketplace_failure(
        codex_command(codex_home.path())?
            .args(["plugin", "marketplace", "list"])
            .assert(),
        "bad/name",
        Path::new("<invalid config>"),
        "marketplace name",
    );

    Ok(())
}

#[tokio::test]
async fn marketplace_list_fails_when_configured_local_marketplace_source_is_missing() -> Result<()>
{
    let codex_home = setup_configured_local_marketplace_with_missing_source()?;

    codex_command(codex_home.path())?
        .args(["plugin", "marketplace", "list"])
        .assert()
        .failure()
        .stderr(contains("failed to load marketplace(s):"))
        .stderr(contains("`debug`"))
        .stderr(contains("<invalid source>"))
        .stderr(contains(
            "configured local marketplace source is missing or empty",
        ));

    Ok(())
}

#[tokio::test]
async fn marketplace_list_fails_when_home_marketplace_is_malformed() -> Result<()> {
    let codex_home = TempDir::new()?;
    let home = TempDir::new()?;
    write_plugins_enabled_config(codex_home.path())?;
    std::fs::create_dir_all(home.path().join(".agents/plugins"))?;
    let home_marketplace_path = home
        .path()
        .join(".agents")
        .join("plugins")
        .join("marketplace.json");
    std::fs::write(&home_marketplace_path, "{not valid json")?;

    codex_command(codex_home.path())?
        .env("HOME", home.path())
        .args(["plugin", "marketplace", "list"])
        .assert()
        .failure()
        .stderr(contains("failed to load marketplace(s):"))
        .stderr(contains(home_marketplace_path.display().to_string()))
        .stderr(contains("key must be a string"));

    Ok(())
}

#[tokio::test]
async fn marketplace_list_fails_when_configured_marketplace_snapshot_is_malformed() -> Result<()> {
    let (codex_home, source) = setup_configured_marketplace_with_malformed_manifest()?;

    assert_marketplace_failure(
        codex_command(codex_home.path())?
            .args(["plugin", "marketplace", "list"])
            .assert(),
        "debug",
        source.path(),
        "key must be a string",
    );

    Ok(())
}

#[tokio::test]
async fn plugin_list_prints_plugins_in_a_table() -> Result<()> {
    let (codex_home, source) = setup_local_marketplace()?;
    let marketplace_manifest = source
        .path()
        .join(".agents")
        .join("plugins")
        .join("marketplace.json");
    let plugin_path = source.path().join("plugins").join("sample");

    codex_command(codex_home.path())?
        .args(["plugin", "list"])
        .assert()
        .success()
        .stdout(contains("Marketplace `debug`"))
        .stdout(contains("PLUGIN"))
        .stdout(contains("STATUS"))
        .stdout(contains("VERSION"))
        .stdout(contains("PATH"))
        .stdout(contains(marketplace_manifest.display().to_string()))
        .stdout(contains("sample@debug"))
        .stdout(contains("not installed"))
        .stdout(contains(plugin_path.display().to_string()));

    Ok(())
}

#[tokio::test]
async fn plugin_list_json_prints_available_plugins_when_requested() -> Result<()> {
    let (codex_home, source) = setup_local_marketplace()?;
    let plugin_path = source.path().join("plugins").join("sample");
    let source_path = source.path().to_string_lossy().into_owned();

    let assert = codex_command(codex_home.path())?
        .args(["plugin", "list", "--available", "--json"])
        .assert()
        .success();
    let stdout = assert.get_output().stdout.as_slice();
    let actual: serde_json::Value = serde_json::from_slice(stdout)?;

    assert_eq!(
        actual,
        json!({
            "installed": [],
            "available": [
                {
                    "pluginId": "sample@debug",
                    "name": "sample",
                    "marketplaceName": "debug",
                    "version": "1.2.3",
                    "installed": false,
                    "enabled": false,
                    "source": {
                        "source": "local",
                        "path": plugin_path.display().to_string(),
                    },
                    "marketplaceSource": {
                        "sourceType": "local",
                        "source": source_path,
                    },
                    "installPolicy": "AVAILABLE",
                    "authPolicy": "ON_INSTALL",
                },
            ],
        })
    );

    Ok(())
}

#[tokio::test]
async fn plugin_list_json_includes_configured_git_marketplace_source() -> Result<()> {
    let codex_home = TempDir::new()?;
    let marketplace_root = codex_home
        .path()
        .join(".tmp")
        .join("marketplaces")
        .join("debug");
    write_plugins_enabled_config(codex_home.path())?;
    write_marketplace_source(&marketplace_root)?;
    let update = MarketplaceConfigUpdate {
        last_updated: "2026-06-04T08:39:49Z",
        last_revision: Some("abc123"),
        source_type: "git",
        source: "https://example.com/acme/agent-skills.git",
        ref_name: None,
        sparse_paths: &[],
    };
    record_user_marketplace(codex_home.path(), "debug", &update)?;
    let plugin_path = marketplace_root.join("plugins").join("sample");
    let normalized_plugin_path = canonicalize_existing_preserving_symlinks(&plugin_path)?;

    let assert = codex_command(codex_home.path())?
        .args(["plugin", "list", "--available", "--json"])
        .assert()
        .success();
    let stdout = assert.get_output().stdout.as_slice();
    let actual: serde_json::Value = serde_json::from_slice(stdout)?;

    assert_eq!(
        actual,
        json!({
            "installed": [],
            "available": [
                {
                    "pluginId": "sample@debug",
                    "name": "sample",
                    "marketplaceName": "debug",
                    "version": "1.2.3",
                    "installed": false,
                    "enabled": false,
                    "source": {
                        "source": "local",
                        "path": normalized_plugin_path.display().to_string(),
                    },
                    "marketplaceSource": {
                        "sourceType": "git",
                        "source": "https://example.com/acme/agent-skills.git",
                    },
                    "installPolicy": "AVAILABLE",
                    "authPolicy": "ON_INSTALL",
                },
            ],
        })
    );

    Ok(())
}

#[tokio::test]
async fn plugin_list_json_prints_installed_plugins() -> Result<()> {
    let (codex_home, source) = setup_local_marketplace()?;
    let plugin_path = source.path().join("plugins").join("sample");
    let source_path = source.path().to_string_lossy().into_owned();

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug"])
        .assert()
        .success();

    let assert = codex_command(codex_home.path())?
        .args(["plugin", "list", "--json"])
        .assert()
        .success();
    let stdout = assert.get_output().stdout.as_slice();
    let actual: serde_json::Value = serde_json::from_slice(stdout)?;

    assert_eq!(
        actual,
        json!({
            "installed": [
                {
                    "pluginId": "sample@debug",
                    "name": "sample",
                    "marketplaceName": "debug",
                    "version": "1.2.3",
                    "installed": true,
                    "enabled": true,
                    "source": {
                        "source": "local",
                        "path": plugin_path.display().to_string(),
                    },
                    "marketplaceSource": {
                        "sourceType": "local",
                        "source": source_path,
                    },
                    "installPolicy": "AVAILABLE",
                    "authPolicy": "ON_INSTALL",
                },
            ],
            "available": [],
        })
    );

    Ok(())
}

#[tokio::test]
async fn plugin_list_available_requires_json() -> Result<()> {
    let (codex_home, _source) = setup_local_marketplace()?;

    codex_command(codex_home.path())?
        .args(["plugin", "list", "--available"])
        .assert()
        .failure()
        .stderr(contains(
            "the following required arguments were not provided",
        ))
        .stderr(contains("--json"));

    Ok(())
}

#[tokio::test]
async fn plugin_list_shows_installed_version_when_plugin_is_installed() -> Result<()> {
    let (codex_home, _source) = setup_local_marketplace()?;

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug"])
        .assert()
        .success();

    codex_command(codex_home.path())?
        .args(["plugin", "list"])
        .assert()
        .success()
        .stdout(contains("sample@debug"))
        .stdout(contains("1.2.3"))
        .stdout(contains("installed, enabled"));

    Ok(())
}

#[tokio::test]
async fn plugin_list_excludes_unconfigured_repo_local_marketplaces() -> Result<()> {
    let (codex_home, source) = setup_unconfigured_local_marketplace()?;

    codex_command_in(codex_home.path(), source.path())?
        .args(["plugin", "list", "--marketplace", "debug"])
        .assert()
        .success()
        .stdout(contains("No plugins found in marketplace `debug`."))
        .stdout(predicates::str::is_match("sample@debug").unwrap().not());

    Ok(())
}

#[tokio::test]
async fn plugin_list_fails_when_configured_marketplace_snapshot_is_missing() -> Result<()> {
    let (codex_home, source) = setup_configured_marketplace_without_manifest()?;

    assert_configured_marketplace_snapshot_failure(
        codex_command(codex_home.path())?
            .args(["plugin", "list"])
            .assert(),
        source.path(),
        "marketplace root does not contain a supported manifest",
    );

    Ok(())
}

#[tokio::test]
async fn plugin_list_ignores_implicit_system_marketplace_roots_without_manifests() -> Result<()> {
    let (codex_home, source, cache_home) = setup_local_marketplace_with_implicit_system_roots()?;

    codex_command(codex_home.path())?
        .env("HOME", cache_home.path())
        .env("USERPROFILE", cache_home.path())
        .args(["plugin", "list"])
        .assert()
        .success()
        .stdout(contains("Marketplace `debug`"))
        .stdout(contains(
            source
                .path()
                .join(".agents")
                .join("plugins")
                .join("marketplace.json")
                .display()
                .to_string(),
        ))
        .stderr(
            predicates::str::contains("failed to load configured marketplace snapshot(s):").not(),
        );

    Ok(())
}

#[tokio::test]
async fn plugin_list_fails_for_custom_marketplace_under_system_root() -> Result<()> {
    let (codex_home, custom_root) = setup_custom_marketplace_under_implicit_system_root()?;

    codex_command(codex_home.path())?
        .args(["plugin", "list"])
        .assert()
        .failure()
        .stderr(contains(
            "failed to load configured marketplace snapshot(s):",
        ))
        .stderr(contains("`custom-marketplace`"))
        .stderr(contains(custom_root.display().to_string()))
        .stderr(contains(
            "marketplace root does not contain a supported manifest",
        ));

    Ok(())
}

#[tokio::test]
async fn plugin_list_hides_version_for_cached_but_unconfigured_plugin() -> Result<()> {
    let (codex_home, _source) = setup_local_marketplace()?;

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug"])
        .assert()
        .success();

    remove_installed_plugin_config(codex_home.path(), "sample@debug")?;

    codex_command(codex_home.path())?
        .args(["plugin", "list"])
        .assert()
        .success()
        .stdout(contains("sample@debug"))
        .stdout(contains("not installed"))
        .stdout(predicates::str::contains("1.2.3").not());

    Ok(())
}

#[tokio::test]
async fn plugin_add_and_remove_updates_installed_plugin_config() -> Result<()> {
    let (codex_home, _source) = setup_local_marketplace()?;

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug"])
        .assert()
        .success()
        .stdout(contains("Added plugin `sample` from marketplace `debug`."));

    let config = std::fs::read_to_string(codex_home.path().join(CONFIG_TOML_FILE))?;
    assert!(config.contains("[plugins.\"sample@debug\"]"));

    codex_command(codex_home.path())?
        .args(["plugin", "remove", "sample", "--marketplace", "debug"])
        .assert()
        .success()
        .stdout(contains(
            "Removed plugin `sample` from marketplace `debug`.",
        ));

    let config = std::fs::read_to_string(codex_home.path().join(CONFIG_TOML_FILE))?;
    assert!(!config.contains("[plugins.\"sample@debug\"]"));

    Ok(())
}

#[tokio::test]
async fn plugin_add_json_prints_install_outcome() -> Result<()> {
    let (codex_home, _source) = setup_local_marketplace()?;

    let assert = codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug", "--json"])
        .assert()
        .success();
    let stdout = assert.get_output().stdout.as_slice();
    let actual: serde_json::Value = serde_json::from_slice(stdout)?;
    let installed_path = codex_home.path().join("plugins/cache/debug/sample/1.2.3");
    let normalized_installed_path = canonicalize_existing_preserving_symlinks(&installed_path)?;

    assert_eq!(
        actual,
        json!({
            "pluginId": "sample@debug",
            "name": "sample",
            "marketplaceName": "debug",
            "version": "1.2.3",
            "installedPath": normalized_installed_path.display().to_string(),
            "authPolicy": "ON_INSTALL",
        })
    );

    Ok(())
}

#[tokio::test]
async fn plugin_add_rejects_setup_when_feature_is_disabled() -> Result<()> {
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(customer_plugin_setup(), /*feature_enabled*/ false)?;

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug"])
        .assert()
        .failure()
        .stderr(contains("plugin_setup"));

    let config = std::fs::read_to_string(codex_home.path().join(CONFIG_TOML_FILE))?;
    assert!(!config.contains("[plugins.\"sample@debug\"]"));
    assert!(
        !codex_home
            .path()
            .join("plugins/data/setup/debug/sample/setup-state.json")
            .exists()
    );
    Ok(())
}

#[tokio::test]
async fn plugin_setup_flags_require_enabled_experimental_feature() -> Result<()> {
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(customer_plugin_setup(), /*feature_enabled*/ false)?;

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug", "--run-setup"])
        .assert()
        .failure()
        .stderr(contains("codex features enable plugin_setup"));

    codex_command(codex_home.path())?
        .args(["plugin", "setup", "sample@debug", "--yes"])
        .assert()
        .failure()
        .stderr(contains("codex features enable plugin_setup"));
    Ok(())
}

#[cfg(not(unix))]
#[tokio::test]
async fn plugin_setup_refuses_unsupported_platform_before_mutation() -> Result<()> {
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(customer_plugin_setup(), /*feature_enabled*/ true)?;

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug"])
        .assert()
        .failure()
        .stderr(contains("declares setup commands"));

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug", "--run-setup", "--json"])
        .assert()
        .failure()
        .stderr(contains("supported only on Unix"));

    let config = std::fs::read_to_string(codex_home.path().join(CONFIG_TOML_FILE))?;
    assert!(!config.contains("[plugins.\"sample@debug\"]"));
    assert!(
        !codex_home
            .path()
            .join("plugins/cache/debug/sample")
            .exists()
    );
    assert!(
        !codex_home
            .path()
            .join("plugins/data/setup/debug/sample")
            .exists()
    );

    codex_command(codex_home.path())?
        .args(["plugin", "setup", "sample@debug", "--yes"])
        .assert()
        .failure()
        .stderr(contains("supported only on Unix"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_add_runs_ordered_customer_setup_and_keeps_json_clean() -> Result<()> {
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(customer_plugin_setup(), /*feature_enabled*/ true)?;
    let project_root = codex_home.path().join("customer-project");
    std::fs::create_dir_all(&project_root)?;
    let secret = "fixture-secret-must-not-appear-in-output";

    let assert = codex_command(codex_home.path())?
        .env("CUSTOMER_API_KEY", secret)
        .args([
            "plugin",
            "add",
            "sample@debug",
            "--run-setup",
            "--json",
            "--set",
        ])
        .arg(format!("project_root={}", project_root.display()))
        .assert()
        .success();
    let output = assert.get_output();
    let actual: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let installed_path = actual["installedPath"]
        .as_str()
        .expect("installed plugin root");
    assert!(installed_path.contains("/@setup-"));
    assert_eq!(actual["pluginId"], json!("sample@debug"));
    assert_eq!(actual["version"], json!("1.2.3"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
    assert!(
        Path::new(installed_path)
            .join(".codex-plugin/setup-complete.json")
            .is_file()
    );

    let data_root = codex_home.path().join("plugins/data/setup/debug/sample");
    let state: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        data_root.join("setup-state.json"),
    )?)?;
    let normalized_project_root = std::fs::canonicalize(&project_root)?;
    let normalized_data_root = std::fs::canonicalize(&data_root)?;
    assert_eq!(
        state,
        json!({
            "project_root": normalized_project_root.to_str().expect("UTF-8 project root"),
            "has_api_key": true,
            "plugin_root": installed_path,
            "legacy_plugin_root": installed_path,
            "legacy_plugin_data": normalized_data_root.to_str().expect("UTF-8 plugin data"),
            "run_count": 1,
            "steps": ["configure project", "verify connection"],
        })
    );

    let listing = codex_command(codex_home.path())?
        .args(["plugin", "list", "--json"])
        .assert()
        .success();
    let listed: serde_json::Value = serde_json::from_slice(&listing.get_output().stdout)?;
    assert_eq!(listed["installed"][0]["pluginId"], json!("sample@debug"));
    assert_eq!(listed["installed"][0]["version"], json!("1.2.3"));
    assert!(!String::from_utf8_lossy(&listing.get_output().stdout).contains("@setup-"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_add_and_rerun_portable_customer_setup_without_legacy_bypass() -> Result<()> {
    let setup = customer_plugin_setup();
    let (codex_home, source) =
        setup_customer_plugin_marketplace(setup.clone(), /*feature_enabled*/ true)?;
    let plugin_root = source.path().join("plugins/sample");
    std::fs::write(
        plugin_root.join("plugin.json"),
        serde_json::to_vec_pretty(&json!({
            "$schema": "https://agent-plugins.org/schemas/1.0.0/plugin.schema.json",
            "name": "sample",
            "version": "1.2.3",
            "extensions": {
                "com.openai": {
                    "setup": setup,
                    "interface": {"displayName": "Portable Customer Tools"}
                }
            }
        }))?,
    )?;
    std::fs::write(
        plugin_root.join(".codex-plugin/plugin.json"),
        serde_json::to_vec_pretty(&json!({
            "name": "sample",
            "interface": {"displayName": "Harmless Legacy Overlay"}
        }))?,
    )?;
    let project_root = codex_home.path().join("customer-project");
    std::fs::create_dir_all(&project_root)?;
    let project_arg = format!("project_root={}", project_root.display());

    let installation = codex_command(codex_home.path())?
        .env("CUSTOMER_API_KEY", "portable-customer-fixture-secret")
        .args([
            "plugin",
            "add",
            "sample@debug",
            "--run-setup",
            "--json",
            "--set",
        ])
        .arg(&project_arg)
        .assert()
        .success();
    let installed: serde_json::Value = serde_json::from_slice(&installation.get_output().stdout)?;
    assert_eq!(installed["pluginId"], json!("sample@debug"));
    assert_eq!(installed["version"], json!("1.2.3"));

    let rerun = codex_command(codex_home.path())?
        .env("CUSTOMER_API_KEY", "portable-customer-fixture-secret")
        .args([
            "plugin",
            "setup",
            "sample@debug",
            "--yes",
            "--json",
            "--set",
        ])
        .arg(&project_arg)
        .assert()
        .success();
    let rerun_output: serde_json::Value = serde_json::from_slice(&rerun.get_output().stdout)?;
    assert_eq!(rerun_output["status"], json!("completed"));

    let state: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        codex_home
            .path()
            .join("plugins/data/setup/debug/sample/setup-state.json"),
    )?)?;
    assert_eq!(state["has_api_key"], json!(true));
    assert_eq!(state["run_count"], json!(2));

    let listing = codex_command(codex_home.path())?
        .args(["plugin", "list", "--json"])
        .assert()
        .success();
    let listed: serde_json::Value = serde_json::from_slice(&listing.get_output().stdout)?;
    assert_eq!(listed["installed"][0]["version"], json!("1.2.3"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_add_collects_hidden_inputs_and_runs_interactive_steps_in_a_real_terminal()
-> Result<()> {
    let mut setup = customer_plugin_setup();
    let commands = setup["commands"]
        .as_array_mut()
        .expect("customer setup command list");
    commands.insert(
        0,
        json!({
            "name": "interactive customer authentication",
            "command": [
                "python3",
                "-c",
                "import os, sys; assert all(os.isatty(fd) for fd in (0, 1, 2)), 'setup requires a real terminal'; assert os.tcgetpgrp(0) == os.getpgrp(), 'setup must own the foreground terminal'; code=input('Customer verification code: '); assert code == 'verification-ok', 'incorrect verification code'; assert os.environ.get('CUSTOMER_API_KEY'), 'missing hidden customer credential'; print('Customer terminal authentication completed.')"
            ],
            "interactive": true
        }),
    );
    commands.insert(
        1,
        json!({
            "name": "verify terminal handoff was restored",
            "command": [
                "python3",
                "-c",
                "import os; assert all(os.isatty(fd) for fd in (0, 1, 2)); assert os.tcgetpgrp(0) == os.getpgrp(), 'foreground terminal was not restored between setup steps'; print('Customer terminal handoff verified.')"
            ],
            "interactive": true
        }),
    );
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(setup, /*feature_enabled*/ true)?;
    let project_root = codex_home.path().join("customer-project");
    std::fs::create_dir_all(&project_root)?;
    let secret = "fixture-hidden-customer-terminal-secret";
    let executable = codex_utils_cargo_bin::cargo_bin("codex")?;
    let pty_driver = r#"
import errno
import fcntl
import os
import pty
import select
import subprocess
import sys
import termios
import time

executable, project_root = sys.argv[1:]
secret = os.environ["CUSTOMER_TEST_HIDDEN_SECRET"]
master, slave = pty.openpty()

def claim_terminal():
    os.setsid()
    fcntl.ioctl(0, termios.TIOCSCTTY, 0)

process = subprocess.Popen(
    [executable, "plugin", "add", "sample@debug"],
    stdin=slave,
    stdout=slave,
    stderr=slave,
    preexec_fn=claim_terminal,
)
os.close(slave)
transcript = bytearray()
questions = [
    (b"Run plugin setup? [y/N]:", b"y\n"),
    (b"Project directory:", os.fsencode(project_root) + b"\n"),
    (b"API key:", secret.encode() + b"\r"),
    (b"Customer verification code:", b"verification-ok\n"),
]
question = 0
search_from = 0
deadline = time.monotonic() + 15

try:
    while time.monotonic() < deadline:
        ready, _, _ = select.select([master], [], [], 0.2)
        if ready:
            try:
                block = os.read(master, 8192)
            except OSError as error:
                if error.errno != errno.EIO:
                    raise
                break
            if not block:
                break
            transcript.extend(block)
            if len(transcript) > 131072:
                raise RuntimeError("interactive setup produced excessive terminal output")
            while (
                question < len(questions)
                and transcript.find(questions[question][0], search_from) != -1
            ):
                search_from = len(transcript)
                os.write(master, questions[question][1])
                question += 1
        if process.poll() is not None and not ready:
            break

    if process.poll() is None:
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            safe_transcript = transcript.decode(errors="replace").replace(secret, "[REDACTED]")
            raise RuntimeError(
                f"interactive plugin setup timed out after answering {question} "
                f"of {len(questions)} prompts: {safe_transcript}"
            )
    if secret.encode() in transcript:
        raise RuntimeError("hidden customer API key was echoed in the terminal")
    if question != len(questions):
        raise RuntimeError(f"interactive setup answered {question} of {len(questions)} prompts")
    if process.returncode != 0:
        raise RuntimeError(
            f"interactive setup exited {process.returncode}: "
            f"{transcript.decode(errors='replace')}"
        )
    sys.stdout.buffer.write(transcript)
finally:
    if process.poll() is None:
        process.kill()
        process.wait()
    os.close(master)
"#;

    let output = std::process::Command::new("python3")
        .args(["-c", pty_driver])
        .arg(&executable)
        .arg(&project_root)
        .env("CODEX_HOME", codex_home.path())
        .env("HOME", codex_home.path())
        .env("CUSTOMER_TEST_HIDDEN_SECRET", secret)
        .env_remove("CUSTOMER_API_KEY")
        .output()?;
    assert!(
        output.status.success(),
        "interactive customer setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let transcript = String::from_utf8_lossy(&output.stdout);
    assert!(transcript.contains("Customer terminal authentication completed."));
    assert!(transcript.contains("Customer terminal handoff verified."));
    assert!(transcript.contains("Added plugin `sample`"));
    assert!(!transcript.contains(secret));

    let state: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        codex_home
            .path()
            .join("plugins/data/setup/debug/sample/setup-state.json"),
    )?)?;
    assert_eq!(state["has_api_key"], json!(true));
    assert_eq!(state["run_count"], json!(1));
    assert_eq!(
        state["steps"],
        json!(["configure project", "verify connection"])
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_add_rejects_interactive_setup_with_redirected_stdout() -> Result<()> {
    let setup = json!({
        "inputs": [{
            "id": "api_key",
            "type": "secret",
            "prompt": "API key",
            "env": "CUSTOMER_API_KEY",
            "required": true
        }],
        "commands": [{
            "name": "interactive customer authentication",
            "command": [
                "python3",
                "-c",
                "import os; from pathlib import Path; secret = os.environ['CUSTOMER_API_KEY']; Path(os.environ['PLUGIN_DATA'], 'unexpected-interactive-setup').write_text(secret); print(secret)"
            ],
            "interactive": true
        }]
    });
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(setup, /*feature_enabled*/ true)?;
    let executable = codex_utils_cargo_bin::cargo_bin("codex")?;
    let secret = "fixture-redirected-interactive-customer-secret";
    let pty_driver = r#"
import errno
import fcntl
import os
import pty
import select
import subprocess
import sys
import termios
import time

executable = sys.argv[1]
secret = os.environ["CUSTOMER_API_KEY"].encode()
master, slave = pty.openpty()

def claim_terminal():
    os.setsid()
    fcntl.ioctl(0, termios.TIOCSCTTY, 0)

process = subprocess.Popen(
    [executable, "plugin", "add", "sample@debug", "--run-setup"],
    stdin=slave,
    stdout=subprocess.PIPE,
    stderr=slave,
    preexec_fn=claim_terminal,
)
os.close(slave)

try:
    stdout = bytearray()
    transcript = bytearray()
    stdout_fd = process.stdout.fileno()
    readable = {master, stdout_fd}
    deadline = time.monotonic() + 30
    while readable and time.monotonic() < deadline:
        ready, _, _ = select.select(list(readable), [], [], 0.2)
        for descriptor in ready:
            try:
                block = os.read(descriptor, 8192)
            except OSError as error:
                if descriptor != master or error.errno != errno.EIO:
                    raise
                readable.remove(descriptor)
                continue
            if not block:
                readable.remove(descriptor)
                continue
            destination = transcript if descriptor == master else stdout
            destination.extend(block)
            if len(destination) > 131072:
                raise RuntimeError("redirected interactive plugin setup emitted excessive output")

    if process.poll() is None:
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            raise RuntimeError("redirected interactive plugin setup did not fail closed")

    if process.returncode == 0:
        raise RuntimeError("redirected interactive plugin setup unexpectedly succeeded")
    if stdout:
        raise RuntimeError("interactive plugin setup wrote to redirected stdout")
    if secret in stdout or secret in transcript:
        raise RuntimeError("interactive plugin setup exposed the customer secret")
    if b"interactive plugin setup steps require a terminal" not in transcript:
        raise RuntimeError(
            f"redirected interactive plugin setup exited {process.returncode} "
            "without explaining the terminal requirement: "
            + transcript.decode(errors="replace")
        )
    sys.stdout.buffer.write(transcript)
finally:
    if process.poll() is None:
        process.kill()
        process.wait()
    os.close(master)
"#;

    let output = std::process::Command::new("python3")
        .args(["-c", pty_driver])
        .arg(&executable)
        .env("CODEX_HOME", codex_home.path())
        .env("HOME", codex_home.path())
        .env("CUSTOMER_API_KEY", secret)
        .output()?;
    assert!(
        output.status.success(),
        "redirected interactive setup regression failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let transcript = String::from_utf8_lossy(&output.stdout);
    assert!(transcript.contains("interactive plugin setup steps require a terminal"));
    assert!(!transcript.contains(secret));

    let config = std::fs::read_to_string(codex_home.path().join(CONFIG_TOML_FILE))?;
    assert!(!config.contains("[plugins.\"sample@debug\"]"));
    assert!(
        !codex_home
            .path()
            .join("plugins/data/setup/debug/sample/unexpected-interactive-setup")
            .exists()
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_add_redacts_secrets_written_by_setup_commands() -> Result<()> {
    let setup = json!({
        "inputs": [{
            "id": "api_key",
            "type": "secret",
            "prompt": "API key",
            "env": "CUSTOMER_API_KEY"
        }],
        "commands": [{
            "name": "verify redaction",
            "command": [
                "python3",
                "-c",
                "import os, sys; secret=os.environ['CUSTOMER_API_KEY']; sys.stdout.write('cross-stream='+secret[:7]); sys.stdout.flush(); sys.stderr.write(secret[7:]+'\\n'); sys.stderr.flush(); sys.stdout.write('same-stream='+secret+'\\n'); sys.stdout.flush()"
            ]
        }]
    });
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(setup, /*feature_enabled*/ true)?;
    let secret = "fixture-secret-must-be-redacted-even-across-output-writes";

    let result = codex_command(codex_home.path())?
        .env("CUSTOMER_API_KEY", secret)
        .args(["plugin", "add", "sample@debug", "--run-setup", "--json"])
        .assert()
        .success();
    let output = result.get_output();
    let json_output: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(json_output["pluginId"], json!("sample@debug"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!stderr.contains(secret));
    assert!(stderr.contains("[REDACTED]"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_add_redacts_overlapping_secrets_at_output_boundaries() -> Result<()> {
    let setup = json!({
        "inputs": [
            {
                "id": "primary_key",
                "type": "secret",
                "prompt": "Primary API key",
                "env": "CUSTOMER_PRIMARY_API_KEY"
            },
            {
                "id": "secondary_key",
                "type": "secret",
                "prompt": "Secondary API key",
                "env": "CUSTOMER_SECONDARY_API_KEY"
            }
        ],
        "commands": [{
            "name": "verify overlapping redaction",
            "command": [
                "python3",
                "-c",
                "import os, sys; first=os.environ['CUSTOMER_PRIMARY_API_KEY']; second=os.environ['CUSTOMER_SECONDARY_API_KEY']; sys.stdout.write(first + second[2:] + 'x' * 6); sys.stdout.flush()"
            ]
        }]
    });
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(setup, /*feature_enabled*/ true)?;
    let primary_secret = "ABCDEFGHIJ";
    let secondary_secret = "IJ123";

    let result = codex_command(codex_home.path())?
        .env("CUSTOMER_PRIMARY_API_KEY", primary_secret)
        .env("CUSTOMER_SECONDARY_API_KEY", secondary_secret)
        .args(["plugin", "add", "sample@debug", "--run-setup", "--json"])
        .assert()
        .success();
    let output = result.get_output();
    let json_output: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(json_output["pluginId"], json!("sample@debug"));
    assert!(!stderr.contains(primary_secret));
    assert!(!stderr.contains(secondary_secret));
    assert!(!stderr.contains("ABCDEFGH"));
    assert!(stderr.contains("[REDACTED]"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_add_bounds_high_volume_output_with_self_overlapping_secrets() -> Result<()> {
    let setup = json!({
        "inputs": [{
            "id": "api_key",
            "type": "secret",
            "prompt": "API key",
            "env": "CUSTOMER_API_KEY"
        }],
        "commands": [{
            "name": "redact repetitive setup output",
            "command": [
                "python3",
                "-c",
                "import sys; sys.stdout.write('a' * (2 * 1024 * 1024)); sys.stdout.flush()"
            ]
        }]
    });
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(setup, /*feature_enabled*/ true)?;
    let started = std::time::Instant::now();

    let result = codex_command(codex_home.path())?
        .env("CUSTOMER_API_KEY", "aa")
        .args(["plugin", "add", "sample@debug", "--run-setup", "--json"])
        .assert()
        .success();
    let output = result.get_output();
    let json_output: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(json_output["pluginId"], json!("sample@debug"));
    assert!(started.elapsed() < std::time::Duration::from_secs(30));
    assert!(!stderr.contains("aa"));
    assert!(stderr.contains("[REDACTED]"));
    assert_eq!(stderr.matches("[plugin setup output truncated]").count(), 1);
    assert!(output.stderr.len() < 6_000);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_add_bounds_setup_output_across_stdout_and_stderr() -> Result<()> {
    let setup = json!({
        "commands": [{
            "name": "exercise bounded output",
            "command": [
                "python3",
                "-c",
                "import sys; sys.stdout.write('a' * 12000); sys.stdout.flush(); sys.stderr.write('b' * 12000); sys.stderr.flush()"
            ]
        }]
    });
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(setup, /*feature_enabled*/ true)?;

    let result = codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug", "--run-setup", "--json"])
        .assert()
        .success();
    let output = result.get_output();
    let json_output: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert_eq!(json_output["pluginId"], json!("sample@debug"));
    assert_eq!(stderr.matches("[plugin setup output truncated]").count(), 1);
    assert!(output.stderr.len() < 6_000);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_add_requires_explicit_setup_approval_without_a_terminal() -> Result<()> {
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(customer_plugin_setup(), /*feature_enabled*/ true)?;

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug"])
        .assert()
        .failure()
        .stderr(contains("explicit consent"));

    let config = std::fs::read_to_string(codex_home.path().join(CONFIG_TOML_FILE))?;
    assert!(!config.contains("[plugins.\"sample@debug\"]"));
    assert!(
        !codex_home
            .path()
            .join("plugins/data/setup/debug/sample/setup-state.json")
            .exists()
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_add_rejects_secret_inputs_from_command_line() -> Result<()> {
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(customer_plugin_setup(), /*feature_enabled*/ true)?;

    codex_command(codex_home.path())?
        .args([
            "plugin",
            "add",
            "sample@debug",
            "--run-setup",
            "--set",
            "api_key=do-not-use-command-line-secrets",
        ])
        .assert()
        .failure()
        .stderr(contains("not --set"));

    let config = std::fs::read_to_string(codex_home.path().join(CONFIG_TOML_FILE))?;
    assert!(!config.contains("[plugins.\"sample@debug\"]"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_add_rejects_missing_required_secret() -> Result<()> {
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(customer_plugin_setup(), /*feature_enabled*/ true)?;
    let project_root = codex_home.path().join("customer-project");
    std::fs::create_dir_all(&project_root)?;

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug", "--run-setup", "--set"])
        .arg(format!("project_root={}", project_root.display()))
        .assert()
        .failure()
        .stderr(contains("CUSTOMER_API_KEY"));

    let config = std::fs::read_to_string(codex_home.path().join(CONFIG_TOML_FILE))?;
    assert!(!config.contains("[plugins.\"sample@debug\"]"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_add_rejects_interactive_setup_in_json_mode() -> Result<()> {
    let mut setup = customer_plugin_setup();
    setup["commands"][0]["interactive"] = json!(true);
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(setup, /*feature_enabled*/ true)?;

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug", "--run-setup", "--json"])
        .assert()
        .failure()
        .stderr(contains("cannot run in JSON"));

    let config = std::fs::read_to_string(codex_home.path().join(CONFIG_TOML_FILE))?;
    assert!(!config.contains("[plugins.\"sample@debug\"]"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_setup_reruns_installed_setup_and_returns_clean_json() -> Result<()> {
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(customer_plugin_setup(), /*feature_enabled*/ true)?;
    let project_root = codex_home.path().join("customer-project");
    std::fs::create_dir_all(&project_root)?;
    let project_arg = format!("project_root={}", project_root.display());

    codex_command(codex_home.path())?
        .env("CUSTOMER_API_KEY", "first-fixture-secret")
        .args(["plugin", "add", "sample@debug", "--run-setup", "--set"])
        .arg(&project_arg)
        .assert()
        .success();

    let assert = codex_command(codex_home.path())?
        .env("CUSTOMER_API_KEY", "second-fixture-secret")
        .args([
            "plugin",
            "setup",
            "sample@debug",
            "--yes",
            "--json",
            "--set",
        ])
        .arg(&project_arg)
        .assert()
        .success();
    let actual: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout)?;
    assert_eq!(
        actual,
        json!({
            "pluginId": "sample@debug",
            "name": "sample",
            "marketplaceName": "debug",
            "status": "completed",
        })
    );

    let state_path = codex_home
        .path()
        .join("plugins/data/setup/debug/sample/setup-state.json");
    let state: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(state_path)?)?;
    assert_eq!(state["run_count"], json!(2));
    assert_eq!(
        state["steps"],
        json!(["configure project", "verify connection"])
    );
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn plugin_setup_rejects_package_mutation_before_activation() -> Result<()> {
    let setup = json!({
        "commands": [{
            "name": "modify package",
            "command": [
                "python3",
                "-c",
                "import os; from pathlib import Path; Path(os.environ['PLUGIN_ROOT'], 'tampered.txt').write_text('modified')"
            ]
        }]
    });
    let (codex_home, _source) =
        setup_customer_plugin_marketplace(setup, /*feature_enabled*/ true)?;

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug", "--run-setup"])
        .assert()
        .failure()
        .stderr(contains("PLUGIN_ROOT"));

    let config = std::fs::read_to_string(codex_home.path().join(CONFIG_TOML_FILE))?;
    assert!(!config.contains("[plugins.\"sample@debug\"]"));
    Ok(())
}

#[tokio::test]
async fn plugin_remove_json_prints_remove_outcome() -> Result<()> {
    let (codex_home, _source) = setup_local_marketplace()?;

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug"])
        .assert()
        .success();

    let assert = codex_command(codex_home.path())?
        .args([
            "plugin",
            "remove",
            "sample",
            "--marketplace",
            "debug",
            "--json",
        ])
        .assert()
        .success();
    let stdout = assert.get_output().stdout.as_slice();
    let actual: serde_json::Value = serde_json::from_slice(stdout)?;

    assert_eq!(
        actual,
        json!({
            "pluginId": "sample@debug",
            "name": "sample",
            "marketplaceName": "debug",
        })
    );

    Ok(())
}

#[tokio::test]
async fn plugin_add_rejects_unconfigured_repo_local_marketplaces() -> Result<()> {
    let (codex_home, source) = setup_unconfigured_local_marketplace()?;

    codex_command_in(codex_home.path(), source.path())?
        .args(["plugin", "add", "sample@debug"])
        .assert()
        .failure()
        .stderr(contains(
            "plugin `sample` was not found in marketplace `debug`",
        ));

    Ok(())
}

#[tokio::test]
async fn plugin_add_fails_when_configured_marketplace_snapshot_is_malformed() -> Result<()> {
    let (codex_home, source) = setup_configured_marketplace_with_malformed_manifest()?;

    assert_configured_marketplace_snapshot_failure(
        codex_command(codex_home.path())?
            .args(["plugin", "add", "sample@debug"])
            .assert(),
        source.path(),
        "key must be a string",
    );

    Ok(())
}

#[tokio::test]
async fn plugin_add_reinstalls_from_configured_marketplace_snapshot() -> Result<()> {
    let (codex_home, _source) = setup_local_marketplace()?;

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug"])
        .assert()
        .success();

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug"])
        .assert()
        .success()
        .stdout(contains("Added plugin `sample` from marketplace `debug`."));

    assert!(
        codex_home
            .path()
            .join("plugins/cache/debug/sample/1.2.3/.codex-plugin/plugin.json")
            .is_file()
    );

    Ok(())
}

#[tokio::test]
async fn plugin_remove_works_after_marketplace_is_removed() -> Result<()> {
    let (codex_home, _source) = setup_local_marketplace()?;

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample", "--marketplace", "debug"])
        .assert()
        .success();

    codex_command(codex_home.path())?
        .args(["plugin", "marketplace", "remove", "debug"])
        .assert()
        .success();

    codex_command(codex_home.path())?
        .args(["plugin", "remove", "sample@debug"])
        .assert()
        .success()
        .stdout(contains(
            "Removed plugin `sample` from marketplace `debug`.",
        ));

    let config = std::fs::read_to_string(codex_home.path().join(CONFIG_TOML_FILE))?;
    assert!(!config.contains("[plugins.\"sample@debug\"]"));

    Ok(())
}

#[tokio::test]
async fn plugin_add_rejects_cached_plugins_without_authorizing_marketplace_snapshot() -> Result<()>
{
    let (codex_home, _source) = setup_local_marketplace()?;

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug"])
        .assert()
        .success();

    codex_command(codex_home.path())?
        .args(["plugin", "marketplace", "remove", "debug"])
        .assert()
        .success();

    assert!(
        codex_home
            .path()
            .join("plugins/cache/debug/sample/1.2.3/.codex-plugin/plugin.json")
            .is_file()
    );

    codex_command(codex_home.path())?
        .args(["plugin", "add", "sample@debug"])
        .assert()
        .failure()
        .stderr(contains(
            "plugin `sample` was not found in marketplace `debug`",
        ));

    Ok(())
}
