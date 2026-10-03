//! What differs between macOS, Linux and Windows, behind one interface.
//!
//! Each operating system has its own file here, and only the target's is compiled: shared code
//! calls `platform::…` and never asks which system it is on. The alternative, `#[cfg]` lines inside
//! shared functions, is what the rest of the tree still does, and it means reading every file to
//! learn what one platform does, and stepping around two platforms' branches to change the third.
//! Each file must provide every item `pub use`d below, so a platform left behind is a compile
//! error on that platform's CI job rather than a gap found by a user.
//!
//! Moved here so far: VPN-mode privilege, owner-only files, the system browser, the core's
//! console window, the core's link (its endpoint and who is on the other end of it), the tray, the
//! TUN's name, which tool sets the system proxy, and what only macOS has: the menu-bar popover,
//! the Dock's reopen, and the NetworkExtension transport; and which system this is, for the
//! frontend (`OS`, `webview_plugin`). The rest moves in one concern at a time (issue #100).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

#[cfg_attr(target_os = "macos", path = "macos.rs")]
#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(windows, path = "windows.rs")]
mod imp;

/// What macOS and Linux share; only their files use it.
#[cfg(unix)]
mod unix;

/// The tray on macOS and Windows, which both use tray-icon's item.
#[cfg(not(target_os = "linux"))]
mod tray_icon;

/// Linux's own StatusNotifierItem, in place of tray-icon's.
#[cfg(target_os = "linux")]
mod sni;

/// The menu-bar popover, which takes the tray menu's place on macOS.
#[cfg(target_os = "macos")]
mod popover;

/// The packet tunnel extension's control plane (the `.appex` itself is Swift).
#[cfg(target_os = "macos")]
mod network_extension;

/// What the "Allow VPN mode" sheet says on this platform, or `None` where the app has no way to
/// obtain privilege for VPN mode and must not offer a button that always fails.
pub use imp::GRANT;

/// Obtains privilege for VPN mode (blocking: it waits on the system's own prompt). `core` is the
/// bundled core binary. Called only where `GRANT` is `Some`.
pub use imp::grant;

/// Narrows a directory to its owner. A no-op on Windows, where a profile's AppData is already
/// private to its user and there is no mode to set.
pub use imp::restrict_dir;

/// Narrows a file just created to its owner, before anything is written to it.
pub use imp::restrict_file;

/// Narrows an existing file that anyone else on the machine can read, saying so in the log.
pub use imp::tighten;

/// The desktop's own command for opening `url` in the system browser, not yet started.
pub use imp::browser_command;

/// Starts a desktop tool (`gsettings`, `xdg-open`, …) in the desktop's own environment, undoing
/// what a bundle's launcher set for the app itself — the AppImage's libraries, on Linux.
pub use imp::host_environment;

/// Keeps a console program started from this GUI app from opening a window of its own. Takes the
/// std command; a tokio one hands its own over with `as_std_mut`.
pub use imp::no_console_window;

/// The TUN interface's name, or `None` to let the system number it (macOS's utunN).
pub use imp::TUN_NAME;

/// Which tool sets the system proxy here. On Linux it depends on the running desktop, so this
/// may fail there, by name, rather than at compile time.
pub use imp::proxy_desktop;

/// The core link's listener between `ipc_bind`, which runs outside the async runtime, and
/// `IpcAcceptor::adopt`, which runs on it: a std unix listener, or a named pipe's name.
pub use imp::PendingListener;

/// One accepted connection from the core: a unix socket, or a named pipe instance.
pub use imp::IpcStream;

/// Hands out connections to the core endpoint, one at a time (`adopt`, then `accept`).
pub use imp::IpcAcceptor;

/// Makes the endpoint the core dials, owner-only where it is a file. Returns the address to give
/// the core: the socket's path, or on Windows the pipe's name (the path given only names the
/// directory there).
pub use imp::ipc_bind;

/// Removes what `ipc_bind` left on disk, if anything.
pub use imp::ipc_unbind;

/// Rejects a peer that runs as another, unprivileged user. Where the platform has no such notion
/// for a pipe, the pid check that follows is the whole identity.
pub use imp::peer_user_ok;

/// The pid of the process on the other end of the link.
pub use imp::peer_pid;

/// The tray, in managed state once the first status built it (`tray::is_up`).
pub use imp::Tray;

/// Puts what the frontend reported into the platform's tray, creating the tray the first time.
pub use imp::tray_mirror;

/// Hides the menu-bar popover, where there is one.
pub use imp::hide_popover;

/// Whatever the platform does with an application event beyond what `lib.rs` does everywhere.
pub use imp::on_run_event;

/// Whether this platform has a NetworkExtension transport to select (`transport::select`).
pub use imp::NETWORK_EXTENSION;

/// That transport, where `NETWORK_EXTENSION` is true.
pub use imp::network_extension_transport;

/// `macos`, `linux` or `windows`: the one platform fact the frontend is told (`src/platform.ts`),
/// so it keys its own per-platform values on this rather than on the webview's guess.
pub use imp::OS;

/// Puts `OS` into every webview as `window.__NUNYA_OS__` before the page's own script runs, so the
/// frontend reads it synchronously, at import, where its constants are. A command would be
/// asynchronous, and needs a capability; this needs neither.
pub fn webview_plugin() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    tauri::plugin::Builder::new("nunya-platform")
        .js_init_script(format!("window.__NUNYA_OS__ = {OS:?};"))
        .build()
}

/// Why `grant` cannot be offered on this machine, where that is known before asking (Linux's
/// AppImage, a development build), so the status card says so rather than offering a button
/// that always fails. `None` where the grant may be tried.
pub use imp::grant_blocked;

/// Adjusts the process before the webview starts, where the platform's webview needs it (Linux,
/// Wayland on NVIDIA). Called first thing in `run`, while the process has one thread.
pub use imp::before_webview;

/// What the caller has to do once `grant` has succeeded. Each platform builds only the variant
/// its grant produces, hence the allowance.
#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
pub enum Granted {
    /// The core binary now carries the privilege; the running one was started without it.
    RestartCore,
    /// A privileged copy of the whole app has been started; this one should quit.
    Relaunched,
}

/// The sheet's words, as data, so the view is the same on every platform and the platform's
/// facts (who asks, how often, what changes) live beside the code that makes them true.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantCopy {
    pub lede: &'static str,
    pub why: &'static str,
    pub facts: &'static [Fact],
    /// The button's label while the system's prompt is up.
    pub waiting: &'static str,
    /// Why proxy mode is the lighter choice here.
    pub alt: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Fact {
    pub title: &'static str,
    pub text: &'static str,
}

/// Pins every function's signature, so each platform's file has to match it and not merely name it.
#[allow(dead_code)]
fn _signature_check(
    core: &Path,
    file: &fs::File,
    meta: &fs::Metadata,
    child: &mut Command,
) -> Result<Granted, String> {
    let _: io::Result<()> = restrict_dir(core);
    let _: io::Result<()> = restrict_file(file);
    tighten(core, meta);
    let mut browser: Command = browser_command("");
    host_environment(&mut browser);
    no_console_window(child);
    let _: Option<&'static str> = TUN_NAME;
    let _: Option<String> = grant_blocked(core);
    before_webview();
    let _: Result<crate::sysproxy::Desktop, String> = proxy_desktop();
    let _: io::Result<(PathBuf, PendingListener)> = ipc_bind(core.to_path_buf());
    ipc_unbind(core);
    grant(core)
}

/// The same, for the tray.
#[allow(dead_code)]
fn _tray_signature_check(
    app: &tauri::AppHandle,
    lines: &crate::tray::Lines,
    icon: crate::tray::Pixels,
) -> Result<(), String> {
    hide_popover(app);
    let _: bool = NETWORK_EXTENSION;
    let _: &'static str = OS;
    let _: Option<std::sync::Arc<dyn crate::transport::TunnelTransport>> = network_extension_transport();
    tray_mirror(app, lines, icon, "")
}

#[allow(dead_code)]
fn _event_signature_check(app: &tauri::AppHandle, event: &tauri::RunEvent) {
    on_run_event(app, event)
}

/// The same, for the link's types and its async half.
#[allow(dead_code)]
async fn _ipc_signature_check(
    listener: PendingListener,
    stream: &IpcStream,
) -> io::Result<IpcStream> {
    let _: Result<(), String> = peer_user_ok(stream);
    let _: io::Result<u32> = peer_pid(stream);
    let mut acceptor: IpcAcceptor = IpcAcceptor::adopt(listener)?;
    acceptor.accept().await
}

#[cfg(test)]
mod tests;
