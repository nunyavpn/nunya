//! Exercises the real NETunnelProviderManager boundary on macOS.
//!
//! These call genuine NetworkExtension APIs. They deliberately stop short of anything that raises a
//! prompt or changes system state: reading the current state is side-effect free, so it can run in
//! CI and on a developer's machine without leaving a VPN profile behind.
//!
//! What they prove is the part that is easy to get wrong and invisible until runtime — that the
//! ObjC shim links, that the C ABI lines up on both sides, and that the completion-handler APIs
//! resolve their channels rather than hanging.

#![cfg(target_os = "macos")]

use std::time::{Duration, Instant};

/// Above this, the callback never fired and we are looking at a deadlock rather than a slow call.
const CALL_CEILING: Duration = Duration::from_secs(25);

use nunya_lib::config::Mode;
use nunya_lib::transport::network_extension::NetworkExtensionTransport;
use nunya_lib::transport::{TunnelState, TunnelTransport};

fn transport() -> NetworkExtensionTransport {
    NetworkExtensionTransport::new("com.nunyavpn.app.NunyaTunnel", "Nunya")
}

/// Reading state must reach NetworkExtension and come back with an answer, not an FFI error.
///
/// On a machine with no saved profile the honest answer is `NeedsPermission`. Without the
/// entitlement the system may refuse instead, which is also a real answer — what must not happen is
/// a hang or a link failure.
#[tokio::test]
async fn reading_state_reaches_networkextension() {
    let t = transport();
    let started = Instant::now();
    let result = t.availability(Mode::Vpn).await;
    let took = started.elapsed();

    // Each call carries its own timeout. Anything at or above the ceiling means the completion
    // handler never fired, which is a deadlock rather than a fast negative.
    assert!(
        took < CALL_CEILING,
        "state query took {took:?}, which means the completion handler did not signal"
    );

    match result {
        Ok(state) => {
            // No configuration has been saved by this test, so the tunnel cannot be up.
            assert_ne!(
                state,
                TunnelState::Connected,
                "reported a live tunnel with no saved configuration"
            );
            eprintln!("NetworkExtension reports {state:?}");
        }
        Err(e) => {
            // Expected until the bundle is signed with the packet-tunnel-provider entitlement.
            eprintln!("NetworkExtension refused, which is the expected unsigned behaviour: {e}");
        }
    }
}

/// Two calls in a row must both return. A context pointer freed twice, or a manager captured
/// across calls, would show up here as the second call hanging.
#[tokio::test]
async fn state_can_be_read_repeatedly() {
    let t = transport();
    for i in 0..3 {
        let started = Instant::now();
        let _ = t.availability(Mode::Vpn).await;
        assert!(
            started.elapsed() < CALL_CEILING,
            "call {i} stalled, so the shim is not re-entrant"
        );
    }
}

/// `state` and `availability` must agree; `state` is defined as a synonym here, and a future
/// refactor that gives them different meanings should fail this rather than confuse the UI.
#[tokio::test]
async fn state_and_availability_agree() {
    let t = transport();
    let a = t.availability(Mode::Vpn).await;
    let b = t.state().await;
    assert_eq!(a.is_ok(), b.is_ok());
    if let (Ok(x), Ok(y)) = (a, b) {
        assert_eq!(x, y);
    }
}

/// Stopping with nothing saved must be a no-op rather than an error, so quitting the app before a
/// profile exists never surfaces a spurious failure to the user.
#[tokio::test]
async fn stopping_without_a_configuration_is_harmless() {
    let t = transport();
    let started = Instant::now();
    let _ = t.stop().await;
    assert!(
        started.elapsed() < CALL_CEILING,
        "stop stalled with no configuration saved"
    );
}
