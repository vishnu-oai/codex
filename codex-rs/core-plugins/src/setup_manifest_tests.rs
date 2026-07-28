use super::PluginManifest;
use super::parse_plugin_manifest;
use super::setup_manifest::MAX_SETUP_COMMAND_ARGUMENT_COUNT;
use super::setup_manifest::MAX_SETUP_COMMAND_COUNT;
use super::setup_manifest::MAX_SETUP_INPUT_COUNT;
use codex_plugin::manifest::PluginManifestSetup;
use codex_plugin::manifest::PluginManifestSetupCommand;
use codex_plugin::manifest::PluginManifestSetupInput;
use codex_plugin::manifest::PluginManifestSetupInputType;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::tempdir;

fn parse_setup_manifest(contents: &str) -> Result<PluginManifest, serde_json::Error> {
    let root = tempdir().expect("create plugin root");
    parse_plugin_manifest(
        root.path(),
        &root.path().join(".codex-plugin/plugin.json"),
        contents,
    )
}

#[test]
fn setup_manifest_parses_typed_inputs_and_ordered_commands() {
    let manifest = parse_setup_manifest(
        r#"{
            "name": "customer-tools",
            "setup": {
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
                        "env": "CUSTOMER_API_KEY",
                        "required": false
                    }
                ],
                "commands": [
                    {
                        "name": "authenticate",
                        "command": ["python3", "./scripts/auth.py"],
                        "interactive": true
                    },
                    {
                        "name": "verify",
                        "command": ["python3", "./scripts/verify.py"]
                    }
                ]
            }
        }"#,
    )
    .expect("parse setup manifest");

    assert_eq!(
        manifest.setup,
        Some(PluginManifestSetup {
            inputs: vec![
                PluginManifestSetupInput {
                    id: "project_root".to_string(),
                    input_type: PluginManifestSetupInputType::Directory,
                    prompt: "Project directory".to_string(),
                    env: "CUSTOMER_PROJECT_ROOT".to_string(),
                    required: true,
                },
                PluginManifestSetupInput {
                    id: "api_key".to_string(),
                    input_type: PluginManifestSetupInputType::Secret,
                    prompt: "API key".to_string(),
                    env: "CUSTOMER_API_KEY".to_string(),
                    required: false,
                },
            ],
            commands: vec![
                PluginManifestSetupCommand {
                    name: "authenticate".to_string(),
                    command: vec!["python3".to_string(), "./scripts/auth.py".to_string()],
                    interactive: true,
                },
                PluginManifestSetupCommand {
                    name: "verify".to_string(),
                    command: vec!["python3".to_string(), "./scripts/verify.py".to_string()],
                    interactive: false,
                },
            ],
        })
    );
}

#[test]
fn setup_manifest_accepts_legacy_single_command() {
    let manifest = parse_setup_manifest(
        r#"{
            "name": "customer-tools",
            "setup": {"command": ["python3", "./scripts/setup.py"]}
        }"#,
    )
    .expect("parse legacy setup");

    assert_eq!(
        manifest.setup,
        Some(PluginManifestSetup {
            inputs: Vec::new(),
            commands: vec![PluginManifestSetupCommand {
                name: "setup".to_string(),
                command: vec!["python3".to_string(), "./scripts/setup.py".to_string()],
                interactive: false,
            }],
        })
    );
}

#[test]
fn setup_manifest_rejects_ambiguous_command_shapes() {
    let error = parse_setup_manifest(
        r#"{
            "name": "customer-tools",
            "setup": {
                "command": ["python3", "./scripts/old.py"],
                "commands": [
                    {"name": "setup", "command": ["python3", "./scripts/new.py"]}
                ]
            }
        }"#,
    )
    .expect_err("reject both setup command forms");

    assert!(error.to_string().contains("not both"));
}

#[test]
fn setup_manifest_rejects_reserved_environment_variables() {
    for reserved in [
        "PLUGIN_ROOT",
        "PLUGIN_DATA",
        "CLAUDE_PLUGIN_ROOT",
        "CLAUDE_PLUGIN_DATA",
    ] {
        let contents = json!({
            "name": "customer-tools",
            "setup": {
                "inputs": [{
                    "id": "override",
                    "type": "text",
                    "prompt": "Override",
                    "env": reserved
                }],
                "commands": [{"name": "setup", "command": ["python3"]}]
            }
        });
        let error = parse_setup_manifest(&contents.to_string())
            .expect_err("reject reserved setup environment variable");
        assert!(error.to_string().contains(reserved));
    }
}

#[test]
fn setup_manifest_rejects_duplicate_input_ids() {
    let error = parse_setup_manifest(
        r#"{
            "name": "customer-tools",
            "setup": {
                "inputs": [
                    {"id": "project", "type": "text", "prompt": "First", "env": "FIRST"},
                    {"id": "project", "type": "text", "prompt": "Second", "env": "SECOND"}
                ],
                "commands": [{"name": "setup", "command": ["python3"]}]
            }
        }"#,
    )
    .expect_err("reject duplicate setup input ID");
    assert!(error.to_string().contains("more than once"));
}

#[test]
fn setup_manifest_rejects_duplicate_environment_variables() {
    let error = parse_setup_manifest(
        r#"{
            "name": "customer-tools",
            "setup": {
                "inputs": [
                    {"id": "first", "type": "text", "prompt": "First", "env": "SHARED"},
                    {"id": "second", "type": "text", "prompt": "Second", "env": "SHARED"}
                ],
                "commands": [{"name": "setup", "command": ["python3"]}]
            }
        }"#,
    )
    .expect_err("reject duplicate setup environment variable");
    assert!(error.to_string().contains("more than once"));
}

#[test]
fn setup_manifest_rejects_duplicate_command_names() {
    let error = parse_setup_manifest(
        r#"{
            "name": "customer-tools",
            "setup": {
                "commands": [
                    {"name": "setup", "command": ["python3"]},
                    {"name": "setup", "command": ["python3"]}
                ]
            }
        }"#,
    )
    .expect_err("reject duplicate setup command");
    assert!(error.to_string().contains("more than once"));
}

#[test]
fn setup_manifest_rejects_terminal_control_sequences() {
    let hostile_prompt = json!({
        "name": "customer-tools",
        "setup": {
            "inputs": [{
                "id": "project",
                "type": "text",
                "prompt": "Project\u{1b}[2J",
                "env": "CUSTOMER_PROJECT"
            }],
            "commands": [{"name": "setup", "command": ["python3"]}]
        }
    });
    assert!(
        parse_setup_manifest(&hostile_prompt.to_string())
            .expect_err("reject terminal control sequence in setup prompt")
            .to_string()
            .contains("prompt")
    );

    let hostile_command_name = json!({
        "name": "customer-tools",
        "setup": {
            "commands": [{"name": "setup\u{1b}[2J", "command": ["python3"]}]
        }
    });
    assert!(
        parse_setup_manifest(&hostile_command_name.to_string())
            .expect_err("reject terminal control sequence in setup command name")
            .to_string()
            .contains("command names")
    );

    let hostile_argument = json!({
        "name": "customer-tools",
        "setup": {
            "commands": [{"name": "setup", "command": ["python3", "unsafe\u{1b}[2J"]}]
        }
    });
    assert!(
        parse_setup_manifest(&hostile_argument.to_string())
            .expect_err("reject terminal control sequence in setup command argument")
            .to_string()
            .contains("arguments")
    );
}

#[test]
fn setup_manifest_rejects_unbounded_rendered_approval_plan() {
    let commands = (0..3)
        .map(|index| {
            json!({
                "name": format!("step-{index}"),
                "command": ["python3", "x".repeat(1_500)]
            })
        })
        .collect::<Vec<_>>();
    let manifest = json!({
        "name": "customer-tools",
        "setup": {"commands": commands}
    });

    assert!(
        parse_setup_manifest(&manifest.to_string())
            .expect_err("reject an unbounded user-visible setup approval plan")
            .to_string()
            .contains("command plan")
    );
}

#[test]
fn setup_manifest_rejects_unbounded_commands_inputs_and_arguments() {
    let too_many_commands = (0..=MAX_SETUP_COMMAND_COUNT)
        .map(|index| json!({"name": format!("step-{index}"), "command": ["python3"]}))
        .collect::<Vec<_>>();
    let too_many_commands_manifest = json!({
        "name": "customer-tools",
        "setup": {"commands": too_many_commands}
    });
    let error = parse_setup_manifest(&too_many_commands_manifest.to_string())
        .expect_err("reject unbounded setup steps");
    assert!(error.to_string().contains("commands"));

    let too_many_inputs = (0..=MAX_SETUP_INPUT_COUNT)
        .map(|index| {
            json!({
                "id": format!("input_{index}"),
                "type": "text",
                "prompt": "Value",
                "env": format!("INPUT_{index}")
            })
        })
        .collect::<Vec<_>>();
    let too_many_inputs_manifest = json!({
        "name": "customer-tools",
        "setup": {
            "inputs": too_many_inputs,
            "commands": [{"name": "setup", "command": ["python3"]}]
        }
    });
    let error = parse_setup_manifest(&too_many_inputs_manifest.to_string())
        .expect_err("reject unbounded setup inputs");
    assert!(error.to_string().contains("inputs"));

    let too_many_arguments = vec!["argument"; MAX_SETUP_COMMAND_ARGUMENT_COUNT + 1];
    let too_many_arguments_manifest = json!({
        "name": "customer-tools",
        "setup": {
            "commands": [{"name": "setup", "command": too_many_arguments}]
        }
    });
    let error = parse_setup_manifest(&too_many_arguments_manifest.to_string())
        .expect_err("reject unbounded setup arguments");
    assert!(error.to_string().contains("arguments"));
}

#[test]
fn setup_manifest_rejects_unknown_fields_and_missing_commands() {
    let unknown = parse_setup_manifest(
        r#"{
            "name": "customer-tools",
            "setup": {
                "commands": [{"name": "setup", "command": ["python3"]}],
                "unexpected": true
            }
        }"#,
    )
    .expect_err("reject unknown setup field");
    assert!(unknown.to_string().contains("unknown field"));

    let missing = parse_setup_manifest(r#"{"name":"customer-tools","setup":{}}"#)
        .expect_err("reject setup without commands");
    assert!(missing.to_string().contains("at least one command"));
}
