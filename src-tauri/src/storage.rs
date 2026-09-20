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
mod tests {
    use super::*;

    /// A directory unique to one test.
    ///
    /// Time alone is not enough: these run in parallel threads of one process, and two of them can
    /// read the same nanosecond, land on the same directory and delete each other's files. The
    /// counter is what actually guarantees uniqueness.
    fn scratch() -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);

        let p = std::env::temp_dir().join(format!(
            "nunya-storage-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        fs::remove_dir_all(&p).ok();
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn a_first_run_has_nothing_saved() {
        let dir = scratch();
        assert_eq!(load(&dir).unwrap(), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn saves_and_reads_back() {
        let dir = scratch();
        save(&dir, r#"{"servers":[]}"#).unwrap();
        assert_eq!(load(&dir).unwrap().as_deref(), Some(r#"{"servers":[]}"#));
        fs::remove_dir_all(&dir).ok();
    }

    /// The file holds credentials, so nobody else on the machine should be able to read it.
    #[cfg(unix)]
    #[test]
    fn the_data_file_and_its_directory_are_owner_only() {
        let dir = scratch();
        save(&dir, "{}").unwrap();

        let file_mode = fs::metadata(data_path(&dir)).unwrap().permissions().mode() & 0o777;
        assert_eq!(file_mode, 0o600, "data file is {file_mode:o}, expected 600");

        let dir_mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "directory is {dir_mode:o}, expected 700");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_failed_write_leaves_no_temporary_file_behind() {
        let dir = scratch();
        // Invalid JSON is rejected before anything is created.
        assert!(save(&dir, "not json").is_err());

        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "left {} temp files", leftovers.len());

        fs::remove_dir_all(&dir).ok();
    }

    /// Replacing must never expose a partial file: the previous contents stay readable right up to
    /// the rename.
    #[test]
    fn overwriting_keeps_the_file_valid_throughout() {
        let dir = scratch();
        save(&dir, r#"{"v":1}"#).unwrap();
        save(&dir, r#"{"v":2}"#).unwrap();
        assert_eq!(load(&dir).unwrap().as_deref(), Some(r#"{"v":2}"#));

        // And only one file remains; the temporary was renamed, not left alongside.
        let count = fs::read_dir(&dir).unwrap().count();
        assert_eq!(count, 1, "expected just the data file");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_data_is_reported_rather_than_silently_dropped() {
        let dir = scratch();
        fs::write(data_path(&dir), "{ this is not json").unwrap();

        let err = load(&dir).unwrap_err();
        assert!(matches!(err, StorageError::Corrupt));
        // The file is still on disk to recover by hand.
        assert!(data_path(&dir).exists());

        fs::remove_dir_all(&dir).ok();
    }

    /// A file that arrives world-readable — from a backup, say — gets narrowed on the way in
    /// rather than left exposed.
    #[cfg(unix)]
    #[test]
    fn an_exposed_data_file_is_narrowed_when_read() {
        let dir = scratch();
        let path = data_path(&dir);
        fs::write(&path, r#"{"servers":[]}"#).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();

        // The read still succeeds; it is the permissions that change.
        assert!(load(&dir).unwrap().is_some());

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "file is still {mode:o}");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_file_reads_as_a_first_run() {
        let dir = scratch();
        fs::write(data_path(&dir), "   ").unwrap();
        assert_eq!(load(&dir).unwrap(), None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_absurdly_large_file_is_refused_without_reading_it() {
        let dir = scratch();
        let path = data_path(&dir);
        let file = fs::File::create(&path).unwrap();
        file.set_len(MAX_SIZE + 1).unwrap();

        let err = load(&dir).unwrap_err();
        assert!(matches!(err, StorageError::Read(_)), "{err}");

        fs::remove_dir_all(&dir).ok();
    }
}
