//! The menu-bar popover on macOS: a small panel under the tray icon with the connection, the
//! server, Quick Connect, the mode, search and the blockers, in the shape NordVPN's has.
//!
//! It is a second webview (`popover.html`) and it owns no state. The main window sends it a model
//! and acts on what it sends back (`src/popover.ts`), the way the tray menu emits `tray-toggle`
//! rather than connecting itself: a store of its own, in a second JavaScript context, would be a
//! second writer to the data file.
//!
//! **Created with the tray, hidden.** A webview takes a moment to load, and a popover that appears
//! half a second after the click reads as a click that did not register.
//!
//! **It hides when it loses focus**, as a menu does. A click on the icon while it is open takes
//! the focus away first — hiding it — and then arrives as a click, which would show it again; a
//! click within `REOPEN_GUARD` of that hide is the close the user meant.
//!
//! The window is transparent (`macOSPrivateApi` in `tauri.conf.json`), so the page draws the
//! rounded panel and its shadow itself, inside `MARGIN`.
//!
//! Linux keeps the tray menu: a StatusNotifierItem delivers no clicks on the icon, so there is
//! nothing to anchor a popover to.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, Rect, WebviewUrl, WebviewWindowBuilder,
    WindowEvent,
};

pub const LABEL: &str = "popover";

/// The panel as the page draws it, in points: tall enough to show everything without scrolling,
/// and well inside the smallest Mac screen's 800 or so.
const PANEL_WIDTH: f64 = 340.0;
const PANEL_HEIGHT: f64 = 640.0;
/// Room around the panel for its shadow. Must match `--margin` in `popover.css`.
const MARGIN: f64 = 14.0;

/// Between the menu bar and the panel.
const GAP: f64 = 4.0;

const REOPEN_GUARD: Duration = Duration::from_millis(300);

static LAST_HIDDEN: Mutex<Option<Instant>> = Mutex::new(None);

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("popover.html".into()))
        .title("Nunya")
        .inner_size(PANEL_WIDTH + 2.0 * MARGIN, PANEL_HEIGHT + 2.0 * MARGIN)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        // The page draws its own, which a transparent window's system shadow would outline.
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        // Over whatever space the user is in, as a menu is.
        .visible_on_all_workspaces(true)
        // A click on a switch should flip it, not merely focus the panel first.
        .accept_first_mouse(true)
        .visible(false)
        .build()?;

    let handle = app.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            hide(&handle);
        }
    });
    Ok(())
}

/// Shows the popover under the icon at `icon`, or hides it if it is showing.
pub fn toggle(app: &AppHandle, icon: Rect) {
    let Some(window) = app.get_webview_window(LABEL) else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        hide(app);
        return;
    }
    let just_hidden = LAST_HIDDEN
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .is_some_and(|at| at.elapsed() < REOPEN_GUARD);
    if just_hidden {
        return;
    }

    let scale = window.scale_factor().unwrap_or(2.0);
    let at = icon.position.to_physical::<f64>(scale);
    let size = icon.size.to_physical::<f64>(scale);
    // The icon's screen, in pixels. The lookup is in points (macOS's `CGDisplayBounds`): asked in
    // pixels, a Retina screen's right half lands on whatever display sits beside it.
    let screen = app
        .monitor_from_point(at.x / scale, at.y / scale)
        .ok()
        .flatten()
        .map(|m| {
            let left = m.position().x as f64;
            (left, left + m.size().width as f64)
        });
    let (x, y) = origin(at.x, size.width, at.y + size.height, scale, screen);

    let _ = window.set_position(PhysicalPosition::new(x, y));
    let _ = window.show();
    let _ = window.set_focus();
    // So the page asks for a fresh model.
    let _ = app.emit_to(LABEL, "popover-shown", ());
}

/// Where the window goes, in pixels, for an icon spanning `icon_x` to `icon_x + icon_width` whose
/// bottom edge is at `icon_bottom`: centred under it, its panel just below the menu bar, and kept
/// within `screen` (left and right edges) so an icon near a corner does not push it off.
///
/// The window is the panel plus a transparent `MARGIN` all round, so the margin is taken back off
/// to put the panel itself where it belongs.
fn origin(
    icon_x: f64,
    icon_width: f64,
    icon_bottom: f64,
    scale: f64,
    screen: Option<(f64, f64)>,
) -> (f64, f64) {
    let width = (PANEL_WIDTH + 2.0 * MARGIN) * scale;
    let mut x = icon_x + icon_width / 2.0 - width / 2.0;
    if let Some((left, right)) = screen {
        x = x.min(right - width).max(left);
    }
    let y = icon_bottom + GAP * scale - MARGIN * scale;
    (x, y)
}

pub fn hide(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        if window.is_visible().unwrap_or(false) {
            let _ = window.hide();
            *LAST_HIDDEN.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
            // The main window builds the popover's model only while it is showing.
            let _ = app.emit_to("main", "popover-hidden", ());
        }
    }
}

/// Escape, and anything in the popover that hands off to somewhere else.
#[tauri::command]
pub fn hide_popover(app: AppHandle) {
    hide(&app);
}

/// The popover's "Open Nunya".
#[tauri::command]
pub fn show_main_window(app: AppHandle) {
    hide(&app);
    crate::tray::show_window(&app);
}

/// The popover's "Quit". Through `ExitRequested`, like the tray menu's, so the core is stopped and
/// the system proxy put back.
#[tauri::command]
pub fn quit(app: AppHandle) {
    app.exit(0);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Retina screen 1512pt wide: 3024 pixels.
    const SCREEN: Option<(f64, f64)> = Some((0.0, 3024.0));

    #[test]
    fn the_panel_is_centred_under_its_icon_just_below_the_menu_bar() {
        let (x, y) = origin(2400.0, 68.0, 66.0, 2.0, SCREEN);
        let width = (PANEL_WIDTH + 2.0 * MARGIN) * 2.0;
        assert_eq!(x + width / 2.0, 2434.0, "centred on the icon's middle");
        // The panel itself, inside its transparent margin, starts a small gap below the bar.
        assert_eq!(y + MARGIN * 2.0, 66.0 + GAP * 2.0);
    }

    #[test]
    fn an_icon_near_the_screens_edge_does_not_push_the_panel_off_it() {
        let width = (PANEL_WIDTH + 2.0 * MARGIN) * 2.0;
        let (right, _) = origin(2980.0, 40.0, 66.0, 2.0, SCREEN);
        assert_eq!(right + width, 3024.0);
        let (left, _) = origin(10.0, 40.0, 66.0, 2.0, SCREEN);
        assert_eq!(left, 0.0);
    }

    #[test]
    fn a_screen_beside_the_first_is_measured_from_its_own_left_edge() {
        let (x, _) = origin(3100.0, 40.0, 66.0, 2.0, Some((3024.0, 6864.0)));
        assert!(x >= 3024.0, "the panel stayed on the icon's screen, at {x}");
    }
}
