use super::*;
use pretty_assertions::assert_eq;
use pretty_assertions::assert_ne;
use std::sync::mpsc;
use std::time::Duration;
use tempfile::tempdir;

#[test]
fn package_fingerprint_changes_with_file_bytes() {
    let temp = tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join("scripts")).unwrap();
    std::fs::write(temp.path().join("scripts/setup.js"), "first").unwrap();
    let first = plugin_content_fingerprint(temp.path()).unwrap();

    std::fs::write(temp.path().join("scripts/setup.js"), "second").unwrap();
    let second = plugin_content_fingerprint(temp.path()).unwrap();

    assert_ne!(first, second);
}

#[test]
fn setup_cache_version_is_the_finalized_package_fingerprint_in_a_reserved_namespace() {
    let temp = tempdir().unwrap();
    std::fs::write(temp.path().join("setup.js"), "first").unwrap();
    let first_fingerprint = plugin_content_fingerprint(temp.path()).unwrap();
    let first = setup_cache_version(temp.path()).unwrap();

    std::fs::write(temp.path().join("setup.js"), "second").unwrap();
    let changed_source = setup_cache_version(temp.path()).unwrap();

    assert_eq!(
        first,
        format!("{SETUP_CACHE_VERSION_PREFIX}{first_fingerprint}")
    );
    assert_ne!(first, changed_source);
}

#[test]
fn setup_completion_marker_matches_content_without_changing_fingerprint() {
    let temp = tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join(".codex-plugin")).unwrap();
    std::fs::write(
        temp.path().join(".codex-plugin/plugin.json"),
        r#"{"name":"sample","setup":{"command":["node","setup.js"]}}"#,
    )
    .unwrap();
    std::fs::write(temp.path().join("setup.js"), "first").unwrap();
    let fingerprint = plugin_content_fingerprint(temp.path()).unwrap();

    write_setup_completion_marker(temp.path(), &fingerprint).unwrap();

    assert!(setup_completion_marker_path(temp.path()).is_file());
    assert_eq!(
        plugin_content_fingerprint(temp.path()).unwrap(),
        fingerprint
    );
    validate_setup_completion_marker(temp.path()).unwrap();

    std::fs::write(temp.path().join("setup.js"), "second").unwrap();
    let err = validate_setup_completion_marker(temp.path()).unwrap_err();
    assert!(err.to_string().contains("does not match"));
}

#[test]
fn preparing_marker_path_stabilizes_fingerprint_for_alternate_manifest_layouts() {
    let temp = tempdir().unwrap();
    std::fs::create_dir_all(temp.path().join(".claude-plugin")).unwrap();
    std::fs::write(
        temp.path().join(".claude-plugin/plugin.json"),
        r#"{"name":"sample","setup":{"command":["node","setup.js"]}}"#,
    )
    .unwrap();

    prepare_setup_completion_marker(temp.path()).unwrap();
    let fingerprint = plugin_content_fingerprint(temp.path()).unwrap();
    write_setup_completion_marker(temp.path(), &fingerprint).unwrap();

    assert_eq!(
        plugin_content_fingerprint(temp.path()).unwrap(),
        fingerprint
    );
    validate_setup_completion_marker(temp.path()).unwrap();
}

#[test]
fn foreground_install_lock_serializes_same_plugin() {
    let temp = tempdir().unwrap();
    let plugin_id = PluginId::new("sample".to_string(), "debug".to_string()).unwrap();
    let first = ForegroundPluginInstallLock::acquire(temp.path(), &plugin_id).unwrap();
    let (acquired_tx, acquired_rx) = mpsc::channel();
    let home = temp.path().to_path_buf();
    let waiter = std::thread::spawn(move || {
        let second = ForegroundPluginInstallLock::acquire(&home, &plugin_id).unwrap();
        acquired_tx.send(()).unwrap();
        drop(second);
    });

    assert!(acquired_rx.recv_timeout(Duration::from_millis(50)).is_err());
    drop(first);
    acquired_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    waiter.join().unwrap();
}
