//! Android: the core in-process, VPN mode through the system's `VpnService`.
//!
//! The core is nunya-core's `mobile` package as an `.aar`, reached through the Kotlin half of the
//! `nunya-platform` plugin (`mobile.rs` says why it is not a child process here). VPN mode is
//! Android's own: the system asks the user once whether Nunya may set up a VPN, creates the
//! interface itself, and hands the core its descriptor; no file permission is granted to anything,
//! so there is no grant here in the desktop's sense.
//!
//! Issue #130 builds the Kotlin side. Until it lands, `native_setup` registers nothing, and
//! `Engine` says the core is not part of this build rather than pretending to connect.

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

pub const OS: &str = "android";

/// The system's VPN consent is asked by the transport, not granted to a file.
pub static GRANT: Option<GrantCopy> = None;

pub fn grant(_core: &Path) -> Result<Granted, String> {
    Err("VPN mode is approved through Android's own prompt here".into())
}

pub fn grant_blocked(_core: &Path) -> Option<String> {
    None
}

/// The system names the interface the `VpnService` builds.
pub const TUN_NAME: Option<&str> = None;

/// An app cannot start a browser by running a program; until the Kotlin side opens pages with an
/// intent (#130), this refuses by name.
pub fn open_browser(_url: &str) -> Result<(), String> {
    Err("opening pages in the browser is not built for Android yet".into())
}

/// Registers the Kotlin half of the plugin; nothing yet (#130).
pub fn native_setup(
    _app: &tauri::AppHandle,
    _api: tauri::plugin::PluginApi<tauri::Wry, ()>,
) -> Result<(), Box<dyn std::error::Error>> {
    Ok(())
}
