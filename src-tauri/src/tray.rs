//! The tray icon, in the macOS menu bar and the Linux top bar: what the tunnel is doing, and a way
//! to change it without the window.
//!
//! A tunnel is a background thing, and a client whose only control is a 1040px window forces a
//! choice between leaving that window open all day and closing it — which, before this, quit the
//! app and tore the tunnel down with it. The icon lets the window go away while the tunnel stays.
//!
//! **The icon is the state**, as OpenVPN's is: the rail's shield, green with a check when
//! connected, amber while connecting, red with an exclamation mark when not working, an outline
//! with a slash when off. The frontend paints it (`trayicon.ts`) from the rail's own glyphs and
//! colour tokens and sends the pixels with every status, so the bar and the window cannot
//! disagree. Off comes marked as a template, which macOS tints to the menu bar like the icons
//! beside it.
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
//! Everything is a menu item because a Linux tray (StatusNotifierItem via libayatana-appindicator)
//! delivers no click events on the icon itself: a left click opens the menu, and that is all. On
//! macOS a left click opens the popover instead (`popover.rs`), and the menu moves to a right
//! click, where it stays as the quick way to Show or Quit.

use std::sync::OnceLock;

use serde::Deserialize;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Emitter, Manager, Wry};

/// The icon and the items whose text changes, in managed state once the first status built them.
pub struct Tray {
    icon: TrayIcon<Wry>,
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
    /// Draw it in the menu bar's own colour. macOS only; the frontend sets it for off.
    template: bool,
}

const TOGGLE: &str = "toggle";
const SHOW: &str = "show";
const QUIT: &str = "quit";

/// Whether the tray is up, and closing the window can therefore hide it rather than quit.
pub fn is_up(app: &AppHandle) -> bool {
    app.try_state::<Tray>().is_some()
}

/// Whether this desktop can show a tray at all; asked once.
fn supported() -> bool {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    *SUPPORTED.get_or_init(desktop_has_tray)
}

/// The tray library is loaded at runtime and panics when it is missing, so its presence is
/// checked first. Without it the window stays the whole UI, and closing it quits.
#[cfg(target_os = "linux")]
fn desktop_has_tray() -> bool {
    let found = ["libayatana-appindicator3.so.1", "libappindicator3.so.1"]
        .iter()
        .any(|name| {
            let name = std::ffi::CString::new(*name).expect("no interior nul");
            // SAFETY: a nul-terminated name; the handle is released again immediately.
            let handle = unsafe { libc::dlopen(name.as_ptr(), libc::RTLD_LAZY) };
            if handle.is_null() {
                return false;
            }
            unsafe { libc::dlclose(handle) };
            true
        });
    if !found {
        log::warn!("no appindicator library; running without a tray icon");
    }
    found
}

#[cfg(not(target_os = "linux"))]
fn desktop_has_tray() -> bool {
    true
}

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

    let builder = TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(template)
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

    #[cfg(target_os = "macos")]
    let builder = {
        crate::popover::create(app)?;
        builder
            .show_menu_on_left_click(false)
            .on_tray_icon_event(|tray, event| {
                use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    rect,
                    ..
                } = event
                {
                    crate::popover::toggle(tray.app_handle(), rect);
                }
            })
    };

    let icon = builder.build(app)?;

    Ok(Tray {
        icon,
        server,
        status,
        toggle,
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
/// `state` is the frontend's `ConnectionState` (`off`, `connecting`, `on`) and `tone` the rail
/// shield's (`off`, `connecting`, `on`, `failed`); `server` is the selected server's name as the
/// list shows it, `detail` is anything the status line must add (proxy mode's listen address),
/// `label` is the shield's own description, used as the tooltip, and `can_connect` is false
/// whenever the status card would refuse Connect — no server, no core — so the menu never offers
/// a button the window would turn down.
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
                // The window is the whole UI; there is nothing to mirror into.
                return Ok(());
            }
            let tray = install(&app, image, icon.template).map_err(|e| {
                log::warn!("could not create the tray icon: {e}");
                e.to_string()
            })?;
            app.manage(tray);
            app.state::<Tray>()
        }
    };

    let lines = Lines::new(
        &state,
        &tone,
        server.as_deref(),
        detail.as_deref(),
        can_connect,
    );
    tray.server.set_text(&lines.server).map_err(|e| e.to_string())?;
    tray.status.set_text(&lines.status).map_err(|e| e.to_string())?;
    tray.toggle.set_text(lines.toggle).map_err(|e| e.to_string())?;
    tray.toggle
        .set_enabled(lines.toggle_enabled)
        .map_err(|e| e.to_string())?;
    // Unsupported by the Linux tray, which shows the status line instead.
    let _ = tray.icon.set_tooltip(Some(format!("Nunya — {label}")));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::Lines;

    #[test]
    fn a_tunnel_that_carries_nothing_says_not_working_and_still_offers_disconnect() {
        let lines = Lines::new("on", "failed", Some("DE-1 Frankfurt"), None, true);
        assert_eq!(lines.status, "Status: Not working");
        assert_eq!(lines.toggle, "Disconnect");
        assert!(lines.toggle_enabled);
    }

    #[test]
    fn an_attempt_that_failed_says_not_working_and_offers_connect_again() {
        let lines = Lines::new("off", "failed", Some("DE-1 Frankfurt"), None, true);
        assert_eq!(lines.status, "Status: Not working");
        assert_eq!(lines.toggle, "Connect");
        assert!(lines.toggle_enabled);
    }

    #[test]
    fn connect_is_offered_only_when_the_window_would_accept_it() {
        assert!(Lines::new("off", "off", Some("a"), None, true).toggle_enabled);
        assert!(!Lines::new("off", "off", Some("a"), None, false).toggle_enabled);
        // Not a second attempt on top of the first.
        assert!(!Lines::new("connecting", "connecting", Some("a"), None, true).toggle_enabled);
    }

    #[test]
    fn proxy_mode_names_the_listener_rather_than_the_machine() {
        let lines = Lines::new("on", "on", Some("a"), Some("proxy on 127.0.0.1:2080"), true);
        assert_eq!(lines.status, "Status: Connected · proxy on 127.0.0.1:2080");
    }

    #[test]
    fn no_selection_is_said_rather_than_left_blank() {
        let lines = Lines::new("off", "off", None, None, false);
        assert_eq!(lines.server, "No server selected");
        assert_eq!(lines.status, "Status: Disconnected");
    }
}
