//! What macOS and Linux do the same way, re-exported by each of their files.
//!
//! Shared here rather than written twice, because both are the same POSIX calls; each platform's
//! file still names every item it provides, so reading `linux.rs` alone says what Linux does.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};

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
pub fn no_console_window(_cmd: &mut std::process::Command) {}

// The core's link: a unix socket, which the GUI binds and the core dials (`rpc/link.rs`).

/// Bound before the async runtime exists, so it stays a std listener until `IpcAcceptor::adopt`.
pub type PendingListener = std::os::unix::net::UnixListener;

pub type IpcStream = tokio::net::UnixStream;

pub fn ipc_bind(socket_path: PathBuf) -> io::Result<(PathBuf, PendingListener)> {
    // A crashed run leaves the socket file behind and bind() would fail with EADDRINUSE.
    if socket_path.exists() {
        fs::remove_file(&socket_path)?;
    }
    let listener = PendingListener::bind(&socket_path)?;
    // Required before tokio can adopt it; a blocking accept would stall the runtime thread.
    listener.set_nonblocking(true)?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
    Ok((socket_path, listener))
}

/// Leaving the socket file behind would make the next run's bind() fail.
pub fn ipc_unbind(socket_path: &Path) {
    let _ = fs::remove_file(socket_path);
}

pub struct IpcAcceptor(tokio::net::UnixListener);

impl IpcAcceptor {
    pub fn adopt(listener: PendingListener) -> io::Result<Self> {
        tokio::net::UnixListener::from_std(listener).map(Self)
    }

    pub async fn accept(&mut self) -> io::Result<IpcStream> {
        Ok(self.0.accept().await?.0)
    }
}

/// Rejects a peer running as anyone but us or root.
///
/// The core may be running as root while we are not (the setuid core on macOS), so a bare
/// equality check is wrong. What must never happen is an *unprivileged* stranger connecting.
pub fn peer_user_ok(stream: &IpcStream) -> Result<(), String> {
    // SAFETY: geteuid cannot fail and touches no memory.
    let ours = unsafe { libc::geteuid() };
    let theirs = super::imp::uid_of_peer(stream.as_raw_fd())
        .map_err(|e| format!("cannot read peer uid: {e}"))?;
    if theirs != ours && theirs != 0 {
        return Err(format!("peer uid {theirs} is neither our own ({ours}) nor root"));
    }
    Ok(())
}

pub fn peer_pid(stream: &IpcStream) -> io::Result<u32> {
    super::imp::pid_of_peer(stream.as_raw_fd())
}
