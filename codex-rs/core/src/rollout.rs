//! Functionality to persist a Codex conversation *rollout* – a linear list of
//! [`ResponseItem`] objects exchanged during a session – to disk so that
//! sessions can be replayed or inspected later (mirrors the behaviour of the
//! upstream TypeScript implementation).

use std::fs::File;
use std::fs::{self};
use std::io::Error as IoError;

use serde::Serialize;
use time::OffsetDateTime;
use time::format_description::FormatItem;
use time::macros::format_description;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::Sender;

use uuid::Uuid;

use crate::config::Config;
use crate::git_info::GitInfo;
use crate::git_info::collect_git_info;
use crate::models::ResponseItem;

/// Folder inside `~/.codex` that holds saved rollouts.
const SESSIONS_SUBDIR: &str = "sessions";

#[derive(Serialize)]
struct SessionMeta {
    id: String,
    timestamp: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    git: Option<GitInfo>,
}

/// Records all [`ResponseItem`]s for a session and flushes them to disk after
/// every update.
///
/// Rollouts are recorded as JSONL and can be inspected with tools such as:
///
/// ```ignore
/// $ jq -C . ~/.codex/sessions/rollout-2025-05-07T17-24-21-5973b6c0-94b8-487b-a530-2aeb6098ae0e.jsonl
/// $ fx ~/.codex/sessions/rollout-2025-05-07T17-24-21-5973b6c0-94b8-487b-a530-2aeb6098ae0e.jsonl
/// ```
#[derive(Clone)]
pub(crate) struct RolloutRecorder {
    tx: Sender<String>,
}

impl RolloutRecorder {
    /// Attempt to create a new [`RolloutRecorder`]. If the sessions directory
    /// cannot be created or the rollout file cannot be opened we return the
    /// error so the caller can decide whether to disable persistence.
    pub async fn new(
        config: &Config,
        uuid: Uuid,
        instructions: Option<String>,
    ) -> std::io::Result<Self> {
        let LogFileInfo {
            file,
            session_id,
            timestamp,
        } = create_log_file(config, uuid)?;

        // Build the static session metadata JSON first.
        let timestamp_format: &[FormatItem] = format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
        );
        let timestamp = timestamp
            .format(timestamp_format)
            .map_err(|e| IoError::other(format!("failed to format timestamp: {e}")))?;

        // A reasonably-sized bounded channel. If the buffer fills up the send
        // future will yield, which is fine – we only need to ensure we do not
        // perform *blocking* I/O on the caller's thread.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(256);

        // Clone the cwd for the spawned task to collect git info asynchronously
        let cwd = config.cwd.clone();

        // Spawn a Tokio task that owns the file handle and performs async
        // writes. Using `tokio::fs::File` keeps everything on the async I/O
        // driver instead of blocking the runtime.
        tokio::task::spawn(async move {
            let mut file = tokio::fs::File::from_std(file);

            // Collect git repository information asynchronously without blocking startup
            let git_info = collect_git_info(&cwd).await;

            let meta = SessionMeta {
                timestamp,
                id: session_id.to_string(),
                instructions,
                git: git_info,
            };

            // Write the SessionMeta as the first item in the file
            if let Ok(json) = serde_json::to_string(&meta) {
                if let Err(e) = file.write_all(json.as_bytes()).await {
                    tracing::warn!("rollout writer: failed to write SessionMeta: {e}");
                    return;
                }
                if let Err(e) = file.write_all(b"\n").await {
                    tracing::warn!("rollout writer: failed to write SessionMeta newline: {e}");
                    return;
                }
                if let Err(e) = file.flush().await {
                    tracing::warn!("rollout writer: failed to flush SessionMeta: {e}");
                    return;
                }
            } else {
                tracing::warn!("rollout writer: failed to serialize SessionMeta");
                return;
            }

            // Now handle the regular stream of items
            while let Some(line) = rx.recv().await {
                // Write line + newline, then flush to disk.
                if let Err(e) = file.write_all(line.as_bytes()).await {
                    tracing::warn!("rollout writer: failed to write line: {e}");
                    break;
                }
                if let Err(e) = file.write_all(b"\n").await {
                    tracing::warn!("rollout writer: failed to write newline: {e}");
                    break;
                }
                if let Err(e) = file.flush().await {
                    tracing::warn!("rollout writer: failed to flush: {e}");
                    break;
                }
            }
        });

        Ok(Self { tx })
    }

    /// Append `items` to the rollout file.
    pub(crate) async fn record_items(&self, items: &[ResponseItem]) -> std::io::Result<()> {
        for item in items {
            match item {
                // Note that function calls may look a bit strange if they are
                // "fully qualified MCP tool calls," so we could consider
                // reformatting them in that case.
                ResponseItem::Message { .. }
                | ResponseItem::LocalShellCall { .. }
                | ResponseItem::FunctionCall { .. }
                | ResponseItem::FunctionCallOutput { .. } => {}
                ResponseItem::Reasoning { .. } | ResponseItem::Other => {
                    // These should never be serialized.
                    continue;
                }
            }
            self.record_item(item).await?;
        }
        Ok(())
    }

    async fn record_item(&self, item: &impl Serialize) -> std::io::Result<()> {
        // Serialize the item to JSON first so that the writer thread only has
        // to perform the actual write.
        let json = serde_json::to_string(item)
            .map_err(|e| IoError::other(format!("failed to serialize response items: {e}")))?;

        self.tx
            .send(json)
            .await
            .map_err(|e| IoError::other(format!("failed to queue rollout item: {e}")))
    }

    /// Non-blocking version of record_item that returns immediately if the channel is full.
    /// Returns Ok(()) if the item was queued successfully, or an Err if the channel is full or closed.
    fn try_record_item(&self, item: &impl Serialize) -> std::io::Result<()> {
        // Serialize the item to JSON first so that the writer thread only has
        // to perform the actual write.
        let json = serde_json::to_string(item)
            .map_err(|e| IoError::other(format!("failed to serialize response items: {e}")))?;

        self.tx
            .try_send(json)
            .map_err(|e| IoError::other(format!("failed to queue rollout item: {e}")))
    }
}

struct LogFileInfo {
    /// Opened file handle to the rollout file.
    file: File,

    /// Session ID (also embedded in filename).
    session_id: Uuid,

    /// Timestamp for the start of the session.
    timestamp: OffsetDateTime,
}

fn create_log_file(config: &Config, session_id: Uuid) -> std::io::Result<LogFileInfo> {
    // Resolve ~/.codex/sessions and create it if missing.
    let mut dir = config.codex_home.clone();
    dir.push(SESSIONS_SUBDIR);
    fs::create_dir_all(&dir)?;

    let timestamp = OffsetDateTime::now_local()
        .map_err(|e| IoError::other(format!("failed to get local time: {e}")))?;

    // Custom format for YYYY-MM-DDThh-mm-ss. Use `-` instead of `:` for
    // compatibility with filesystems that do not allow colons in filenames.
    let format: &[FormatItem] =
        format_description!("[year]-[month]-[day]T[hour]-[minute]-[second]");
    let date_str = timestamp
        .format(format)
        .map_err(|e| IoError::other(format!("failed to format timestamp: {e}")))?;

    let filename = format!("rollout-{date_str}-{session_id}.jsonl");

    let path = dir.join(filename);
    let file = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(&path)?;

    Ok(LogFileInfo {
        file,
        session_id,
        timestamp,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    #![allow(clippy::unwrap_used)]

    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_try_record_item_non_blocking() {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        
        // Create a minimal config just for testing
        let config = crate::config::ConfigToml::default();
        let config = Config::load_from_base_config_with_overrides(
            config,
            crate::config::ConfigOverrides {
                cwd: Some(temp_dir.path().to_path_buf()),
                ..Default::default()
            },
            temp_dir.path().to_path_buf(),
        ).expect("Failed to create config");

        let recorder = RolloutRecorder::new(&config, uuid::Uuid::new_v4(), None)
            .await
            .expect("Failed to create recorder");

        // Test that try_record_item doesn't block
        let test_data = serde_json::json!({"test": "data"});
        let result = recorder.try_record_item(&test_data);
        
        // Should succeed without blocking
        assert!(result.is_ok());
    }
}
