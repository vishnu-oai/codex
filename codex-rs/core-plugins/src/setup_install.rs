use crate::store::PluginStoreError;
use codex_plugin::PluginId;
use serde::Deserialize;
use serde::Serialize;
use sha2::Digest;
use sha2::Sha256;
use std::ffi::OsStr;
use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::Read;
use std::path::Path;

const SETUP_INSTALL_LOCKS_DIR: &str = "plugins/setup-install-locks";
const SETUP_COMPLETION_MARKER_RELATIVE_PATH: &str = ".codex-plugin/setup-complete.json";
const SETUP_COMPLETION_MARKER_SCHEMA_VERSION: u8 = 1;
pub(crate) const SETUP_CACHE_VERSION_PREFIX: &str = "@setup-";

#[cfg(test)]
#[path = "setup_install_tests.rs"]
mod tests;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SetupCompletionMarker {
    schema_version: u8,
    content_fingerprint: String,
}

/// Exclusive per-plugin cache mutation lock.
///
/// Foreground setup holds this from materialization through activation; all other cache writers
/// acquire the same lock for the duration of their mutation.
#[derive(Debug)]
pub(crate) struct ForegroundPluginInstallLock {
    file: File,
    plugin_id: PluginId,
}

/// Shared per-plugin cache read lock.
///
/// Plugin loading uses a nonblocking shared lock so it fails closed while a cache writer is
/// active without making startup wait for an interactive setup command to finish.
#[derive(Debug)]
pub(crate) struct PluginCacheReadLock {
    file: File,
}

impl ForegroundPluginInstallLock {
    pub(crate) fn acquire(
        codex_home: &Path,
        plugin_id: &PluginId,
    ) -> Result<Self, PluginStoreError> {
        let file = open_plugin_cache_lock(codex_home, plugin_id)?;
        file.lock().map_err(|err| {
            PluginStoreError::io("failed to acquire plugin setup install lock", err)
        })?;
        Ok(Self {
            file,
            plugin_id: plugin_id.clone(),
        })
    }

    pub(crate) fn validate_plugin_id(&self, plugin_id: &PluginId) -> Result<(), PluginStoreError> {
        if &self.plugin_id == plugin_id {
            return Ok(());
        }
        Err(PluginStoreError::Invalid(format!(
            "plugin cache mutation lock for `{}` cannot mutate `{}`",
            self.plugin_id.as_key(),
            plugin_id.as_key()
        )))
    }
}

impl PluginCacheReadLock {
    pub(crate) fn try_acquire(
        codex_home: &Path,
        plugin_id: &PluginId,
    ) -> Result<Option<Self>, PluginStoreError> {
        let file = open_plugin_cache_lock(codex_home, plugin_id)?;
        match file.try_lock_shared() {
            Ok(()) => Ok(Some(Self { file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(err)) => Err(PluginStoreError::io(
                "failed to acquire shared plugin cache read lock",
                err,
            )),
        }
    }
}

fn open_plugin_cache_lock(
    codex_home: &Path,
    plugin_id: &PluginId,
) -> Result<File, PluginStoreError> {
    let lock_dir = codex_home
        .join(SETUP_INSTALL_LOCKS_DIR)
        .join(&plugin_id.marketplace_name);
    fs::create_dir_all(&lock_dir).map_err(|err| {
        PluginStoreError::io("failed to create plugin setup install lock directory", err)
    })?;
    let lock_path = lock_dir.join(format!("{}.lock", plugin_id.plugin_name));
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(|err| PluginStoreError::io("failed to open plugin setup install lock", err))
}

impl Drop for ForegroundPluginInstallLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

impl Drop for PluginCacheReadLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub(crate) fn plugin_content_fingerprint(root: &Path) -> Result<String, PluginStoreError> {
    if !root.is_dir() {
        return Err(PluginStoreError::Invalid(format!(
            "cannot fingerprint plugin package because its root is not a directory: {}",
            root.display()
        )));
    }
    let mut hasher = Sha256::new();
    hash_directory(root, root, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Returns a content-addressed cache generation for a setup-bearing package.
///
/// The installed path is part of the runtime trust boundary because long-lived capabilities can
/// retain absolute paths beneath `PLUGIN_ROOT`. Different package bytes must therefore never reuse
/// a path that an older process may still hold.
pub(crate) fn setup_cache_version(staged_root: &Path) -> Result<String, PluginStoreError> {
    let content_fingerprint = plugin_content_fingerprint(staged_root)?;
    Ok(format!("{SETUP_CACHE_VERSION_PREFIX}{content_fingerprint}"))
}

pub(crate) fn prepare_setup_completion_marker(root: &Path) -> Result<(), PluginStoreError> {
    let marker_path = setup_completion_marker_path(root);
    match fs::symlink_metadata(&marker_path) {
        Ok(metadata) if metadata.is_dir() => {
            fs::remove_dir_all(&marker_path).map_err(|err| {
                PluginStoreError::io("failed to remove plugin setup completion marker", err)
            })?;
        }
        Ok(_) => {
            fs::remove_file(&marker_path).map_err(|err| {
                PluginStoreError::io("failed to remove plugin setup completion marker", err)
            })?;
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err(PluginStoreError::io(
                "failed to inspect plugin setup completion marker",
                err,
            ));
        }
    }
    let marker_parent = marker_path.parent().ok_or_else(|| {
        PluginStoreError::Invalid("plugin setup completion marker has no parent".to_string())
    })?;
    fs::create_dir_all(marker_parent).map_err(|err| {
        PluginStoreError::io(
            "failed to create plugin setup completion marker directory",
            err,
        )
    })
}

pub(crate) fn write_setup_completion_marker(
    root: &Path,
    expected_content_fingerprint: &str,
) -> Result<(), PluginStoreError> {
    let content_fingerprint = plugin_content_fingerprint(root)?;
    if content_fingerprint != expected_content_fingerprint {
        return Err(PluginStoreError::Invalid(
            "plugin package changed before setup completion could be recorded".to_string(),
        ));
    }
    let marker = SetupCompletionMarker {
        schema_version: SETUP_COMPLETION_MARKER_SCHEMA_VERSION,
        content_fingerprint,
    };
    let marker_path = setup_completion_marker_path(root);
    let marker_parent = marker_path.parent().ok_or_else(|| {
        PluginStoreError::Invalid("plugin setup completion marker has no parent".to_string())
    })?;
    fs::create_dir_all(marker_parent).map_err(|err| {
        PluginStoreError::io(
            "failed to create plugin setup completion marker directory",
            err,
        )
    })?;
    let contents = serde_json::to_vec_pretty(&marker).map_err(|err| {
        PluginStoreError::Invalid(format!(
            "failed to serialize plugin setup completion marker: {err}"
        ))
    })?;
    fs::write(&marker_path, contents)
        .map_err(|err| PluginStoreError::io("failed to write plugin setup completion marker", err))
}

pub(crate) fn validate_setup_completion_marker(root: &Path) -> Result<(), PluginStoreError> {
    let marker_path = setup_completion_marker_path(root);
    let contents = match fs::read(&marker_path) {
        Ok(contents) => contents,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(PluginStoreError::Invalid(
                "plugin setup is incomplete: setup completion marker is missing".to_string(),
            ));
        }
        Err(err) => {
            return Err(PluginStoreError::io(
                "failed to read plugin setup completion marker",
                err,
            ));
        }
    };
    let marker: SetupCompletionMarker = serde_json::from_slice(&contents).map_err(|err| {
        PluginStoreError::Invalid(format!("plugin setup completion marker is invalid: {err}"))
    })?;
    if marker.schema_version != SETUP_COMPLETION_MARKER_SCHEMA_VERSION {
        return Err(PluginStoreError::Invalid(format!(
            "plugin setup completion marker has unsupported schema version {}",
            marker.schema_version
        )));
    }
    let content_fingerprint = plugin_content_fingerprint(root)?;
    if marker.content_fingerprint != content_fingerprint {
        return Err(PluginStoreError::Invalid(
            "plugin setup completion marker does not match the current package contents"
                .to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn setup_completion_marker_path(root: &Path) -> std::path::PathBuf {
    root.join(SETUP_COMPLETION_MARKER_RELATIVE_PATH)
}

fn hash_directory(
    root: &Path,
    directory: &Path,
    hasher: &mut Sha256,
) -> Result<(), PluginStoreError> {
    let mut entries = fs::read_dir(directory)
        .map_err(|err| PluginStoreError::io("failed to read plugin package for fingerprint", err))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| {
            PluginStoreError::io("failed to enumerate plugin package for fingerprint", err)
        })?;
    entries.sort_by_key(fs::DirEntry::file_name);

    for entry in entries {
        let path = entry.path();
        let relative_path = path.strip_prefix(root).map_err(|err| {
            PluginStoreError::Invalid(format!(
                "failed to fingerprint plugin package path {}: {err}",
                path.display()
            ))
        })?;
        if relative_path == Path::new(SETUP_COMPLETION_MARKER_RELATIVE_PATH) {
            continue;
        }
        let metadata = fs::symlink_metadata(&path).map_err(|err| {
            PluginStoreError::io("failed to inspect plugin package for fingerprint", err)
        })?;
        if metadata.is_dir() {
            hash_entry_header(hasher, /*kind*/ b'd', relative_path.as_os_str());
            hash_permissions(hasher, &metadata);
            hash_directory(root, &path, hasher)?;
        } else if metadata.is_file() {
            hash_entry_header(hasher, /*kind*/ b'f', relative_path.as_os_str());
            hash_permissions(hasher, &metadata);
            hasher.update(metadata.len().to_le_bytes());
            let mut file = File::open(&path).map_err(|err| {
                PluginStoreError::io("failed to open plugin file for fingerprint", err)
            })?;
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = file.read(&mut buffer).map_err(|err| {
                    PluginStoreError::io("failed to read plugin file for fingerprint", err)
                })?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
        } else if metadata.file_type().is_symlink() {
            hash_entry_header(hasher, /*kind*/ b'l', relative_path.as_os_str());
            hash_permissions(hasher, &metadata);
            let target = fs::read_link(&path).map_err(|err| {
                PluginStoreError::io("failed to read plugin symlink for fingerprint", err)
            })?;
            hash_os_str(hasher, target.as_os_str());
        } else {
            return Err(PluginStoreError::Invalid(format!(
                "unsupported plugin package entry while fingerprinting: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn hash_entry_header(hasher: &mut Sha256, kind: u8, path: &OsStr) {
    hasher.update([kind]);
    hash_os_str(hasher, path);
}

#[cfg(unix)]
fn hash_permissions(hasher: &mut Sha256, metadata: &fs::Metadata) {
    use std::os::unix::fs::PermissionsExt;

    hasher.update(metadata.permissions().mode().to_le_bytes());
}

#[cfg(windows)]
fn hash_permissions(hasher: &mut Sha256, metadata: &fs::Metadata) {
    hasher.update([u8::from(metadata.permissions().readonly())]);
}

#[cfg(not(any(unix, windows)))]
fn hash_permissions(hasher: &mut Sha256, metadata: &fs::Metadata) {
    hasher.update([u8::from(metadata.permissions().readonly())]);
}

#[cfg(unix)]
fn hash_os_str(hasher: &mut Sha256, value: &OsStr) {
    use std::os::unix::ffi::OsStrExt;

    let bytes = value.as_bytes();
    hasher.update((bytes.len() as u64).to_le_bytes());
    hasher.update(bytes);
}

#[cfg(windows)]
fn hash_os_str(hasher: &mut Sha256, value: &OsStr) {
    use std::os::windows::ffi::OsStrExt;

    let units = value.encode_wide().collect::<Vec<_>>();
    hasher.update((units.len() as u64).to_le_bytes());
    for unit in units {
        hasher.update(unit.to_le_bytes());
    }
}

#[cfg(not(any(unix, windows)))]
fn hash_os_str(hasher: &mut Sha256, value: &OsStr) {
    let value = value.to_string_lossy();
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value.as_bytes());
}
