//! iOS: the core in a packet tunnel extension, VPN mode through NetworkExtension.
//!
//! The core is nunya-core's `mobile` package as an `.xcframework`, linked into the extension the
//! system launches, which is the same provider the macOS packet tunnel needs. The app reaches it
//! through the Swift half of the `nunya-platform` plugin and the system's own channel
//! (`NETunnelProviderSession`), not a socket (`mobile.rs`). VPN mode is the system's consent to the
//! VPN configuration, so there is no grant here in the desktop's sense.
//!
//! Written and built, not released: shipping a VPN app needs an Apple Developer account enrolled
//! as an organization (issue #131). Until the Swift side lands, `native_setup` registers nothing,
//! and `Engine` says the core is not part of this build rather than pretending to connect.

use std::path::Path;

pub use super::mobile::{
    before_webview, hide_popover, network_extension_transport, on_run_event,
    proxy_desktops, show_window, tray_mirror, Engine, Tray, IN_APP_UPDATES, NETWORK_EXTENSION,
};
pub use super::unix::{
    ipc_bind, ipc_unbind, no_console_window, peer_pid, peer_user_ok, restrict_dir, restrict_file,
    tighten,
    IpcAcceptor, IpcStream, PendingListener,
};
pub(super) use super::mobile::{pid_of_peer, uid_of_peer};
use super::{GrantCopy, Granted};

pub const OS: &str = "ios";

/// The system's VPN consent is asked by the transport, not granted to a file.
pub static GRANT: Option<GrantCopy> = None;

pub fn grant(_core: &Path) -> Result<Granted, String> {
    Err("VPN mode is approved through the system's VPN prompt here".into())
}

pub fn grant_blocked(_core: &Path) -> Option<String> {
    None
}

/// The system numbers the utun it gives the extension.
pub const TUN_NAME: Option<&str> = None;

/// An app cannot start a browser by running a program; until the Swift side opens pages through
/// the system (#131), this refuses by name.
pub fn open_browser(_url: &str) -> Result<(), String> {
    Err("opening pages in the browser is not built for iOS yet".into())
}

/// Registers the Swift half of the plugin; nothing yet (#131).
pub fn native_setup(
    _app: &tauri::AppHandle,
    _api: tauri::plugin::PluginApi<tauri::Wry, ()>,
) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}
