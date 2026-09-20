//! Peer verification for the core IPC socket.
//!
//! The link is asymmetric: the GUI listens and the core dials in (`ConnectIPC` in
//! the core's `internal/ipc/ipc_unix.go`). The core already refuses to talk to a listener whose owning
//! process is not its own parent, by reading `LOCAL_PEERPID` off the socket. Nothing checked the
//! other direction, so any local process that won the race to our socket could impersonate the
//! core and feed the UI whatever connection state it liked — including a convincing "you're
//! protected" while the tunnel was never up.
//!
//! These checks close that gap. They are cheap and they run before a single byte is read.

use std::io;
use std::os::unix::io::RawFd;

/// `getsockopt` level for `AF_UNIX` socket options on Darwin.
#[cfg(target_os = "macos")]
const SOL_LOCAL: libc::c_int = 0;

/// Returns the pid of the process on the other end of a connected `AF_UNIX` socket.
#[cfg(target_os = "macos")]
const LOCAL_PEERPID: libc::c_int = 0x002;

/// Effective uid of the peer process.
///
/// `getpeereid` is a BSD interface. Linux has no such call and answers all three questions — pid,
/// uid and gid — from a single `SO_PEERCRED` option instead, which is why the two platforms split
/// here rather than sharing one implementation.
#[cfg(not(target_os = "linux"))]
pub fn peer_uid(fd: RawFd) -> io::Result<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: fd is a live connected AF_UNIX socket; both out-params are valid stack slots.
    let rc = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

/// The peer's credentials, as the kernel recorded them when the socket was connected.
#[cfg(target_os = "linux")]
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

#[cfg(target_os = "linux")]
pub fn peer_uid(fd: RawFd) -> io::Result<u32> {
    Ok(peer_cred(fd)?.uid)
}

/// Pid of the peer process.
#[cfg(target_os = "macos")]
pub fn peer_pid(fd: RawFd) -> io::Result<u32> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: fd is live; `pid` and `len` are valid out-params sized for the option.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            SOL_LOCAL,
            LOCAL_PEERPID,
            &mut pid as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(pid as u32)
}

/// Pid of the peer process, from the same `SO_PEERCRED` block as the uid.
#[cfg(target_os = "linux")]
pub fn peer_pid(fd: RawFd) -> io::Result<u32> {
    Ok(peer_cred(fd)?.pid as u32)
}

/// Rejects a connection that is not the core process we spawned.
///
/// `expected_pid` of 0 means no core has been spawned yet, so nothing should be connecting at all.
pub fn verify(fd: RawFd, expected_pid: u32) -> Result<(), String> {
    let ours = unsafe { libc::geteuid() };
    let theirs = peer_uid(fd).map_err(|e| format!("cannot read peer uid: {e}"))?;

    // The core may be running as root while we are not, so a bare equality check is wrong once the
    // privileged helper lands. What must never happen is an *unprivileged* stranger connecting.
    if theirs != ours && theirs != 0 {
        return Err(format!(
            "peer uid {theirs} is neither our own ({ours}) nor root"
        ));
    }

    if expected_pid == 0 {
        return Err("a peer connected before any core was spawned".to_string());
    }

    let pid = peer_pid(fd).map_err(|e| format!("cannot read peer pid: {e}"))?;
    if pid != expected_pid {
        return Err(format!(
            "peer pid {pid} is not the core we spawned ({expected_pid})"
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::UnixStream;

    #[test]
    fn socketpair_peer_is_ourselves() {
        let (a, _b) = UnixStream::pair().unwrap();
        let uid = peer_uid(a.as_raw_fd()).unwrap();
        assert_eq!(uid, unsafe { libc::geteuid() });

        let pid = peer_pid(a.as_raw_fd()).unwrap();
        assert_eq!(pid, std::process::id());
    }

    #[test]
    fn rejects_a_peer_that_is_not_the_spawned_core() {
        let (a, _b) = UnixStream::pair().unwrap();
        // Our own pid is on the other end, so claiming to expect a different one must fail.
        let err = verify(a.as_raw_fd(), std::process::id() + 1).unwrap_err();
        assert!(err.contains("is not the core we spawned"), "{err}");
    }

    #[test]
    fn rejects_a_connection_before_any_spawn() {
        let (a, _b) = UnixStream::pair().unwrap();
        let err = verify(a.as_raw_fd(), 0).unwrap_err();
        assert!(err.contains("before any core was spawned"), "{err}");
    }

    #[test]
    fn accepts_the_expected_peer() {
        let (a, _b) = UnixStream::pair().unwrap();
        verify(a.as_raw_fd(), std::process::id()).unwrap();
    }
}
