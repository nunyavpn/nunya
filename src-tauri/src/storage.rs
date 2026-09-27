//! On-disk storage for the app's data.
//!
//! The file holds every server the user has, which means UUIDs, passwords and Reality keys —
//! everything needed to impersonate them. So it is written with the same care as a key file:
//! owner-only permissions, inside an owner-only directory, and replaced atomically so a crash or a
//! full disk cannot leave a half-written list behind.
//!
//! The payload is treated as opaque JSON. The schema lives in the frontend, which is the only thing
//! that reads it today, and mirroring it here would mean two definitions to keep in step for no
//! current benefit. When something in Rust needs to read a server — testing latency without the
//! window open, say — this is where a typed model goes.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const FILE_NAME: &str = "data.json";

/// A data file larger than this is not something this app wrote.
const MAX_SIZE: u64 = 32 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("could not read saved data: {0}")]
    Read(String),
    #[error("could not save data: {0}")]
    Write(String),
    #[error("the saved data is not valid JSON")]
    Corrupt,
}

pub fn data_path(dir: &Path) -> PathBuf {
    dir.join(FILE_NAME)
}

/// Creates the directory if needed and narrows it to the owner.
fn ensure_dir(dir: &Path) -> Result<(), StorageError> {
    fs::create_dir_all(dir).map_err(|e| StorageError::Write(e.to_string()))?;
    #[cfg(unix)]
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .map_err(|e| StorageError::Write(e.to_string()))?;
    Ok(())
}

/// Narrows a data file that is readable by anyone else on the machine.
///
/// The app writes 0600, but a file can arrive by other routes — a restored backup, a copy made with
/// a permissive umask, an older version that did not set this. Since the contents are credentials,
/// finding one exposed and leaving it that way would be the wrong choice.
#[cfg(unix)]
fn tighten(path: &Path, meta: &fs::Metadata) {
    let mode = meta.permissions().mode() & 0o777;
    if mode & 0o077 == 0 {
        return;
    }
    match fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
        Ok(()) => log::warn!(
            "{} was mode {mode:o}, readable by others; narrowed it to 600",
            path.display()
        ),
        Err(e) => log::error!("{} is mode {mode:o} and could not be narrowed: {e}", path.display()),
    }
}

#[cfg(not(unix))]
fn tighten(_path: &Path, _meta: &fs::Metadata) {}

/// Reads the saved data, or `None` on a first run.
///
/// A corrupt file is reported rather than silently discarded: losing a server list without saying
/// so is worse than refusing to start with it, and the file is still there to recover by hand.
pub fn load(dir: &Path) -> Result<Option<String>, StorageError> {
    let path = data_path(dir);

    let meta = match fs::metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(StorageError::Read(e.to_string())),
    };

    if meta.len() > MAX_SIZE {
        return Err(StorageError::Read(format!(
            "{} is {} bytes, which is far larger than this app writes",
            path.display(),
            meta.len()
        )));
    }

    tighten(&path, &meta);

    let text = fs::read_to_string(&path).map_err(|e| StorageError::Read(e.to_string()))?;
    if text.trim().is_empty() {
        return Ok(None);
    }

    // Validated here so a damaged file is caught at load rather than confusing the frontend.
    serde_json::from_str::<serde_json::Value>(&text).map_err(|_| StorageError::Corrupt)?;

    Ok(Some(text))
}

/// Writes the data, replacing any previous copy atomically.
///
/// The write goes to a temporary file in the same directory, is flushed to disk, and is then
/// renamed over the target. Writing in place would leave the file truncated if the process died
/// mid-write, and a VPN client that loses every server on an unlucky crash is not one anybody keeps.
pub fn save(dir: &Path, json: &str) -> Result<(), StorageError> {
    serde_json::from_str::<serde_json::Value>(json).map_err(|_| StorageError::Corrupt)?;

    ensure_dir(dir)?;
    let target = data_path(dir);
    // Same directory, so the rename stays on one filesystem and is therefore atomic.
    let temp = dir.join(format!("{FILE_NAME}.{}.tmp", std::process::id()));

    let write = || -> std::io::Result<()> {
        let mut file = fs::File::create(&temp)?;
        #[cfg(unix)]
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(json.as_bytes())?;
        // Without this the rename can land before the contents do, leaving an empty file after a
        // power loss.
        file.sync_all()?;
        Ok(())
    };

    if let Err(e) = write() {
        let _ = fs::remove_file(&temp);
        return Err(StorageError::Write(e.to_string()));
    }

    fs::rename(&temp, &target).map_err(|e| {
        let _ = fs::remove_file(&temp);
        StorageError::Write(e.to_string())
    })?;

    Ok(())
}

#[cfg(test)]
mod tests;
