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
//!
//! This file is the rule; reading the peer's credentials off a socket or a pipe is the platform's
//! (`platform::peer_user_ok`, `platform::peer_pid`). On Windows the link is a named pipe, and the
//! core checks it the same way from its side (`GetNamedPipeServerProcessId`,
//! `internal/ipc/ipc_windows.go`).

use crate::platform::{self, IpcStream};

/// Rejects a connection that is not the core process we spawned.
///
/// `expected_pid` of 0 means no core has been spawned yet, so nothing should be connecting at all.
pub fn verify(stream: &IpcStream, expected_pid: u32) -> Result<(), String> {
    platform::peer_user_ok(stream)?;
    if expected_pid == 0 {
        return Err(NOT_SPAWNED.to_string());
    }
    let pid = platform::peer_pid(stream).map_err(|e| format!("cannot read peer pid: {e}"))?;
    is_the_core(pid, expected_pid)
}

const NOT_SPAWNED: &str = "a peer connected before any core was spawned";

fn is_the_core(pid: u32, expected_pid: u32) -> Result<(), String> {
    if pid != expected_pid {
        return Err(format!(
            "peer pid {pid} is not the core we spawned ({expected_pid})"
        ));
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests;

#[cfg(all(test, windows))]
mod windows_tests;
