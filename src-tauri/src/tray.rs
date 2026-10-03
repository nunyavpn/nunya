//! The tray icon, in the macOS menu bar and the Linux top bar: what the tunnel is doing, and a way
//! to change it without the window.
//!
//! A tunnel is a background thing, and a client whose only control is a 1040px window forces a
//! choice between leaving that window open all day and closing it — which, before this, quit the
//! app and tore the tunnel down with it. The icon lets the window go away while the tunnel stays.
//!
//! **The icon is the state**, as OpenVPN's is: the app's mark in the rail shield's colours — green
//! when connected, amber while connecting, red when not working, dimmed when off. The frontend
//! paints it (`trayicon.ts`) from the mark's path and the rail's colour tokens and sends the pixels
//! with every status, so the bar and the window cannot disagree. Off comes marked as a template,
//! which macOS tints to the menu bar like the icons beside it.
//!
//! The menu owns no connection logic. Connecting needs the selected server, the bypass rules and
//! the settings, all of which live in the frontend store, so "Connect" only emits `tray-toggle` and
//! the frontend runs the same `toggleConnection` the status card does. The frontend in turn reports
//! every state change through `set_tray_status`. Building a `BuildRequest` here instead would be a
//! second copy of that logic, and the two would drift.
//!
//! **The first status creates the tray**, not startup. Until the frontend has painted there is no
//! right icon to show, and the app icon standing in would flash in the bar at every launch. Closing
//! the window hides it only once the tray is up (`is_up`), so a frontend that never reports leaves
//! the old behaviour — closing quits — rather than a hidden window with nothing to bring it back.
//!
//! On Linux everything is a menu item, because a tray there delivers no click events on the icon
//! itself: a click opens the menu, and that is all. The item is not tray-icon's, though —
//! `platform/sni.rs` publishes our own StatusNotifierItem, because libayatana's declares an
//! `Activate` method nothing answers, which costs GNOME a 400 ms double-click wait before it will
//! open the menu, and because it carries the icon as a file it deletes too early. That module's
//! header has the detail. On macOS the icon carries no menu at all; a click opens the popover
//! (`popover.rs`), which has everything the menu has. The macOS `install` says why there is no
//! menu as well.
//!
//! Windows keeps tray-icon's menu: it has neither problem, and a second implementation there would
//! be one more thing to keep in step for no gain.
//!
//! This file is what every platform shares — what the menu says (`Lines`), the icon's pixels, the
//! command. Building the tray and putting a state into it is the platform's (`platform::Tray`,
//! `platform::tray_mirror`): `linux.rs` for the StatusNotifierItem, `tray_icon.rs` for what macOS
//! and Windows share, and each of those two files for its own `install_tray`.

use serde::Deserialize;
use tauri::{AppHandle, Manager};

use crate::platform;

/// The icon as `trayicon.ts` painted it.
#[derive(Deserialize)]
pub struct Pixels {
    // Read by the desktop trays; a phone has no tray to paint, so its build reads none of these.
    #[allow(dead_code)]
    pub width: u32,
    #[allow(dead_code)]
    pub height: u32,
    /// Straight RGBA, row by row.
    #[allow(dead_code)]
    pub rgba: Vec<u8>,
    /// Draw it in the menu bar's own colour. macOS only; the frontend sets it for off. The Linux
    /// item has no equivalent — a shell there tints nothing — so it goes unread in that build.
    #[allow(dead_code)]
    pub template: bool,
}

/// Whether the tray is up, and closing the window can therefore hide it rather than quit.
pub fn is_up(app: &AppHandle) -> bool {
    app.try_state::<platform::Tray>().is_some()
}

/// What the menu says, worked out from what the frontend reported.
#[derive(Debug, PartialEq)]
pub struct Lines {
    pub server: String,
    pub status: String,
    pub toggle: &'static str,
    pub toggle_enabled: bool,
}

impl Lines {
    fn new(
        state: &str,
        tone: &str,
        server: Option<&str>,
        detail: Option<&str>,
        can_connect: bool,
    ) -> Self {
        let (word, toggle, toggle_enabled) = match state {
            "on" => ("Connected", "Disconnect", true),
            "connecting" => ("Connecting…", "Connect", false),
            // Not a second stop on top of the first; see `connect`/`disconnect` in `main.ts`.
            "disconnecting" => ("Disconnecting…", "Disconnect", false),
            _ => ("Disconnected", "Connect", can_connect),
        };
        // The icon is red for a tunnel that is up and carries nothing, and after an attempt that
        // failed; "Connected" or "Disconnected" beside it would contradict it. The toggle still
        // follows the connection: a dead tunnel is disconnected, a failed attempt retried.
        let word = if tone == "failed" { "Not working" } else { word };
        Lines {
            server: match server {
                Some(name) => format!("Server: {name}"),
                None => "No server selected".to_string(),
            },
            status: match detail {
                Some(d) => format!("Status: {word} · {d}"),
                None => format!("Status: {word}"),
            },
            toggle,
            toggle_enabled,
        }
    }
}

/// Mirrors the frontend's connection state into the tray, creating it the first time.
///
/// `state` is the frontend's `ConnectionState` (`off`, `connecting`, `on`, `disconnecting`) and
/// `tone` the rail shield's (`off`, `connecting`, `on`, `failed`); `server` is the selected
/// server's name as the list shows it, `detail` is anything the status line must add (proxy mode's
/// listen address), `label` is the shield's own description, used as the tooltip, and
/// `can_connect` is false whenever the status card would refuse Connect — no server, no core — so
/// the menu never offers a button the window would turn down.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub fn set_tray_status(
    app: AppHandle,
    state: String,
    tone: String,
    server: Option<String>,
    detail: Option<String>,
    label: String,
    can_connect: bool,
    icon: Pixels,
) -> Result<(), String> {
    let lines = Lines::new(
        &state,
        &tone,
        server.as_deref(),
        detail.as_deref(),
        can_connect,
    );
    platform::tray_mirror(&app, &lines, icon, &label)
}

/// The popover's Escape, and anything in it that hands off to somewhere else. The popover is
/// macOS's, but its commands are registered everywhere so `lib.rs` names them without a `cfg`;
/// elsewhere nothing calls them, and hiding a popover that does not exist does nothing.
#[tauri::command]
pub fn hide_popover(app: AppHandle) {
    platform::hide_popover(&app);
}

/// The popover's "Open Nunya".
#[tauri::command]
pub fn show_main_window(app: AppHandle) {
    platform::hide_popover(&app);
    platform::show_window(&app);
}

/// The popover's "Quit". Through `ExitRequested`, like the tray menu's, so the core is stopped and
/// the system proxy put back.
#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}

#[cfg(test)]
mod tests;
