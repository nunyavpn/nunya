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
