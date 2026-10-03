use super::*;
use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

/// A pipe with its client connected from this very process, so the expected pid is our own.
async fn connected_pipe() -> tokio::net::windows::named_pipe::NamedPipeServer {
    let name = format!(r"\\.\pipe\nunya-peer-test-{}", std::process::id());
    let server = ServerOptions::new().first_pipe_instance(true).create(&name).unwrap();
    let client = ClientOptions::new().open(&name).unwrap();
    server.connect().await.unwrap();
    // The client end is ours to keep open for as long as the server is looked at.
    std::mem::forget(client);
    server
}

#[tokio::test]
async fn a_pipe_client_is_identified_by_its_pid() {
    let server = connected_pipe().await;

    verify(&server, std::process::id()).unwrap();

    let err = verify(&server, std::process::id() + 1).unwrap_err();
    assert!(err.contains("is not the core we spawned"), "{err}");

    let err = verify(&server, 0).unwrap_err();
    assert!(err.contains("before any core was spawned"), "{err}");
}
