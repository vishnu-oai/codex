use super::SetupInvocation;
use super::SetupOutputPolicy;
use super::resolve_setup_executable;
use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use std::future::Future;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::io::AsyncRead;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::task::JoinHandle;
use tokio::time::timeout;

const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

struct SetupProcessTree {
    process_group_id: u32,
    terminated: AtomicBool,
}

impl SetupProcessTree {
    fn from_child(child: &tokio::process::Child) -> std::io::Result<Self> {
        let process_group_id = child
            .id()
            .ok_or_else(|| std::io::Error::other("plugin setup process has no process group ID"))?;
        Ok(Self {
            process_group_id,
            terminated: AtomicBool::new(false),
        })
    }

    fn terminate(&self) -> std::io::Result<()> {
        if self.terminated.load(Ordering::Relaxed) {
            return Ok(());
        }
        kill_process_group(self.process_group_id)?;
        self.terminated.store(true, Ordering::Relaxed);
        Ok(())
    }
}

impl Drop for SetupProcessTree {
    fn drop(&mut self) {
        let _ = self.terminate();
    }
}

struct InteractiveTerminalForeground {
    terminal_fd: libc::c_int,
    original_process_group: libc::pid_t,
}

impl InteractiveTerminalForeground {
    fn acquire(process_group_id: u32) -> std::io::Result<Self> {
        let terminal_fd = libc::STDIN_FILENO;
        // SAFETY: stdin is an existing process file descriptor. tcgetpgrp does not retain it.
        let original_process_group = unsafe { libc::tcgetpgrp(terminal_fd) };
        if original_process_group == -1 {
            return Err(std::io::Error::last_os_error());
        }
        let child_process_group = libc::pid_t::try_from(process_group_id).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "plugin setup process group exceeds the supported process ID range",
            )
        })?;
        set_terminal_foreground(terminal_fd, child_process_group)?;
        let guard = Self {
            terminal_fd,
            original_process_group,
        };

        // SAFETY: the target is the dedicated process group created for this setup child.
        let continued = unsafe { libc::killpg(child_process_group, libc::SIGCONT) };
        if continued == -1 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }

        Ok(guard)
    }
}

impl Drop for InteractiveTerminalForeground {
    fn drop(&mut self) {
        let _ = set_terminal_foreground(self.terminal_fd, self.original_process_group);
    }
}

fn set_terminal_foreground(
    terminal_fd: libc::c_int,
    process_group: libc::pid_t,
) -> std::io::Result<()> {
    // SAFETY: sigaction is a plain C structure; zero is valid before its fields are initialized.
    let mut ignored_action = unsafe { std::mem::zeroed::<libc::sigaction>() };
    ignored_action.sa_sigaction = libc::SIG_IGN;
    // SAFETY: sa_mask points to the initialized sigaction structure for this call.
    if unsafe { libc::sigemptyset(&mut ignored_action.sa_mask) } == -1 {
        return Err(std::io::Error::last_os_error());
    }

    // SAFETY: sigaction is a plain C structure populated by the successful sigaction call.
    let mut previous_action = unsafe { std::mem::zeroed::<libc::sigaction>() };
    // SAFETY: both sigaction pointers remain valid for the complete call.
    if unsafe { libc::sigaction(libc::SIGTTOU, &ignored_action, &mut previous_action) } == -1 {
        return Err(std::io::Error::last_os_error());
    }

    // SAFETY: terminal_fd is the current terminal and process_group belongs to this session.
    let switch_result = unsafe { libc::tcsetpgrp(terminal_fd, process_group) };
    let switch_error = (switch_result == -1).then(std::io::Error::last_os_error);

    // SAFETY: previous_action was initialized by the successful sigaction call above.
    let restore_result =
        unsafe { libc::sigaction(libc::SIGTTOU, &previous_action, std::ptr::null_mut()) };
    let restore_error = (restore_result == -1).then(std::io::Error::last_os_error);

    if let Some(error) = switch_error {
        return Err(error);
    }
    if let Some(error) = restore_error {
        return Err(error);
    }
    Ok(())
}

pub(super) async fn run_plugin_setup_with_timeout(
    invocation: &SetupInvocation,
    setup_timeout: Duration,
) -> Result<()> {
    run_plugin_setup_with_timeouts(invocation, setup_timeout, OUTPUT_DRAIN_TIMEOUT).await
}

pub(super) async fn run_plugin_setup_with_timeouts(
    invocation: &SetupInvocation,
    setup_timeout: Duration,
    output_drain_timeout: Duration,
) -> Result<()> {
    // Register before spawning so Ctrl-C cannot leave a newly created process group orphaned.
    let mut interrupts = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
        .context("install Ctrl-C handler for plugin setup")?;
    let interrupt = async move {
        interrupts.recv().await.ok_or_else(|| {
            std::io::Error::other("plugin setup Ctrl-C signal stream ended unexpectedly")
        })
    };
    run_plugin_setup_with_interrupt(invocation, setup_timeout, output_drain_timeout, interrupt)
        .await
}

pub(super) async fn run_plugin_setup_with_interrupt(
    invocation: &SetupInvocation,
    setup_timeout: Duration,
    output_drain_timeout: Duration,
    interrupt: impl Future<Output = std::io::Result<()>>,
) -> Result<()> {
    let (executable, args) = invocation
        .command
        .split_first()
        .context("plugin setup command is empty")?;
    let resolved_executable =
        resolve_setup_executable(invocation.installed_path.as_path(), executable)?;
    let mut command = Command::new(&resolved_executable);
    command
        .args(args)
        .current_dir(invocation.installed_path.as_path())
        .envs(&invocation.environment)
        .env("PLUGIN_ROOT", invocation.installed_path.as_path())
        .env("PLUGIN_DATA", invocation.data_path.as_path())
        .env("CLAUDE_PLUGIN_ROOT", invocation.installed_path.as_path())
        .env("CLAUDE_PLUGIN_DATA", invocation.data_path.as_path())
        .stdin(if invocation.interactive {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .stdout(if invocation.interactive {
            Stdio::inherit()
        } else {
            Stdio::piped()
        })
        .stderr(if invocation.interactive {
            Stdio::inherit()
        } else {
            Stdio::null()
        })
        .kill_on_drop(true);
    if !invocation.interactive {
        // SAFETY: this post-fork closure only duplicates stdout onto stderr using dup2, which
        // is async-signal-safe. Both output streams then share one bounded redaction pipeline,
        // so a secret split across stdout and stderr cannot be reassembled in parent output.
        unsafe {
            command.pre_exec(|| {
                if libc::dup2(libc::STDOUT_FILENO, libc::STDERR_FILENO) == -1 {
                    Err(std::io::Error::last_os_error())
                } else {
                    Ok(())
                }
            });
        }
    }
    command.process_group(/*pgroup*/ 0);
    let mut child = command
        .spawn()
        .with_context(|| format!("start plugin setup executable `{executable}`"))?;
    let process_tree =
        SetupProcessTree::from_child(&child).context("record plugin setup process containment")?;
    let _terminal_foreground = if invocation.interactive {
        Some(
            InteractiveTerminalForeground::acquire(process_tree.process_group_id)
                .context("hand the foreground terminal to interactive plugin setup")?,
        )
    } else {
        None
    };

    let stdout_forwarder = if let Some(stdout) = child.stdout.take() {
        tokio::spawn(forward_to_parent_stderr(
            stdout,
            Arc::clone(&invocation.output_policy),
        ))
    } else if invocation.interactive {
        tokio::spawn(async { Ok(()) })
    } else {
        bail!("capture plugin setup stdout");
    };
    let stderr_forwarder = tokio::spawn(async { Ok(()) });

    enum WaitOutcome {
        Exited(std::io::Result<std::process::ExitStatus>),
        TimedOut,
        Interrupted(std::io::Result<()>),
    }

    tokio::pin!(interrupt);
    let wait_outcome = tokio::select! {
        status = child.wait() => WaitOutcome::Exited(status),
        _ = tokio::time::sleep(setup_timeout) => WaitOutcome::TimedOut,
        interrupt = &mut interrupt => WaitOutcome::Interrupted(interrupt),
    };
    let status = match wait_outcome {
        WaitOutcome::Exited(status) => status.context("wait for plugin setup command")?,
        incomplete @ WaitOutcome::TimedOut | incomplete @ WaitOutcome::Interrupted(_) => {
            let termination_result = terminate_and_reap(&mut child, &process_tree).await;
            let forwarding_result = finish_output_forwarding(
                stdout_forwarder,
                stderr_forwarder,
                &process_tree,
                output_drain_timeout,
            )
            .await;
            termination_result?;
            forwarding_result?;
            match incomplete {
                WaitOutcome::TimedOut => {
                    bail!(
                        "plugin setup timed out after {} seconds; the plugin remains inactive",
                        setup_timeout.as_secs()
                    );
                }
                WaitOutcome::Interrupted(interrupt) => {
                    interrupt.context("listen for Ctrl-C while plugin setup runs")?;
                    bail!("plugin setup was interrupted; the plugin remains inactive");
                }
                WaitOutcome::Exited(_) => unreachable!(),
            }
        }
    };
    let forwarding_result = finish_output_forwarding(
        stdout_forwarder,
        stderr_forwarder,
        &process_tree,
        output_drain_timeout,
    )
    .await;
    let cleanup_result = process_tree.terminate();
    forwarding_result?;
    cleanup_result.context("clean up remaining plugin setup processes")?;
    if !status.success() {
        bail!(
            "plugin setup failed with status {status}; the plugin remains inactive and `codex plugin add` can retry it"
        );
    }
    Ok(())
}

async fn forward_to_parent_stderr(
    mut output: impl AsyncRead + Unpin,
    policy: Arc<SetupOutputPolicy>,
) -> std::io::Result<()> {
    const BUFFER_SIZE: usize = 8 * 1024;

    let mut stderr = tokio::io::stderr();
    let mut buffer = [0_u8; BUFFER_SIZE];
    let mut pending = Vec::new();
    let retained_bytes = policy
        .redactions
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(1)
        .saturating_sub(1);
    let mut write_error = None;
    loop {
        let read = output.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        if write_error.is_some()
            || (policy.remaining_bytes.load(Ordering::Acquire) == 0
                && policy.truncation_announced.load(Ordering::Acquire))
        {
            continue;
        }
        pending.extend_from_slice(&buffer[..read]);
        let safe_boundary = pending.len().saturating_sub(retained_bytes);
        let mut safe_end = 0;
        while safe_end < safe_boundary {
            if let Some(secret) = policy
                .redactions
                .iter()
                .find(|secret| pending[safe_end..].starts_with(secret.as_slice()))
            {
                safe_end += secret.len();
            } else {
                safe_end += 1;
            }
        }
        if safe_end > 0 {
            let safe_output = pending.drain(..safe_end).collect::<Vec<_>>();
            if let Err(error) =
                write_sanitized_setup_output(&mut stderr, &safe_output, &policy).await
            {
                // Keep draining the child even when the parent stream cannot accept more data.
                write_error = Some(error);
            }
        }
    }
    if write_error.is_none()
        && !pending.is_empty()
        && let Err(error) = write_sanitized_setup_output(&mut stderr, &pending, &policy).await
    {
        write_error = Some(error);
    }
    if let Some(error) = write_error {
        return Err(error);
    }
    stderr.flush().await
}

async fn write_sanitized_setup_output(
    stderr: &mut tokio::io::Stderr,
    output: &[u8],
    policy: &SetupOutputPolicy,
) -> std::io::Result<()> {
    let mut sanitized = Vec::with_capacity(output.len());
    let mut offset = 0;
    while offset < output.len() {
        if let Some(secret) = policy
            .redactions
            .iter()
            .find(|secret| output[offset..].starts_with(secret.as_slice()))
        {
            sanitized.extend_from_slice(b"[REDACTED]");
            offset += secret.len();
            continue;
        }
        let byte = output[offset];
        if (byte < 0x20 && byte != b'\n' && byte != b'\t') || byte == 0x7f {
            sanitized.push(b'?');
        } else {
            sanitized.push(byte);
        }
        offset += 1;
    }

    let requested = sanitized.len();
    let previous = policy
        .remaining_bytes
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |remaining| {
            Some(remaining.saturating_sub(requested))
        })
        .unwrap_or(0);
    let permitted = previous.min(requested);
    if permitted != 0 {
        stderr.write_all(&sanitized[..permitted]).await?;
    }
    if permitted < requested && !policy.truncation_announced.swap(true, Ordering::AcqRel) {
        stderr
            .write_all(b"\n[plugin setup output truncated]\n")
            .await?;
    }
    Ok(())
}

async fn terminate_and_reap(
    child: &mut tokio::process::Child,
    process_tree: &SetupProcessTree,
) -> Result<()> {
    let process_tree_kill_error = process_tree.terminate().err();
    let child_kill_error = child
        .start_kill()
        .err()
        .filter(|error| error.kind() != std::io::ErrorKind::InvalidInput);
    child
        .wait()
        .await
        .context("reap timed out plugin setup command")?;
    if let Some(error) = process_tree_kill_error {
        return Err(error).context("terminate plugin setup process tree");
    }
    if let Some(error) = child_kill_error {
        return Err(error).context("terminate timed out plugin setup command");
    }
    Ok(())
}

async fn finish_output_forwarding(
    mut stdout: JoinHandle<std::io::Result<()>>,
    mut stderr: JoinHandle<std::io::Result<()>>,
    process_tree: &SetupProcessTree,
    output_drain_timeout: Duration,
) -> Result<()> {
    let forwarding = async {
        let (stdout, stderr) = tokio::join!(&mut stdout, &mut stderr);
        stdout
            .context("join plugin setup stdout forwarder")?
            .context("forward plugin setup stdout")?;
        stderr
            .context("join plugin setup stderr forwarder")?
            .context("forward plugin setup stderr")?;
        Ok(())
    };
    match timeout(output_drain_timeout, forwarding).await {
        Ok(result) => result,
        Err(_) => {
            let process_tree_kill_error = process_tree.terminate().err();
            stdout.abort();
            stderr.abort();
            if let Some(error) = process_tree_kill_error {
                return Err(error)
                    .context("terminate plugin setup process tree holding output streams");
            }
            bail!(
                "plugin setup output streams did not close within {} seconds; the plugin remains inactive",
                output_drain_timeout.as_secs_f64()
            )
        }
    }
}

fn kill_process_group(process_group_id: u32) -> std::io::Result<()> {
    let process_group_id = libc::pid_t::try_from(process_group_id).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "plugin setup process group exceeds the supported process ID range",
        )
    })?;
    // SAFETY: the ID is the dedicated process group created for the setup command.
    let result = unsafe { libc::killpg(process_group_id, libc::SIGKILL) };
    if result == 0 {
        return Ok(());
    }
    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(error)
    }
}
