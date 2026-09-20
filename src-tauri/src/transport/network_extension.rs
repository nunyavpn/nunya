//! The macOS transport: a `NEPacketTunnelProvider` extension bundled inside the app.
//!
//! This is how WireGuard, Mullvad, NordVPN and Tailscale all work on macOS, and it is why none of
//! them ask for a password to connect. The system owns the utun and launches the provider; the app
//! only saves a configuration and asks the system to start it.
//!
//! ```text
//! Nunya.app/Contents/
//! ├── MacOS/Nunya                     this process — UI only, unprivileged
//! └── PlugIns/NunyaTunnel.appex/      system-launched, holds the utun
//!     └── NunyaCore.xcframework       core/mobile, linked in
//! ```
//!
//! Control goes through `NETunnelProviderManager`, wrapped in `macos/tunnel_manager.m`. Saving a
//! configuration raises the standard "would like to add VPN configurations" prompt once, after
//! which the entry lives in System Settings › VPN. Runtime messages use `sendProviderMessage`,
//! which replaces the unix-socket RPC the subprocess transport uses — the extension is not our
//! child and has no socket of its own.
//!
//! # Why every call here is a channel
//!
//! NetworkExtension's completion handlers are delivered through the calling thread's run loop, so
//! blocking that thread to wait for one deadlocks. The C side therefore takes a callback, and each
//! Rust call resolves a oneshot. Each also carries its own timeout, so a handler that never fires
//! surfaces as an error instead of a future that never completes.
//!
//! # What is and is not working
//!
//! This control plane is built and linked. What it drives — the `.appex` — cannot be loaded until
//! the bundle is signed with `com.apple.developer.networking.networkextension`
//! (`packet-tunnel-provider`), which needs a paid Apple Developer membership. Until then these
//! calls reach real NetworkExtension APIs and report the system's own error, which is more useful
//! than a placeholder.

use std::ffi::{c_char, c_void, CStr, CString};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::oneshot;

use super::{Throughput, TransportError, TunnelState, TunnelTransport};
use crate::config::{self, BuildRequest};

/// Generous for a preferences read, short enough that a wedged call is reported rather than hung.
const CALL_TIMEOUT: Duration = Duration::from_secs(20);

const STATE_ERROR: i32 = -1;

#[link(name = "NetworkExtension", kind = "framework")]
extern "C" {}

type Callback = extern "C" fn(ctx: *mut c_void, code: i32, message: *const c_char);

extern "C" {
    fn nunya_ne_save_configuration(
        provider_bundle_id: *const c_char,
        display_name: *const c_char,
        server_address: *const c_char,
        config_json: *const c_char,
        ctx: *mut c_void,
        cb: Callback,
    );
    fn nunya_ne_state_now(ctx: *mut c_void, cb: Callback);
    fn nunya_ne_start(ctx: *mut c_void, cb: Callback);
    fn nunya_ne_stop(ctx: *mut c_void, cb: Callback);
    fn nunya_ne_send_message(message: *const c_char, ctx: *mut c_void, cb: Callback);
    fn nunya_ne_remove_configuration(ctx: *mut c_void, cb: Callback);
}

/// What the ObjC side hands back: a code, and a message that is either an error or a reply.
type Outcome = (i32, String);

/// Turns the C callback into a resolved channel.
///
/// # Safety
///
/// `ctx` must be the pointer produced by `Box::into_raw` in `call`, and the C side must invoke this
/// exactly once — which every path in `tunnel_manager.m` does, including its error paths.
extern "C" fn trampoline(ctx: *mut c_void, code: i32, message: *const c_char) {
    if ctx.is_null() {
        return;
    }
    // SAFETY: ownership of the box transfers back here, and this runs once per call.
    let tx = unsafe { Box::from_raw(ctx as *mut oneshot::Sender<Outcome>) };

    let text = if message.is_null() {
        String::new()
    } else {
        // SAFETY: the C side passes a NUL-terminated string valid for this call.
        unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned()
    };

    // A dropped receiver means the call timed out; nothing to do but discard the late answer.
    let _ = tx.send((code, text));
}

/// Drives one C call to completion.
async fn call<F>(start: F) -> Result<Outcome, TransportError>
where
    F: FnOnce(*mut c_void, Callback),
{
    let (tx, rx) = oneshot::channel::<Outcome>();
    let ctx = Box::into_raw(Box::new(tx)) as *mut c_void;

    start(ctx, trampoline);

    match tokio::time::timeout(CALL_TIMEOUT, rx).await {
        Ok(Ok(outcome)) => Ok(outcome),
        // The sender was dropped without sending, which would mean the C side leaked the context.
        Ok(Err(_)) => Err(TransportError::Core(
            "the tunnel manager dropped the request without answering".into(),
        )),
        Err(_) => Err(TransportError::Unavailable(
            "NetworkExtension did not answer; the VPN configuration may be unavailable to this \
             build (it needs the packet-tunnel-provider entitlement)"
                .into(),
        )),
    }
}

fn ok_or_err((code, message): Outcome) -> Result<(), TransportError> {
    if code == STATE_ERROR {
        if message.contains("permission") || message.contains("denied") {
            return Err(TransportError::PermissionDenied);
        }
        return Err(TransportError::Core(message));
    }
    Ok(())
}

pub struct NetworkExtensionTransport {
    provider_bundle_id: CString,
    display_name: CString,
}

impl NetworkExtensionTransport {
    pub fn new(provider_bundle_id: &str, display_name: &str) -> Self {
        Self {
            provider_bundle_id: CString::new(provider_bundle_id).expect("bundle id has no NUL"),
            display_name: CString::new(display_name).expect("display name has no NUL"),
        }
    }

    fn state_from_code(code: i32) -> TunnelState {
        match code {
            0 => TunnelState::Disconnected,
            1 => TunnelState::Connecting,
            2 => TunnelState::Connected,
            _ => TunnelState::NeedsPermission,
        }
    }

    /// Writes the configuration, creating the profile on first use.
    ///
    /// Carries the whole generated sing-box config, so it is rewritten before every connect rather
    /// than only once.
    async fn save(&self, server: &str, config_json: &str) -> Result<(), TransportError> {
        let server = CString::new(server)
            .map_err(|_| TransportError::Core("server address contains a NUL byte".into()))?;
        let config = CString::new(config_json)
            .map_err(|_| TransportError::Core("config contains a NUL byte".into()))?;

        let outcome = call(|ctx, cb| unsafe {
            nunya_ne_save_configuration(
                self.provider_bundle_id.as_ptr(),
                self.display_name.as_ptr(),
                server.as_ptr(),
                config.as_ptr(),
                ctx,
                cb,
            )
        })
        .await?;
        ok_or_err(outcome)
    }

    /// Removes the VPN configuration, so the entry disappears from System Settings.
    ///
    /// Worth offering explicitly: an app that leaves a VPN profile behind after the user is done
    /// with it has overstayed its welcome.
    pub async fn forget(&self) -> Result<(), TransportError> {
        let outcome = call(|ctx, cb| unsafe { nunya_ne_remove_configuration(ctx, cb) }).await?;
        ok_or_err(outcome)
    }
}

#[async_trait]
impl TunnelTransport for NetworkExtensionTransport {
    async fn availability(&self) -> Result<TunnelState, TransportError> {
        let (code, message) = call(|ctx, cb| unsafe { nunya_ne_state_now(ctx, cb) }).await?;
        if code == STATE_ERROR {
            return Err(TransportError::Unavailable(message));
        }
        Ok(Self::state_from_code(code))
    }

    /// Saves the VPN configuration, which is what raises the system prompt.
    ///
    /// There is no separate "ask for permission" API: in NetworkExtension, consent *is* the act of
    /// saving a configuration. A profile has to exist before the prompt can appear, so this writes
    /// a placeholder that `start` later replaces with the real configuration.
    async fn request_permission(&self) -> Result<(), TransportError> {
        self.save("Nunya", "{}").await
    }

    async fn start(&self, request: &BuildRequest) -> Result<(), TransportError> {
        let cfg = config::build(request).to_string();
        self.save(&request.profile.server, &cfg).await?;

        let outcome = call(|ctx, cb| unsafe { nunya_ne_start(ctx, cb) }).await?;
        ok_or_err(outcome)
    }

    async fn stop(&self) -> Result<(), TransportError> {
        let outcome = call(|ctx, cb| unsafe { nunya_ne_stop(ctx, cb) }).await?;
        ok_or_err(outcome)
    }

    async fn state(&self) -> Result<TunnelState, TransportError> {
        self.availability().await
    }

    async fn throughput(&self) -> Result<Throughput, TransportError> {
        let msg = CString::new(r#"{"kind":"stats"}"#).unwrap();
        let (code, reply) =
            call(|ctx, cb| unsafe { nunya_ne_send_message(msg.as_ptr(), ctx, cb) }).await?;

        if code == STATE_ERROR {
            return Err(TransportError::Core(reply));
        }
        if reply.is_empty() {
            return Ok(Throughput::default());
        }
        serde_json::from_str::<Throughput>(&reply)
            .map_err(|e| TransportError::Core(format!("bad reply from the provider: {e}")))
    }
}
