use super::*;
use tokio::net::UnixStream;

#[tokio::test]
async fn socketpair_peer_is_ourselves() {
    let (a, _b) = UnixStream::pair().unwrap();
    platform::peer_user_ok(&a).unwrap();

    let pid = platform::peer_pid(&a).unwrap();
    assert_eq!(pid, std::process::id());
}

#[tokio::test]
async fn rejects_a_peer_that_is_not_the_spawned_core() {
    let (a, _b) = UnixStream::pair().unwrap();
    // Our own pid is on the other end, so claiming to expect a different one must fail.
    let err = verify(&a, std::process::id() + 1).unwrap_err();
    assert!(err.contains("is not the core we spawned"), "{err}");
}

#[tokio::test]
async fn rejects_a_connection_before_any_spawn() {
    let (a, _b) = UnixStream::pair().unwrap();
    let err = verify(&a, 0).unwrap_err();
    assert!(err.contains("before any core was spawned"), "{err}");
}

#[tokio::test]
async fn accepts_the_expected_peer() {
    let (a, _b) = UnixStream::pair().unwrap();
    verify(&a, std::process::id()).unwrap();
}
