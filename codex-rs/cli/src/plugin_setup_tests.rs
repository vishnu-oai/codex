use super::*;
use codex_utils_absolute_path::AbsolutePathBuf;
use pretty_assertions::assert_eq;
#[cfg(unix)]
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use tempfile::tempdir;

#[test]
fn approval_policy_requires_explicit_flag_for_headless_and_json_modes() {
    assert_eq!(
        setup_approval(
            /*run_setup*/ true, /*json*/ true, /*stdin_is_terminal*/ false,
            /*stderr_is_terminal*/ false,
        )
        .unwrap(),
        SetupApproval::Approved
    );
    assert_eq!(
        setup_approval(
            /*run_setup*/ false, /*json*/ false, /*stdin_is_terminal*/ true,
            /*stderr_is_terminal*/ true,
        )
        .unwrap(),
        SetupApproval::Prompt
    );
    for (json, stdin_is_terminal, stderr_is_terminal) in [
        (true, true, true),
        (false, false, true),
        (false, true, false),
    ] {
        let err = setup_approval(
            /*run_setup*/ false,
            json,
            stdin_is_terminal,
            stderr_is_terminal,
        )
        .unwrap_err();
        assert!(err.to_string().contains("--run-setup"));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn setup_command_runs_from_final_root_with_plugin_environment() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("plugin");
    let data = temp.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    let command = helper_command();
    let invocation = setup_invocation(&root, &data, command.clone());

    run_plugin_setup_with_timeout(&invocation, Duration::from_secs(5))
        .await
        .unwrap();

    let observation: serde_json::Value =
        serde_json::from_slice(&std::fs::read(data.join("setup-observation.json")).unwrap())
            .unwrap();
    assert_eq!(
        observation,
        serde_json::json!({
            "cwd": std::fs::canonicalize(&root).unwrap(),
            "pluginRoot": root,
            "pluginData": data,
            "args": &command[1..],
        })
    );
}

#[cfg(unix)]
#[tokio::test]
async fn timed_out_setup_is_killed_and_cannot_write_a_delayed_sentinel() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("plugin");
    let data = temp.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(root.join("setup-helper-mode"), "delayed-sentinel").unwrap();
    let invocation = setup_invocation(&root, &data, helper_command());

    let err = run_plugin_setup_with_timeout(&invocation, Duration::from_millis(100))
        .await
        .unwrap_err();

    assert!(err.to_string().contains("timed out"));
    std::thread::sleep(Duration::from_millis(650));
    assert!(!data.join("late-sentinel").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn high_volume_output_is_drained_without_blocking_the_child() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("plugin");
    let data = temp.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(root.join("setup-helper-mode"), "high-volume").unwrap();
    let invocation = setup_invocation(&root, &data, helper_command());

    run_plugin_setup_with_timeout(&invocation, Duration::from_secs(5))
        .await
        .unwrap();

    assert!(data.join("setup-observation.json").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn inherited_output_pipe_cannot_hold_cli_open_indefinitely() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("plugin");
    let data = temp.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(root.join("setup-helper-mode"), "lingering-output").unwrap();
    let invocation = setup_invocation(&root, &data, helper_command());
    let started = std::time::Instant::now();

    let err = run_plugin_setup_with_timeouts(
        &invocation,
        Duration::from_secs(5),
        Duration::from_millis(100),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("output streams did not close"));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(data.join("setup-observation.json").exists());
    std::thread::sleep(Duration::from_millis(650));
    assert!(!data.join("lingering-sentinel").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn timed_out_setup_kills_descendants_in_its_process_tree() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("plugin");
    let data = temp.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(root.join("setup-helper-mode"), "timeout-with-descendant").unwrap();
    let invocation = setup_invocation(&root, &data, helper_command());

    let err = run_plugin_setup_with_timeouts(
        &invocation,
        Duration::from_millis(100),
        Duration::from_millis(500),
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("timed out"));
    std::thread::sleep(Duration::from_millis(650));
    assert!(!data.join("lingering-sentinel").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn completed_setup_kills_descendants_before_returning_success() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("plugin");
    let data = temp.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(root.join("setup-helper-mode"), "detached-descendant").unwrap();
    let invocation = setup_invocation(&root, &data, helper_command());

    run_plugin_setup_with_timeout(&invocation, Duration::from_secs(5))
        .await
        .unwrap();

    assert!(data.join("setup-observation.json").exists());
    std::thread::sleep(Duration::from_millis(650));
    assert!(!data.join("lingering-sentinel").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn interrupted_setup_kills_and_reaps_its_process_tree() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("plugin");
    let data = temp.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(root.join("setup-helper-mode"), "timeout-with-descendant").unwrap();
    let invocation = setup_invocation(&root, &data, helper_command());
    let interrupt = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        Ok::<(), std::io::Error>(())
    };

    let err = run_plugin_setup_with_interrupt(
        &invocation,
        Duration::from_secs(5),
        Duration::from_millis(500),
        interrupt,
    )
    .await
    .unwrap_err();

    assert!(err.to_string().contains("interrupted"));
    std::thread::sleep(Duration::from_millis(650));
    assert!(!data.join("lingering-sentinel").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn relative_executable_and_literal_arguments_are_preserved() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("plugin");
    let data = temp.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    let shell_sentinel = temp.path().join("shell-expanded");
    let literal_argument = format!(
        "literal value with spaces; touch {} && $(echo not-a-shell) & *",
        shell_sentinel.display()
    );
    let command = copied_relative_helper_command(&root, &literal_argument);
    let invocation = setup_invocation(&root, &data, command.clone());

    run_plugin_setup_with_timeout(&invocation, Duration::from_secs(5))
        .await
        .unwrap();

    let observation: serde_json::Value =
        serde_json::from_slice(&std::fs::read(data.join("setup-observation.json")).unwrap())
            .unwrap();
    assert_eq!(
        observation,
        serde_json::json!({
            "cwd": std::fs::canonicalize(&root).unwrap(),
            "pluginRoot": root,
            "pluginData": data,
            "args": &command[1..],
        })
    );
    assert!(command.contains(&literal_argument));
    assert!(!shell_sentinel.exists());
}

#[test]
fn executable_resolution_uses_path_only_for_bare_names() {
    let temp = tempdir().unwrap();
    let root = temp.path();
    let relative = PathBuf::from("scripts").join("setup-helper");
    std::fs::create_dir_all(root.join("scripts")).unwrap();
    std::fs::write(root.join(&relative), "setup helper").unwrap();
    let absolute = std::env::current_exe().unwrap();

    assert_eq!(
        resolve_setup_executable(root, "setup-helper").unwrap(),
        PathBuf::from("setup-helper")
    );
    assert_eq!(
        resolve_setup_executable(root, &relative.to_string_lossy()).unwrap(),
        std::fs::canonicalize(root.join(relative)).unwrap()
    );
    assert_eq!(
        resolve_setup_executable(root, &absolute.to_string_lossy()).unwrap(),
        absolute
    );
}

#[cfg(not(unix))]
#[tokio::test]
async fn setup_execution_fails_closed_without_process_group_containment() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("plugin");
    let data = temp.path().join("data");
    std::fs::create_dir_all(&root).unwrap();
    let invocation = setup_invocation(&root, &data, vec!["must-not-run".to_string()]);

    let err = run_plugin_setup_with_timeout(&invocation, Duration::from_secs(5))
        .await
        .unwrap_err();

    assert!(err.to_string().contains("supported only on Unix"));
    assert!(!data.exists());
}

fn setup_invocation(root: &Path, data: &Path, command: Vec<String>) -> SetupInvocation {
    SetupInvocation {
        installed_path: AbsolutePathBuf::try_from(root.to_path_buf()).unwrap(),
        data_path: AbsolutePathBuf::try_from(data.to_path_buf()).unwrap(),
        command,
        interactive: false,
        environment: HashMap::new(),
        output_policy: SetupOutputPolicy::new(&[], &HashMap::new()),
    }
}

#[cfg(unix)]
fn helper_command() -> Vec<String> {
    vec![
        std::env::current_exe().unwrap().display().to_string(),
        "--ignored".to_string(),
        "--exact".to_string(),
        "plugin_setup::tests::plugin_setup_test_helper".to_string(),
        "--nocapture".to_string(),
    ]
}

#[cfg(unix)]
fn copied_relative_helper_command(root: &Path, literal_argument: &str) -> Vec<String> {
    let source = std::env::current_exe().unwrap();
    let relative = PathBuf::from("scripts").join(source.file_name().unwrap());
    let destination = root.join(&relative);
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::fs::copy(source, destination).unwrap();
    vec![
        relative.to_string_lossy().into_owned(),
        "--ignored".to_string(),
        "--exact".to_string(),
        "plugin_setup::tests::plugin_setup_test_helper".to_string(),
        "--nocapture".to_string(),
        "--skip".to_string(),
        literal_argument.to_string(),
    ]
}

#[cfg(unix)]
#[test]
#[ignore]
fn plugin_setup_test_helper() {
    let root = std::env::var_os("PLUGIN_ROOT")
        .map(std::path::PathBuf::from)
        .unwrap();
    let data = std::env::var_os("PLUGIN_DATA")
        .map(std::path::PathBuf::from)
        .unwrap();
    let mode = std::fs::read_to_string(root.join("setup-helper-mode")).unwrap_or_default();
    match mode.as_str() {
        "delayed-sentinel" => {
            std::thread::sleep(Duration::from_millis(500));
            std::fs::create_dir_all(&data).unwrap();
            std::fs::write(data.join("late-sentinel"), "setup child survived timeout").unwrap();
            return;
        }
        "high-volume" => {
            const CHUNK: [u8; 8 * 1024] = [b'x'; 8 * 1024];
            let mut stdout = std::io::stdout().lock();
            let mut stderr = std::io::stderr().lock();
            for _ in 0..128 {
                stdout.write_all(&CHUNK).unwrap();
                stderr.write_all(&CHUNK).unwrap();
            }
            stdout.flush().unwrap();
            stderr.flush().unwrap();
        }
        "lingering-output" => {
            spawn_lingering_descendant(/*inherit_output*/ true);
        }
        "timeout-with-descendant" => {
            spawn_lingering_descendant(/*inherit_output*/ false);
            std::thread::sleep(Duration::from_secs(5));
        }
        "detached-descendant" => {
            spawn_lingering_descendant(/*inherit_output*/ false);
        }
        _ => {}
    }
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("setup-observation.json"),
        serde_json::to_vec(&serde_json::json!({
            "cwd": std::env::current_dir().unwrap(),
            "pluginRoot": root,
            "pluginData": data,
            "args": std::env::args().skip(1).collect::<Vec<_>>(),
        }))
        .unwrap(),
    )
    .unwrap();
}

#[cfg(unix)]
fn spawn_lingering_descendant(inherit_output: bool) {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args([
        "--ignored",
        "--exact",
        "plugin_setup::tests::plugin_setup_lingering_descendant",
        "--nocapture",
    ]);
    if !inherit_output {
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
    }
    let child = command.spawn().unwrap();
    // The helper must deliberately exit or be killed without reaping this descendant so the
    // production process-tree cleanup and bounded pipe drain are exercised.
    std::mem::forget(child);
}

#[cfg(unix)]
#[test]
#[ignore]
fn plugin_setup_lingering_descendant() {
    let data = std::env::var_os("PLUGIN_DATA")
        .map(std::path::PathBuf::from)
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(
        data.join("lingering-sentinel"),
        "setup descendant escaped cleanup",
    )
    .unwrap();
}

#[test]
fn setup_inputs_accept_explicit_existing_directory() {
    let temp = tempdir().unwrap();
    let project = temp.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let inputs = vec![PluginManifestSetupInput {
        id: "project_root".to_string(),
        input_type: PluginManifestSetupInputType::Directory,
        prompt: "Project directory".to_string(),
        env: "CODEX_SETUP_TEST_PROJECT_ROOT".to_string(),
        required: true,
    }];
    let values = collect_setup_inputs(
        &inputs,
        &[format!("project_root={}", project.display())],
        /*json*/ true,
        /*stdin_is_terminal*/ false,
        /*stderr_is_terminal*/ false,
    )
    .unwrap();

    assert_eq!(
        values,
        HashMap::from([(
            "CODEX_SETUP_TEST_PROJECT_ROOT".to_string(),
            std::fs::canonicalize(project)
                .unwrap()
                .to_str()
                .unwrap()
                .to_string(),
        )])
    );
}

#[test]
fn setup_inputs_reject_command_line_secrets() {
    let inputs = vec![PluginManifestSetupInput {
        id: "api_key".to_string(),
        input_type: PluginManifestSetupInputType::Secret,
        prompt: "API key".to_string(),
        env: "CODEX_SETUP_TEST_API_KEY".to_string(),
        required: true,
    }];
    let error = collect_setup_inputs(
        &inputs,
        &["api_key=must-not-appear-in-argv".to_string()],
        /*json*/ true,
        /*stdin_is_terminal*/ false,
        /*stderr_is_terminal*/ false,
    )
    .unwrap_err();

    assert!(error.to_string().contains("not --set"));
}

#[test]
fn setup_inputs_reject_unknown_or_duplicate_assignments() {
    let inputs = vec![PluginManifestSetupInput {
        id: "project".to_string(),
        input_type: PluginManifestSetupInputType::Text,
        prompt: "Project".to_string(),
        env: "CODEX_SETUP_TEST_PROJECT".to_string(),
        required: true,
    }];
    let unknown = collect_setup_inputs(
        &inputs,
        &["unexpected=value".to_string()],
        /*json*/ true,
        /*stdin_is_terminal*/ false,
        /*stderr_is_terminal*/ false,
    )
    .unwrap_err();
    assert!(unknown.to_string().contains("unknown"));

    let duplicate = collect_setup_inputs(
        &inputs,
        &["project=first".to_string(), "project=second".to_string()],
        /*json*/ true,
        /*stdin_is_terminal*/ false,
        /*stderr_is_terminal*/ false,
    )
    .unwrap_err();
    assert!(duplicate.to_string().contains("more than once"));
}

#[test]
fn setup_inputs_reject_incorrect_path_types() {
    let temp = tempdir().unwrap();
    let file = temp.path().join("config.yaml");
    std::fs::write(&file, "enabled: true").unwrap();
    let inputs = vec![PluginManifestSetupInput {
        id: "project".to_string(),
        input_type: PluginManifestSetupInputType::Directory,
        prompt: "Project".to_string(),
        env: "CODEX_SETUP_TEST_DIRECTORY".to_string(),
        required: true,
    }];
    let error = collect_setup_inputs(
        &inputs,
        &[format!("project={}", file.display())],
        /*json*/ true,
        /*stdin_is_terminal*/ false,
        /*stderr_is_terminal*/ false,
    )
    .unwrap_err();

    assert!(error.to_string().contains("directory"));
}

#[test]
fn setup_executable_cannot_escape_plugin_root() {
    let temp = tempdir().unwrap();
    let root = temp.path().join("plugin");
    std::fs::create_dir_all(&root).unwrap();
    let outside = temp.path().join("outside");
    std::fs::write(outside, "not part of plugin").unwrap();

    let error = resolve_setup_executable(&root, "../outside").unwrap_err();

    assert!(error.to_string().contains("inside PLUGIN_ROOT"));
}
