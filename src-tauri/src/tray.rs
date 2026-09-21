//! The top-bar icon on Linux: what the tunnel is doing, and a way to change it without the window.
//!
//! A tunnel is a background thing, and a client whose only control is a 1040px window forces a
//! choice between leaving that window open all day and closing it — which, before this, quit the
//! app and tore the tunnel down with it. The icon lets the window go away while the tunnel stays.
//!
//! The menu owns no connection logic. Connecting needs the selected server, the bypass rules and
//! the settings, all of which live in the frontend store, so "Connect" only emits `tray-toggle` and
//! the frontend runs the same `toggleConnection` the status card does. The frontend in turn reports
//! every state change through `set_tray_status`. Building a `BuildRequest` here instead would be a
//! second copy of that logic, and the two would drift.
//!
//! Everything is a menu item because a Linux tray (StatusNotifierItem via libayatana-appindicator)
//! delivers no click events on the icon itself: a left click opens the menu, and that is all.
//!
//! Linux only. macOS gets the menu-bar popover, which is a different piece of UI, and building a
//! plain tray there first would leave two things to reconcile.

use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, Wry};

/// The items whose text changes. Kept in managed state so `set_tray_status` can reach them.
pub struct Tray {
    server: MenuItem<Wry>,
    status: MenuItem<Wry>,
    toggle: MenuItem<Wry>,
}

const TOGGLE: &str = "toggle";
const SHOW: &str = "show";
const QUIT: &str = "quit";

pub fn install(app: &AppHandle) -> tauri::Result<()> {
    // Disabled: these are labels, and a clickable line invites a click that does nothing.
    // The server is its own line because it matters most when disconnected — it is what
    // Connect will connect to.
    let server = MenuItem::with_id(app, "server", "No server selected", false, None::<&str>)?;
    let status = MenuItem::with_id(app, "status", "Status: Disconnected", false, None::<&str>)?;
    // Disabled until the frontend has said a connect could work; see `set_tray_status`.
    let toggle = MenuItem::with_id(app, TOGGLE, "Connect", false, None::<&str>)?;
    let show = MenuItem::with_id(app, SHOW, "Show Nunya", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, QUIT, "Quit Nunya", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &server,
            &status,
            &PredefinedMenuItem::separator(app)?,
            &toggle,
            &PredefinedMenuItem::separator(app)?,
            &show,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .tooltip("Nunya")
        .on_menu_event(|app, event| match event.id().as_ref() {
            TOGGLE => {
                let _ = app.emit("tray-toggle", ());
            }
            SHOW => show_window(app),
            // Goes through `ExitRequested`, so the core is stopped and the routes given back
            // exactly as when the last window closes.
            QUIT => app.exit(0),
            _ => {}
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;

    app.manage(Tray { server, status, toggle });
    Ok(())
}

pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Mirrors the frontend's connection state into the menu.
///
/// `state` is the frontend's `ConnectionState` (`off`, `connecting`, `on`), `server` is the
/// selected server's name as the list shows it, `detail` is anything the status line must add
/// (proxy mode's listen address), and `can_connect` is false whenever the status card would refuse
/// Connect — no server, no core — so the menu never offers a button the window would turn down.
#[tauri::command]
pub fn set_tray_status(
    app: AppHandle,
    state: String,
    server: Option<String>,
    detail: Option<String>,
    can_connect: bool,
) -> Result<(), String> {
    let Some(tray) = app.try_state::<Tray>() else {
        // No tray on this platform, or it failed to come up; the window is still the whole UI.
        return Ok(());
    };
    let (status_text, toggle_text, toggle_enabled) = match state.as_str() {
        "on" => ("Connected", "Disconnect", true),
        "connecting" => ("Connecting…", "Connect", false),
        _ => ("Disconnected", "Connect", can_connect),
    };
    let status = match detail {
        Some(d) => format!("Status: {status_text} · {d}"),
        None => format!("Status: {status_text}"),
    };
    let server = match &server {
        Some(name) => format!("Server: {name}"),
        None => "No server selected".to_string(),
    };
    tray.server.set_text(&server).map_err(|e| e.to_string())?;
    tray.status.set_text(&status).map_err(|e| e.to_string())?;
    tray.toggle.set_text(toggle_text).map_err(|e| e.to_string())?;
    tray.toggle
        .set_enabled(toggle_enabled)
        .map_err(|e| e.to_string())?;
    if let Some(icon) = app.tray_by_id("main") {
        let _ = icon.set_tooltip(Some(format!("Nunya — {status_text}")));
    }
    Ok(())
}
