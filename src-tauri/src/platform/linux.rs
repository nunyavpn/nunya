//! Linux: no way yet for the app to obtain privilege for VPN mode.
//!
//! The core needs `CAP_NET_ADMIN` for a TUN. An AppImage runs from a read-only, nosuid mount, so
//! neither a capability nor setuid can be put on the core inside it, and the release core's parent
//! check forbids running a copy of it from anywhere else. Until that is designed, VPN mode is not
//! offered a grant here, and proxy mode is the Linux build's mode.

use std::io;
use std::os::unix::io::RawFd;
use std::path::Path;
use std::process::Command;

pub use super::unix::{
    ipc_bind, ipc_unbind, no_console_window, peer_pid, peer_user_ok, restrict_dir, restrict_file,
    tighten, IpcAcceptor, IpcStream, PendingListener,
};
use super::{GrantCopy, Granted};

pub static GRANT: Option<GrantCopy> = None;

pub fn grant(_core: &Path) -> Result<Granted, String> {
    Err("this build cannot obtain privilege for VPN mode on Linux yet; use proxy mode".into())
}

pub fn browser_command(url: &str) -> Command {
    let mut c = Command::new("xdg-open");
    c.arg(url);
    c
}

/// The peer's credentials, as the kernel recorded them when the socket was connected. Linux has
/// no `getpeereid` and answers pid, uid and gid from this one option instead.
fn peer_cred(fd: RawFd) -> io::Result<libc::ucred> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: fd is live; `cred` and `len` are valid out-params sized for the option.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred)
}

pub(super) fn uid_of_peer(fd: RawFd) -> io::Result<u32> {
    Ok(peer_cred(fd)?.uid)
}

pub(super) fn pid_of_peer(fd: RawFd) -> io::Result<u32> {
    Ok(peer_cred(fd)?.pid as u32)
}
