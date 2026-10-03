//! What Android and iOS do the same way, re-exported by each of their files.
//!
//! **How the core is hosted.** A phone app cannot start a program of its own, so the core is not a
//! child process here: it is nunya-core's `mobile` package, built by gomobile into the app (an
//! `.aar` on Android, an `.xcframework` on iOS) and reached through the app's native half — the
//! Kotlin or Swift side of the `nunya-platform` plugin. The calls are the desktop's exactly: a
//! method name from `rpc::method` and a protobuf payload each way, which the core's `mobile.Call`
//! answers with the same handlers its socket does. So everything above `Engine` is unchanged.
//!
//! The desktop's peer check has nothing to guard here. It exists because a local process could
//! win the race to the socket and pose as the core; in-process there is no socket to win, and on
//! iOS the app reaches its extension only through the system's own channel.
//!
//! A *session* is one core instance. The app's own is session 0; a probe opens another for as long
//! as it runs, in the same process, where the desktop starts a scratch process.
//!
//! **What a phone does not have** is answered here as "not here", by name: no tray (the tunnel's
//! notification takes its place on Android), no popover, no system proxy, no in-app updates (the
//! store or the system installer updates the app), and no window to un-minimise.

use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tauri::plugin::PluginHandle;
use tauri::{AppHandle, Manager, Wry};

use crate::rpc::LinkError;

/// The native half of the `nunya-platform` plugin, kept in managed state once its `setup` has
/// registered it. Absent where the native half is not built yet; `Engine` then says so.
pub struct Native(pub PluginHandle<Wry>);

/// The session the app's own core runs in.
const MAIN: u32 = 0;

/// One core instance in this process.
pub struct Engine {
    native: Option<PluginHandle<Wry>>,
    session: u32,
}

#[derive(Serialize)]
struct CallArgs<'a> {
    session: u32,
    method: &'a str,
    /// The protobuf request, base64: the bridge carries JSON.
    payload: String,
}

#[derive(Deserialize)]
struct Reply {
    /// The protobuf reply, base64.
    payload: String,
}

#[derive(Serialize, Deserialize)]
struct Session {
    session: u32,
}

#[derive(Deserialize)]
struct Loaded {
    loaded: bool,
}

impl Engine {
    /// The app's own core. Nothing starts here: the library is loaded with the app, and the core
    /// it holds starts a tunnel only when told to, as the desktop's does.
    pub fn launch(app: &AppHandle) -> Result<Arc<Engine>, String> {
        let native = app.try_state::<Native>().map(|n| n.0.clone());
        if native.is_none() {
            log::warn!("the core's native half is not part of this build; nothing can connect");
        }
        Ok(Arc::new(Engine { native, session: MAIN }))
    }

    /// A second instance, for one probe run.
    pub async fn scratch(&self) -> Result<Engine, String> {
        let native = self.native()?;
        let opened: Session = native
            .run_mobile_plugin_async("open", ())
            .await
            .map_err(|e| format!("could not open a probe instance: {e}"))?;
        Ok(Engine { native: Some(native.clone()), session: opened.session })
    }

    pub async fn call<Req, Resp>(&self, method: &str, req: &Req) -> Result<Resp, LinkError>
    where
        Req: prost::Message,
        Resp: prost::Message + Default,
    {
        let native = self.native().map_err(LinkError::Core)?;
        let codec = base64::engine::general_purpose::STANDARD;
        let reply: Reply = native
            .run_mobile_plugin_async(
                "call",
                CallArgs {
                    session: self.session,
                    method,
                    payload: codec.encode(req.encode_to_vec()),
                },
            )
            .await
            .map_err(|e| LinkError::Core(e.to_string()))?;
        let bytes = codec
            .decode(reply.payload)
            .map_err(|e| LinkError::Core(format!("the core's reply was not base64: {e}")))?;
        Ok(Resp::decode(bytes.as_slice())?)
    }

    pub async fn is_connected(&self) -> bool {
        let Ok(native) = self.native() else {
            return false;
        };
        native
            .run_mobile_plugin_async::<Loaded>("loaded", ())
            .await
            .is_ok_and(|l| l.loaded)
    }

    /// Nothing to restart: the core is a library in this process, and no grant or update here
    /// replaces it while the app runs.
    pub async fn restart(&self) -> Result<u32, String> {
        Err("the core runs inside the app here and is not restarted".into())
    }

    /// A probe's instance is closed; the app's own stays loaded with the app.
    pub async fn stop(&self) {
        if self.session == MAIN {
            return;
        }
        if let Ok(native) = self.native() {
            let _ = native
                .run_mobile_plugin_async::<()>("close", Session { session: self.session })
                .await;
        }
    }

    pub async fn shut_down(&self) {
        self.stop().await;
    }

    /// There is no binary to give a privilege to: VPN mode is the system's consent here, not a
    /// file permission. Only the desktop grant reads this.
    pub fn binary(&self) -> &Path {
        Path::new("")
    }

    fn native(&self) -> Result<&PluginHandle<Wry>, String> {
        self.native
            .as_ref()
            .ok_or_else(|| "the core is not part of this build yet".into())
    }
}

/// No window to bring back: the app is the screen.
pub fn show_window(_app: &AppHandle) {}

/// No tray: on Android the tunnel's own notification shows its state.
pub struct Tray;

pub fn tray_mirror(
    _app: &AppHandle,
    _lines: &crate::tray::Lines,
    _icon: crate::tray::Pixels,
    _label: &str,
) -> Result<(), String> {
    Ok(())
}

pub fn hide_popover(_app: &AppHandle) {}

pub fn on_run_event(_app: &AppHandle, _event: &tauri::RunEvent) {}

pub fn before_webview() {}

pub const NETWORK_EXTENSION: bool = false;

pub fn network_extension_transport() -> Option<Arc<dyn crate::transport::TunnelTransport>> {
    None
}

/// The store, or the system's installer, updates the app.
pub const IN_APP_UPDATES: bool = false;

/// There is no system proxy for an app to set on a phone; proxy mode covers apps pointed at it.
pub fn proxy_desktops() -> Result<Vec<crate::sysproxy::Desktop>, String> {
    Err("a phone has no system proxy for Nunya to set".into())
}

/// The socket link is never accepted on a phone, so there is no peer to ask about; these exist
/// only so `unix.rs`'s half of the link compiles, and refuse by name if anything reaches them.
pub(super) fn pid_of_peer(_fd: std::os::unix::io::RawFd) -> std::io::Result<u32> {
    Err(no_socket_link())
}

pub(super) fn uid_of_peer(_fd: std::os::unix::io::RawFd) -> std::io::Result<u32> {
    Err(no_socket_link())
}

fn no_socket_link() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "a phone hosts the core in-process; there is no socket link to check",
    )
}
