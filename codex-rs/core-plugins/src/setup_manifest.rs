use super::compatibility_json_error;
use codex_plugin::manifest::PluginManifestSetup;
use codex_plugin::manifest::PluginManifestSetupCommand;
use codex_plugin::manifest::PluginManifestSetupInput;
use codex_plugin::manifest::PluginManifestSetupInputType;
use serde::Deserialize;
use std::collections::HashSet;

pub(super) const MAX_SETUP_COMMAND_COUNT: usize = 32;
pub(super) const MAX_SETUP_INPUT_COUNT: usize = 32;
pub(super) const MAX_SETUP_COMMAND_ARGUMENT_COUNT: usize = 64;
const MAX_SETUP_INPUT_ID_BYTES: usize = 64;
const MAX_SETUP_ENV_BYTES: usize = 128;
const MAX_SETUP_PROMPT_BYTES: usize = 512;
const MAX_SETUP_COMMAND_NAME_BYTES: usize = 128;
const MAX_SETUP_COMMAND_ARGUMENT_BYTES: usize = 2048;
const MAX_SETUP_RENDERED_PLAN_BYTES: usize = 4096;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RawPluginManifestSetup {
    #[serde(default)]
    inputs: Vec<RawPluginManifestSetupInput>,
    #[serde(default)]
    commands: Vec<RawPluginManifestSetupCommand>,
    #[serde(default)]
    command: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawPluginManifestSetupInput {
    id: String,
    #[serde(rename = "type")]
    input_type: RawPluginManifestSetupInputType,
    prompt: String,
    env: String,
    #[serde(default = "default_setup_input_required")]
    required: bool,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawPluginManifestSetupInputType {
    Text,
    Directory,
    File,
    Secret,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawPluginManifestSetupCommand {
    name: String,
    command: Vec<String>,
    #[serde(default)]
    interactive: bool,
}

const fn default_setup_input_required() -> bool {
    true
}

pub(super) fn resolve_plugin_setup(
    raw: RawPluginManifestSetup,
) -> Result<PluginManifestSetup, serde_json::Error> {
    let RawPluginManifestSetup {
        inputs,
        mut commands,
        command,
    } = raw;
    if let Some(command) = command {
        if !commands.is_empty() {
            return Err(compatibility_json_error(
                "plugin setup must declare either `command` or `commands`, not both",
            ));
        }
        commands.push(RawPluginManifestSetupCommand {
            name: "setup".to_string(),
            command,
            interactive: false,
        });
    }
    if commands.is_empty() {
        return Err(compatibility_json_error(
            "plugin setup must declare at least one command",
        ));
    }
    if commands.len() > MAX_SETUP_COMMAND_COUNT {
        return Err(compatibility_json_error(format!(
            "plugin setup must not contain more than {MAX_SETUP_COMMAND_COUNT} commands"
        )));
    }
    if inputs.len() > MAX_SETUP_INPUT_COUNT {
        return Err(compatibility_json_error(format!(
            "plugin setup must not contain more than {MAX_SETUP_INPUT_COUNT} inputs"
        )));
    }

    let mut input_ids = HashSet::new();
    let mut input_env_names = HashSet::new();
    let inputs = inputs
        .into_iter()
        .map(|input| {
            if input.id.trim().is_empty()
                || input.id.len() > MAX_SETUP_INPUT_ID_BYTES
                || !input
                    .id
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
            {
                return Err(compatibility_json_error(
                    "plugin setup input IDs must contain only ASCII letters, digits, `_`, or `-`",
                ));
            }
            if !input_ids.insert(input.id.clone()) {
                return Err(compatibility_json_error(format!(
                    "plugin setup input `{}` is declared more than once",
                    input.id
                )));
            }
            if input.prompt.trim().is_empty()
                || input.prompt.len() > MAX_SETUP_PROMPT_BYTES
                || input.prompt.chars().any(char::is_control)
            {
                return Err(compatibility_json_error(format!(
                    "plugin setup input `{}` must have a non-empty prompt",
                    input.id
                )));
            }
            let mut env_chars = input.env.chars();
            if input.env.len() > MAX_SETUP_ENV_BYTES
                || !env_chars
                    .next()
                    .is_some_and(|ch| ch.is_ascii_alphabetic() || ch == '_')
                || !env_chars.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            {
                return Err(compatibility_json_error(format!(
                    "plugin setup input `{}` must declare a valid environment variable",
                    input.id
                )));
            }
            if matches!(
                input.env.as_str(),
                "PLUGIN_ROOT" | "PLUGIN_DATA" | "CLAUDE_PLUGIN_ROOT" | "CLAUDE_PLUGIN_DATA"
            ) {
                return Err(compatibility_json_error(format!(
                    "plugin setup input `{}` cannot override reserved environment variable `{}`",
                    input.id, input.env
                )));
            }
            if !input_env_names.insert(input.env.clone()) {
                return Err(compatibility_json_error(format!(
                    "plugin setup environment variable `{}` is declared more than once",
                    input.env
                )));
            }
            let input_type = match input.input_type {
                RawPluginManifestSetupInputType::Text => PluginManifestSetupInputType::Text,
                RawPluginManifestSetupInputType::Directory => {
                    PluginManifestSetupInputType::Directory
                }
                RawPluginManifestSetupInputType::File => PluginManifestSetupInputType::File,
                RawPluginManifestSetupInputType::Secret => PluginManifestSetupInputType::Secret,
            };
            Ok(PluginManifestSetupInput {
                id: input.id,
                input_type,
                prompt: input.prompt,
                env: input.env,
                required: input.required,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut command_names = HashSet::new();
    let commands = commands
        .into_iter()
        .map(|action| {
            if action.name.trim().is_empty()
                || action.name.len() > MAX_SETUP_COMMAND_NAME_BYTES
                || action.name.chars().any(char::is_control)
            {
                return Err(compatibility_json_error(
                    "plugin setup command names must not be blank",
                ));
            }
            if !command_names.insert(action.name.clone()) {
                return Err(compatibility_json_error(format!(
                    "plugin setup command `{}` is declared more than once",
                    action.name
                )));
            }
            if action.command.is_empty()
                || action.command[0].trim().is_empty()
                || action.command.len() > MAX_SETUP_COMMAND_ARGUMENT_COUNT
                || action.command.iter().any(|argument| {
                    argument.len() > MAX_SETUP_COMMAND_ARGUMENT_BYTES
                        || argument.chars().any(char::is_control)
                })
            {
                return Err(compatibility_json_error(format!(
                    "plugin setup command `{}` must contain a non-empty executable and at most {MAX_SETUP_COMMAND_ARGUMENT_COUNT} NUL-free arguments",
                    action.name
                )));
            }
            Ok(PluginManifestSetupCommand {
                name: action.name,
                command: action.command,
                interactive: action.interactive,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let rendered_plan_bytes = commands
        .iter()
        .map(|action| action.name.len() + action.command.iter().map(String::len).sum::<usize>())
        .sum::<usize>();
    if rendered_plan_bytes > MAX_SETUP_RENDERED_PLAN_BYTES {
        return Err(compatibility_json_error(format!(
            "plugin setup command plan must not exceed {MAX_SETUP_RENDERED_PLAN_BYTES} bytes"
        )));
    }

    Ok(PluginManifestSetup { inputs, commands })
}
