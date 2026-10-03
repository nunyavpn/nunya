//! What macOS and Linux do the same way, re-exported by each of their files.
//!
//! Shared here rather than written twice, because both are the same POSIX calls; each platform's
//! file still names every item it provides, so reading `linux.rs` alone says what Linux does.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

pub fn restrict_dir(dir: &Path) -> io::Result<()> {
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
}

pub fn restrict_file(file: &fs::File) -> io::Result<()> {
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

/// The app writes 0600, but a file can arrive by other routes — a restored backup, a copy made with
/// a permissive umask, an older version that did not set this. Since the contents are credentials,
/// finding one exposed and leaving it that way would be the wrong choice.
pub fn tighten(path: &Path, meta: &fs::Metadata) {
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

/// Nothing to do: a unix process has no console window of its own to suppress.
pub fn no_console_window(_cmd: &mut tokio::process::Command) {}
