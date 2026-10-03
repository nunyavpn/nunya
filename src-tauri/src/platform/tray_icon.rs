//! The tray on macOS and Windows: tray-icon's status item, re-exported by each of their files.
//!
//! Both put the state into the item the same way — the icon, the tooltip and, where there is one,
//! the menu's lines — and differ only in what `install_tray` builds: a menu on Windows, none on
//! macOS (the popover takes its place). Linux has its own item altogether (`sni.rs`).

use tauri::image::Image;
use tauri::menu::MenuItem;
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Wry};

use super::desktop::{give_up, supported};
use crate::tray::{Lines, Pixels};

/// The icon, and the menu's lines where there is a menu; in managed state once the first status
/// built them.
pub struct Tray {
    pub(super) icon: TrayIcon<Wry>,
    /// `None` on macOS, where the popover takes the menu's place.
    pub(super) menu: Option<MenuLines>,
}

/// The menu items whose text changes. Never built on macOS, where `Tray::menu` is always `None`.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(super) struct MenuLines {
    pub(super) server: MenuItem<Wry>,
    pub(super) status: MenuItem<Wry>,
    pub(super) toggle: MenuItem<Wry>,
}

pub(super) fn base(icon: Image<'static>, template: bool) -> TrayIconBuilder<Wry> {
    TrayIconBuilder::with_id("main")
        .icon(icon)
        .icon_as_template(template)
        .tooltip("Nunya")
}

pub fn tray_mirror(app: &AppHandle, lines: &Lines, icon: Pixels, label: &str) -> Result<(), String> {
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
            let tray = super::imp::install_tray(app, image, icon.template).map_err(|e| {
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
