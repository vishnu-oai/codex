use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use codex_core_plugins::InstalledPluginSetup;
use codex_core_plugins::PendingPluginSetup;
use codex_plugin::manifest::PluginManifestSetup;
use codex_plugin::manifest::PluginManifestSetupCommand;
use codex_plugin::manifest::PluginManifestSetupInput;
use codex_plugin::manifest::PluginManifestSetupInputType;
use crossterm::event::Event;
use crossterm::event::KeyCode;
use crossterm::event::KeyEventKind;
use crossterm::event::KeyModifiers;
use std::collections::HashMap;
use std::io::IsTerminal;
use std::io::Write;
use std::path::Component;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

const SETUP_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_SETUP_OUTPUT_BYTES: usize = 4096;
const MAX_SETUP_REDACTION_BYTES: usize = 8192;
const MAX_SETUP_REDACTION_COUNT: usize = 64;

#[cfg(unix)]
#[path = "plugin_setup_process.rs"]
mod process;

#[cfg(unix)]
use process::run_plugin_setup_with_timeout;

#[cfg(all(test, unix))]
use process::run_plugin_setup_with_interrupt;

#[cfg(all(test, unix))]
use process::run_plugin_setup_with_timeouts;

#[cfg(test)]
#[path = "plugin_setup_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SetupApproval {
    Approved,
    Prompt,
}

struct SetupOutputPolicy {
    remaining_bytes: AtomicUsize,
    truncation_announced: AtomicBool,
    redactions: Vec<Vec<u8>>,
}

impl SetupOutputPolicy {
    fn new(
        inputs: &[PluginManifestSetupInput],
        environment: &HashMap<String, String>,
    ) -> Arc<Self> {
        let mut redactions = inputs
            .iter()
            .filter(|input| input.input_type == PluginManifestSetupInputType::Secret)
            .filter_map(|input| environment.get(&input.env))
            .filter(|value| !value.is_empty() && value.len() <= MAX_SETUP_REDACTION_BYTES)
            .map(|value| value.as_bytes().to_vec())
            .collect::<Vec<_>>();
        for (name, value) in std::env::vars() {
            if redactions.len() >= MAX_SETUP_REDACTION_COUNT {
                break;
            }
            let name = name.to_ascii_uppercase();
            if (name.contains("SECRET")
                || name.contains("TOKEN")
                || name.contains("PASSWORD")
                || name.contains("CREDENTIAL")
                || name.contains("AUTHORIZATION")
                || name.contains("_KEY"))
                && !value.is_empty()
                && value.len() <= MAX_SETUP_REDACTION_BYTES
            {
                redactions.push(value.into_bytes());
            }
        }
        redactions
            .sort_by(|left, right| right.len().cmp(&left.len()).then_with(|| left.cmp(right)));
        redactions.dedup();
        redactions.truncate(MAX_SETUP_REDACTION_COUNT);
        Arc::new(Self {
            remaining_bytes: AtomicUsize::new(MAX_SETUP_OUTPUT_BYTES),
            truncation_announced: AtomicBool::new(false),
            redactions,
        })
    }
}

#[derive(Clone)]
struct SetupInvocation {
    installed_path: codex_utils_absolute_path::AbsolutePathBuf,
    data_path: codex_utils_absolute_path::AbsolutePathBuf,
    command: Vec<String>,
    interactive: bool,
    environment: HashMap<String, String>,
    output_policy: Arc<SetupOutputPolicy>,
}

impl SetupInvocation {
    fn from_action(
        installed_path: &codex_utils_absolute_path::AbsolutePathBuf,
        data_path: &codex_utils_absolute_path::AbsolutePathBuf,
        action: &PluginManifestSetupCommand,
        environment: &HashMap<String, String>,
        output_policy: &Arc<SetupOutputPolicy>,
    ) -> Self {
        Self {
            installed_path: installed_path.clone(),
            data_path: data_path.clone(),
            command: action.command.clone(),
            interactive: action.interactive,
            environment: environment.clone(),
            output_policy: Arc::clone(output_policy),
        }
    }
}

pub(crate) fn approve_plugin_setup(
    setup: &PluginManifestSetup,
    installed_path: &Path,
    run_setup: bool,
    json: bool,
) -> Result<()> {
    ensure_setup_execution_supported()?;
    eprintln!("Plugin setup will run these steps:");
    for (index, action) in setup.commands.iter().enumerate() {
        let command =
            serde_json::to_string(&action.command).context("serialize plugin setup command")?;
        eprintln!("  {}. {}: {command}", index + 1, action.name);
    }
    eprintln!("Working directory: {}", installed_path.display());
    eprintln!("These commands run unsandboxed with your user permissions.");
    let approval = setup_approval(
        run_setup,
        json,
        std::io::stdin().is_terminal(),
        std::io::stderr().is_terminal(),
    )?;
    if approval == SetupApproval::Prompt && !confirm("Run plugin setup? [y/N]: ")? {
        bail!(
            "plugin setup was declined; the plugin remains inactive and rerunning `codex plugin add` with `--run-setup` retries it"
        );
    }

    Ok(())
}

pub(crate) async fn run_plugin_setup(
    pending: &PendingPluginSetup,
    supplied_inputs: &[String],
    json: bool,
) -> Result<()> {
    run_setup_plan(
        &pending.setup,
        &pending.installed_path,
        &pending.data_path,
        supplied_inputs,
        json,
    )
    .await
}

pub(crate) async fn rerun_installed_plugin_setup(
    installed: &InstalledPluginSetup,
    supplied_inputs: &[String],
    json: bool,
) -> Result<()> {
    run_setup_plan(
        &installed.setup,
        &installed.installed_path,
        &installed.data_path,
        supplied_inputs,
        json,
    )
    .await
}

async fn run_setup_plan(
    setup: &PluginManifestSetup,
    installed_path: &codex_utils_absolute_path::AbsolutePathBuf,
    data_path: &codex_utils_absolute_path::AbsolutePathBuf,
    supplied_inputs: &[String],
    json: bool,
) -> Result<()> {
    ensure_setup_execution_supported()?;
    if setup.commands.iter().any(|action| action.interactive)
        && (json
            || !std::io::stdin().is_terminal()
            || !std::io::stdout().is_terminal()
            || !std::io::stderr().is_terminal())
    {
        bail!(
            "interactive plugin setup steps require a terminal and cannot run in JSON or non-interactive mode"
        );
    }
    std::fs::create_dir_all(data_path.as_path())
        .with_context(|| format!("create plugin data directory {}", data_path.display()))?;
    let environment = collect_setup_inputs(
        &setup.inputs,
        supplied_inputs,
        json,
        std::io::stdin().is_terminal(),
        std::io::stderr().is_terminal(),
    )?;
    let output_policy = SetupOutputPolicy::new(&setup.inputs, &environment);
    for (index, action) in setup.commands.iter().enumerate() {
        eprintln!("[{}/{}] {}", index + 1, setup.commands.len(), action.name);
        let invocation = SetupInvocation::from_action(
            installed_path,
            data_path,
            action,
            &environment,
            &output_policy,
        );
        run_plugin_setup_with_timeout(&invocation, SETUP_TIMEOUT)
            .await
            .with_context(|| format!("plugin setup step {} failed", action.name))?;
    }
    Ok(())
}

fn collect_setup_inputs(
    inputs: &[PluginManifestSetupInput],
    supplied_inputs: &[String],
    json: bool,
    stdin_is_terminal: bool,
    stderr_is_terminal: bool,
) -> Result<HashMap<String, String>> {
    let mut supplied = HashMap::new();
    for assignment in supplied_inputs {
        let (id, value) = assignment
            .split_once('=')
            .context("plugin setup inputs must use --set input_id=value")?;
        let input = inputs
            .iter()
            .find(|input| input.id == id)
            .with_context(|| format!("unknown plugin setup input: {id}"))?;
        if input.input_type == PluginManifestSetupInputType::Secret {
            bail!(
                "secret setup input {id} must be provided by its environment variable or a hidden terminal prompt, not --set"
            );
        }
        if supplied.insert(id.to_string(), value.to_string()).is_some() {
            bail!("plugin setup input {id} was supplied more than once");
        }
    }

    let interactive = !json && stdin_is_terminal && stderr_is_terminal;
    let mut environment = HashMap::new();
    for input in inputs {
        let value = if let Some(value) = supplied.remove(&input.id) {
            Some(value)
        } else {
            match std::env::var(&input.env) {
                Ok(value) => Some(value),
                Err(std::env::VarError::NotUnicode(_)) => {
                    bail!(
                        "plugin setup environment variable {} is not valid UTF-8",
                        input.env
                    )
                }
                Err(std::env::VarError::NotPresent) if interactive => {
                    Some(prompt_setup_input(input)?)
                }
                Err(std::env::VarError::NotPresent) => None,
            }
        };
        let Some(value) = value else {
            if input.required {
                if input.input_type == PluginManifestSetupInputType::Secret {
                    bail!(
                        "required secret setup input {} is missing; set its {} environment variable",
                        input.id,
                        input.env
                    );
                }
                bail!(
                    "required plugin setup input {} is missing; use --set {}=<value> or set {}",
                    input.id,
                    input.id,
                    input.env
                );
            }
            continue;
        };
        if value.is_empty() {
            if input.required {
                bail!("required plugin setup input {} cannot be empty", input.id);
            }
            continue;
        }
        if input.input_type == PluginManifestSetupInputType::Secret
            && value.len() > MAX_SETUP_REDACTION_BYTES
        {
            bail!(
                "plugin setup secret input {} exceeds the maximum supported secret length",
                input.id
            );
        }
        environment.insert(input.env.clone(), validate_setup_input(input, value)?);
    }
    Ok(environment)
}

fn prompt_setup_input(input: &PluginManifestSetupInput) -> Result<String> {
    if input.input_type == PluginManifestSetupInputType::Secret {
        return read_hidden_setup_input(&input.prompt);
    }

    eprint!("{}: ", input.prompt);
    std::io::stderr()
        .flush()
        .context("display plugin setup input prompt")?;
    let mut response = String::new();
    std::io::stdin()
        .read_line(&mut response)
        .context("read plugin setup input")?;
    Ok(response.trim_end_matches(['\r', '\n']).to_string())
}

fn validate_setup_input(input: &PluginManifestSetupInput, value: String) -> Result<String> {
    let expected_directory = match input.input_type {
        PluginManifestSetupInputType::Directory => true,
        PluginManifestSetupInputType::File => false,
        PluginManifestSetupInputType::Text | PluginManifestSetupInputType::Secret => {
            return Ok(value);
        }
    };
    let path = std::fs::canonicalize(&value).with_context(|| {
        format!(
            "resolve plugin setup input {} as an existing path",
            input.id
        )
    })?;
    let metadata = std::fs::metadata(&path)
        .with_context(|| format!("inspect plugin setup input {}", input.id))?;
    if expected_directory && !metadata.is_dir() {
        bail!("plugin setup input {} must be a directory", input.id);
    }
    if !expected_directory && !metadata.is_file() {
        bail!("plugin setup input {} must be a file", input.id);
    }
    path.to_str()
        .map(ToString::to_string)
        .with_context(|| format!("plugin setup input {} must be a UTF-8 path", input.id))
}

struct RawTerminalMode;

impl RawTerminalMode {
    fn enable() -> Result<Self> {
        crossterm::terminal::enable_raw_mode().context("enable hidden plugin setup input")?;
        Ok(Self)
    }
}

impl Drop for RawTerminalMode {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

fn read_hidden_setup_input(prompt: &str) -> Result<String> {
    let raw_mode = RawTerminalMode::enable()?;
    eprint!("{prompt}: ");
    std::io::stderr()
        .flush()
        .context("display hidden plugin setup input prompt")?;
    let mut response = String::new();
    loop {
        let Event::Key(event) =
            crossterm::event::read().context("read hidden plugin setup input")?
        else {
            continue;
        };
        if event.kind != KeyEventKind::Press {
            continue;
        }
        match event.code {
            KeyCode::Enter => break,
            KeyCode::Backspace => {
                response.pop();
            }
            KeyCode::Char('c') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                drop(raw_mode);
                eprintln!();
                bail!("plugin setup secret input was interrupted");
            }
            KeyCode::Char(value) => response.push(value),
            _ => {}
        }
    }
    drop(raw_mode);
    eprintln!();
    Ok(response)
}

#[cfg(unix)]
pub(crate) fn ensure_setup_execution_supported() -> Result<()> {
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn ensure_setup_execution_supported() -> Result<()> {
    bail!("plugin setup commands are currently supported only on Unix; the plugin remains inactive")
}

fn setup_approval(
    run_setup: bool,
    json: bool,
    stdin_is_terminal: bool,
    stderr_is_terminal: bool,
) -> Result<SetupApproval> {
    if run_setup {
        return Ok(SetupApproval::Approved);
    }
    if json || !(stdin_is_terminal && stderr_is_terminal) {
        bail!(
            "plugin setup requires explicit consent in non-interactive or JSON mode; the plugin remains inactive and rerunning with `--run-setup` retries it"
        );
    }
    Ok(SetupApproval::Prompt)
}

fn confirm(prompt: &str) -> std::io::Result<bool> {
    eprint!("{prompt}");
    std::io::stderr().flush()?;
    let mut response = String::new();
    std::io::stdin().read_line(&mut response)?;
    Ok(matches!(
        response.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
}

#[cfg(not(unix))]
async fn run_plugin_setup_with_timeout(
    _invocation: &SetupInvocation,
    _setup_timeout: Duration,
) -> Result<()> {
    ensure_setup_execution_supported()
}

fn resolve_setup_executable(installed_path: &Path, executable: &str) -> Result<PathBuf> {
    let executable_path = Path::new(executable);
    let mut components = executable_path.components();
    let is_bare_name = matches!(components.next(), Some(Component::Normal(_)))
        && components.next().is_none()
        && !executable.contains(std::path::MAIN_SEPARATOR)
        && !(cfg!(windows) && executable.contains('/'));
    if executable_path.is_absolute() || is_bare_name {
        Ok(executable_path.to_path_buf())
    } else {
        let plugin_root = installed_path
            .canonicalize()
            .context("resolve plugin setup root")?;
        let executable_path = installed_path
            .join(executable_path)
            .canonicalize()
            .context("resolve plugin-relative setup executable")?;
        if !executable_path.starts_with(&plugin_root) {
            bail!("plugin-relative setup executable must remain inside PLUGIN_ROOT");
        }
        if !executable_path.is_file() {
            bail!("plugin-relative setup executable must be a file");
        }
        Ok(executable_path)
    }
}
