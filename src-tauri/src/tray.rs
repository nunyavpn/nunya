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
//! [`sni.rs`](crate::sni) publishes our own StatusNotifierItem, because libayatana's declares an
//! `Activate` method nothing answers, which costs GNOME a 400 ms double-click wait before it will
//! open the menu, and because it carries the icon as a file it deletes too early. That module's
//! header has the detail. On macOS the icon carries no menu at all; a click opens the popover
//! (`popover.rs`), which has everything the menu has. The macOS `install` says why there is no
//! menu as well.
//!
//! Windows keeps tray-icon's menu: it has neither problem, and a second implementation there would
//! be one more thing to keep in step for no gain.

use std::sync::OnceLock;

use serde::Deserialize;
#[cfg(not(target_os = "linux"))]
use tauri::image::Image;
#[cfg(not(target_os = "linux"))]
use tauri::menu::MenuItem;
#[cfg(all(not(target_os = "macos"), not(target_os = "linux")))]
use tauri::menu::{Menu, PredefinedMenuItem};
#[cfg(not(target_os = "linux"))]
use tauri::tray::{TrayIcon, TrayIconBuilder};
#[cfg(not(target_os = "macos"))]
use tauri::Emitter;
#[cfg(not(target_os = "linux"))]
use tauri::Wry;
use tauri::{AppHandle, Manager};

/// The icon, and the menu's lines where there is a menu; in managed state once the first status
/// built them.
#[cfg(not(target_os = "linux"))]
pub struct Tray {
    icon: TrayIcon<Wry>,
    /// `None` on macOS, where the popover takes the menu's place.
    menu: Option<MenuLines>,
}

/// The channel into the thread that owns the StatusNotifierItem. The item's state lives there, not
/// here, because libdbus is not thread-safe; see [`sni`](crate::sni).
#[cfg(target_os = "linux")]
pub struct Tray {
    updates: std::sync::mpsc::Sender<crate::sni::Update>,
}

/// The menu items whose text changes. Never built on macOS, where `Tray::menu` is always `None`.
#[cfg(not(target_os = "linux"))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
struct MenuLines {
    server: MenuItem<Wry>,
    status: MenuItem<Wry>,
    toggle: MenuItem<Wry>,
}

/// The icon as `trayicon.ts` painted it.
#[derive(Deserialize)]
pub struct Pixels {
    width: u32,
    height: u32,
    /// Straight RGBA, row by row.
    rgba: Vec<u8>,
    /// Draw it in the menu bar's own colour. macOS only; the frontend sets it for off. The Linux
    /// item has no equivalent — a shell there tints nothing — so it goes unread in this build.
    #[cfg_attr(target_os = "linux", allow(dead_code))]
    template: bool,
}

#[cfg(all(not(target_os = "macos"), not(target_os = "linux")))]
const TOGGLE: &str = "toggle";
#[cfg(all(not(target_os = "macos"), not(target_os = "linux")))]
const SHOW: &str = "show";
#[cfg(all(not(target_os = "macos"), not(target_os = "linux")))]
const QUIT: &str = "quit";

/// Whether the tray is up, and closing the window can therefore hide it rather than quit.
pub fn is_up(app: &AppHandle) -> bool {
    app.try_state::<Tray>().is_some()
}

/// Set once the tray has been found not to work here, so it is not attempted again.
static NO_TRAY: OnceLock<()> = OnceLock::new();

/// Whether a tray is worth attempting at all.
///
/// On Linux this used to dlopen libayatana, which tray-icon loads lazily and which panics when it is
/// missing. Nothing loads it any more — [`sni`](crate::sni) speaks the protocol directly — so the
/// only real question is whether a watcher answers, which `install` finds out by asking. A failure
/// is remembered rather than retried: the frontend reports a status every few seconds, and each
/// attempt would wait on the bus before failing the same way.
fn supported() -> bool {
    NO_TRAY.get().is_none()
}

/// Remembers that there is no tray here, so `set_tray_status` stops trying.
fn give_up() {
    let _ = NO_TRAY.set(());
}

/// Starts the StatusNotifierItem and the thread that answers its menu.
///
/// The actions come back on a channel rather than through a callback because the item's thread must
/// not block on Tauri: a menu click that had to wait for the main thread would hold up the bus, and
/// the shell would redraw the menu as unresponsive.
#[cfg(target_os = "linux")]
fn install(app: &AppHandle, lines: &Lines, icon: &Pixels, label: &str) -> Result<Tray, String> {
    let (action_tx, action_rx) = std::sync::mpsc::channel::<crate::sni::Action>();
    let updates = crate::sni::start(update_for(lines, icon, label), action_tx)?;

    let app = app.clone();
    std::thread::Builder::new()
        .name("nunya-tray-actions".into())
        .spawn(move || {
            // Ends when the item's thread drops the sender, which is when the app is going away.
            while let Ok(action) = action_rx.recv() {
                let app = app.clone();
                // Window and lifecycle calls belong to the main thread; `emit` would be safe
                // anywhere, but routing all three the same way keeps the ordering obvious.
                let _ = app.clone().run_on_main_thread(move || match action {
                    crate::sni::Action::Toggle => {
                        let _ = app.emit("tray-toggle", ());
                    }
                    crate::sni::Action::Show => show_window(&app),
                    // Goes through `ExitRequested`, so the core is stopped and the routes given
                    // back exactly as when the last window closes.
                    crate::sni::Action::Quit => app.exit(0),
                });
            }
        })
        .map_err(|e| e.to_string())?;

    Ok(Tray { updates })
}

#[cfg(not(target_os = "linux"))]
fn base(icon: Image<'static>, template: bool) -> TrayIconBuilder<Wry> {
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(template)
        .tooltip("Nunya")
}

/// A status item with no menu, whose clicks open the popover.
///
/// No menu, rather than one moved to the right click: on macOS 27 a menu attached to the status
/// item takes every click, the left one included, before tray-icon sees it — so the popover never
/// opened and the menu did, whatever `show_menu_on_left_click` said. tray-icon 0.25.1 fixes it by
/// attaching the menu only while showing it, but Tauri 2 is held to 0.24. The popover carries all
/// the menu did (the status, Connect/Disconnect, Open Nunya, Quit), so either button opens it, as
/// NordVPN's does.
#[cfg(target_os = "macos")]
fn install(app: &AppHandle, icon: Image<'static>, template: bool) -> tauri::Result<Tray> {
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};

    crate::popover::create(app)?;
    let icon = base(icon, template)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left | MouseButton::Right,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            {
                crate::popover::toggle(tray.app_handle(), rect);
            }
        })
        .build(app)?;
    Ok(Tray { icon, menu: None })
}

#[cfg(all(not(target_os = "macos"), not(target_os = "linux")))]
fn install(app: &AppHandle, icon: Image<'static>, template: bool) -> tauri::Result<Tray> {
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

    let icon = base(icon, template)
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            TOGGLE => {
                let _ = app.emit("tray-toggle", ());
            }
            SHOW => show_window(app),
            // Goes through `ExitRequested`, so the core is stopped and the routes given back
            // exactly as when the last window closes.
            QUIT => app.exit(0),
            _ => {}
        })
        .build(app)?;

    Ok(Tray {
        icon,
        menu: Some(MenuLines {
            server,
            status,
            toggle,
        }),
    })
}

pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// What the menu says, worked out from what the frontend reported.
#[derive(Debug, PartialEq)]
struct Lines {
    server: String,
    status: String,
    toggle: &'static str,
    toggle_enabled: bool,
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
    mirror(&app, &lines, icon, &label)
}

/// Puts the state into the platform's tray, creating it the first time.
///
/// Split from `set_tray_status` rather than branched inside it, so each platform's path reads as one
/// whole thing — the same reason `install` is two functions.
#[cfg(target_os = "linux")]
fn mirror(app: &AppHandle, lines: &Lines, icon: Pixels, label: &str) -> Result<(), String> {
    match app.try_state::<Tray>() {
        // A closed channel means the item's thread is gone, which only happens on the way out.
        Some(tray) => {
            let _ = tray.updates.send(update_for(lines, &icon, label));
        }
        None => {
            if !supported() {
                // The window is the whole UI; there is nothing to mirror into.
                return Ok(());
            }
            match install(app, lines, &icon, label) {
                Ok(tray) => {
                    app.manage(tray);
                }
                Err(e) => {
                    log::warn!("could not create the tray icon: {e}");
                    give_up();
                    // Not an error to the frontend: a desktop with no tray is a supported shape,
                    // and the window has nothing to do about it.
                }
            }
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn mirror(app: &AppHandle, lines: &Lines, icon: Pixels, label: &str) -> Result<(), String> {
    let image = Image::new_owned(icon.rgba, icon.width, icon.height);
    let tray = match app.try_state::<Tray>() {
        Some(tray) => {
            tray.icon
                .set_icon_with_as_template(Some(image), icon.template)
                .map_err(|e| e.to_string())?;
            tray
        }
        None => {
            if !supported() {
                return Ok(());
            }
            let tray = install(app, image, icon.template).map_err(|e| {
                log::warn!("could not create the tray icon: {e}");
                give_up();
                e.to_string()
            })?;
            app.manage(tray);
            app.state::<Tray>()
        }
    };

    if let Some(menu) = &tray.menu {
        menu.server.set_text(&lines.server).map_err(|e| e.to_string())?;
        menu.status.set_text(&lines.status).map_err(|e| e.to_string())?;
        menu.toggle.set_text(lines.toggle).map_err(|e| e.to_string())?;
        menu.toggle
            .set_enabled(lines.toggle_enabled)
            .map_err(|e| e.to_string())?;
    }
    let _ = tray.icon.set_tooltip(Some(format!("Nunya — {label}")));
    Ok(())
}

/// Packs what the frontend reported into the item thread's message.
#[cfg(target_os = "linux")]
fn update_for(lines: &Lines, icon: &Pixels, label: &str) -> crate::sni::Update {
    crate::sni::Update {
        rgba: icon.rgba.clone(),
        width: icon.width as i32,
        height: icon.height as i32,
        tooltip: format!("Nunya — {label}"),
        server: lines.server.clone(),
        status: lines.status.clone(),
        toggle: lines.toggle.to_string(),
        toggle_enabled: lines.toggle_enabled,
    }
}

#[cfg(test)]
mod tests;
