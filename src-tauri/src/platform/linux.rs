//! Linux: no way yet for the app to obtain privilege for VPN mode.
//!
//! The core needs `CAP_NET_ADMIN` for a TUN. An AppImage runs from a read-only, nosuid mount, so
//! neither a capability nor setuid can be put on the core inside it, and the release core's parent
//! check forbids running a copy of it from anywhere else. Until that is designed, VPN mode is not
//! offered a grant here, and proxy mode is the Linux build's mode.

use std::io;
use std::os::unix::io::RawFd;
use std::path::Path;
use std::process::Command;

pub use super::unix::{
    ipc_bind, ipc_unbind, no_console_window, peer_pid, peer_user_ok, restrict_dir, restrict_file,
    tighten, IpcAcceptor, IpcStream, PendingListener,
};
use tauri::{AppHandle, Emitter, Manager};

use super::sni;
use super::{GrantCopy, Granted};
use crate::sysproxy::{run, Desktop};
use crate::tray::{give_up, show_window, supported, Lines, Pixels};

pub static GRANT: Option<GrantCopy> = None;

pub fn grant(_core: &Path) -> Result<Granted, String> {
    Err("this build cannot obtain privilege for VPN mode on Linux yet; use proxy mode".into())
}

pub fn browser_command(url: &str) -> Command {
    let mut c = Command::new("xdg-open");
    c.arg(url);
    c
}

/// The peer's credentials, as the kernel recorded them when the socket was connected. Linux has
/// no `getpeereid` and answers pid, uid and gid from this one option instead.
fn peer_cred(fd: RawFd) -> io::Result<libc::ucred> {
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: fd is live; `cred` and `len` are valid out-params sized for the option.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(cred)
}

pub(super) fn uid_of_peer(fd: RawFd) -> io::Result<u32> {
    Ok(peer_cred(fd)?.uid)
}

pub(super) fn pid_of_peer(fd: RawFd) -> io::Result<u32> {
    Ok(peer_cred(fd)?.pid as u32)
}

/// The channel into the thread that owns the StatusNotifierItem. The item's state lives there, not
/// here, because libdbus is not thread-safe; see `sni.rs`.
pub struct Tray {
    updates: std::sync::mpsc::Sender<sni::Update>,
}

pub fn tray_mirror(app: &AppHandle, lines: &Lines, icon: Pixels, label: &str) -> Result<(), String> {
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
            match install_tray(app, lines, &icon, label) {
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

/// Starts the StatusNotifierItem and the thread that answers its menu.
///
/// The actions come back on a channel rather than through a callback because the item's thread must
/// not block on Tauri: a menu click that had to wait for the main thread would hold up the bus, and
/// the shell would redraw the menu as unresponsive.
fn install_tray(app: &AppHandle, lines: &Lines, icon: &Pixels, label: &str) -> Result<Tray, String> {
    let (action_tx, action_rx) = std::sync::mpsc::channel::<sni::Action>();
    let updates = sni::start(update_for(lines, icon, label), action_tx)?;

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
                    sni::Action::Toggle => {
                        let _ = app.emit("tray-toggle", ());
                    }
                    sni::Action::Show => show_window(&app),
                    // Goes through `ExitRequested`, so the core is stopped and the routes given
                    // back exactly as when the last window closes.
                    sni::Action::Quit => app.exit(0),
                });
            }
        })
        .map_err(|e| e.to_string())?;

    Ok(Tray { updates })
}

/// Packs what the frontend reported into the item thread's message.
fn update_for(lines: &Lines, icon: &Pixels, label: &str) -> sni::Update {
    sni::Update {
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

pub const TUN_NAME: Option<&str> = Some("nunya-tun");

/// Which desktop's settings hold the system proxy. A runtime question here, not a compile-time
/// one: GNOME, KDE and the rest all run this one binary.
pub fn proxy_desktop() -> Result<Desktop, String> {
    let current = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if current.split(':').any(|d| d.eq_ignore_ascii_case("KDE")) {
        for tool in ["kwriteconfig6", "kwriteconfig5"] {
            if run(tool, &["--help"]).is_ok() {
                return Ok(Desktop::Kde(tool.to_string()));
            }
        }
        return Err("KDE is running, but neither kwriteconfig6 nor kwriteconfig5 was found".into());
    }

    // GNOME, and the desktops that share its settings schema (Budgie, Cinnamon's GTK apps,
    // Pantheon, Unity). Asked of gsettings itself rather than inferred from the desktop name.
    if run("gsettings", &["list-keys", "org.gnome.system.proxy"]).is_ok() {
        return Ok(Desktop::Gnome);
    }

    Err(format!(
        "no supported way to set the system proxy on this desktop ({}); GNOME and KDE are supported",
        if current.is_empty() { "unknown" } else { &current }
    ))
}
