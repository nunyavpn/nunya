//! Linux: VPN mode by file capabilities on the installed core, granted through polkit.
//!
//! A TUN needs network-administration rights, and the core counts itself privileged only with the
//! whole set sing-box's own service unit carries (`CAPS`; `hasTunCapabilities` in the core's
//! `internal/rpc/privilege_linux.go`) — a subset opens the TUN and then fails part-way. So the grant
//! is `pkexec setcap` on the bundled core: the system asks for the password, and `setcap`, not the
//! core, runs as root. The core stays a plain child of `Nunya`, which both its parent check and
//! ours (`rpc/peer.rs`) require — the reason `sudo` or `pkexec` *on the core* would not do, as on
//! macOS. Capabilities rather than setuid root (the macOS shape) because they grant only what a
//! TUN needs, not every power root has.
//!
//! **Only a root-owned core beside a root-owned `Nunya` is granted** (the .deb's `/usr/bin`). The
//! release core's parent check is what keeps other programs from driving it, and that check is only
//! a lock if nobody but root can put another `Nunya` beside it; in a user-owned directory the
//! user's own code could, and would then drive a core with network-administration rights. A
//! development core has the check compiled out altogether. Where the grant cannot be made safely
//! or at all, `grant_blocked` says why and no grant is offered.
//!
//! A package upgrade replaces the core and with it the capabilities, so the prompt comes back after
//! an update, as on macOS.

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
use super::{Fact, GrantCopy, Granted};
use crate::sysproxy::{run, Desktop};
use crate::tray::{give_up, show_window, supported, Lines, Pixels};

pub static GRANT: Option<GrantCopy> = Some(GrantCopy {
    lede: "VPN mode needs your password once",
    why: "To send all of this computer's traffic through the tunnel, Nunya creates a network \
          interface, and Linux allows that only with an administrator's approval.",
    facts: &[
        Fact {
            title: "Your system asks, not Nunya.",
            text: "The password goes to your system's own prompt; Nunya never sees it.",
        },
        Fact { title: "Once.", text: "You're asked again only after Nunya updates." },
        Fact {
            title: "What changes.",
            text: "Nunya's tunnel engine gets network-administration rights, not full root, and runs \
                   only when Nunya starts it.",
        },
    ],
    waiting: "Waiting for authentication…",
    alt: "no password, but it covers only apps set to use it.",
});

/// sing-box's service unit set, in `setcap`'s spelling. Keep in step with the core's
/// `tunCapabilities`: it counts itself privileged only with all of them.
const CAPS: &str = "cap_net_admin,cap_net_raw,cap_net_bind_service,cap_sys_ptrace,cap_dac_read_search+ep";

/// Called by full path: an app started from a desktop launcher can have a minimal `PATH`, and a
/// `pkexec` or `setcap` found elsewhere on it is not one to hand a password to.
const PKEXEC: &str = "/usr/bin/pkexec";
const SETCAP: &str = "/usr/sbin/setcap";
const SETCAP_ALT: &str = "/usr/bin/setcap";

pub fn grant_blocked(core: &Path) -> Option<String> {
    let exe = match std::env::current_exe() {
        Ok(e) => e,
        Err(e) => return Some(e.to_string()),
    };
    if exe.file_name() != Some(std::ffi::OsStr::new("Nunya")) || core.parent() != exe.parent() {
        return Some(
            "only the tunnel engine installed beside Nunya can be given network rights; \
             for development, use scripts/dev-linux.sh"
                .into(),
        );
    }
    for path in [exe.as_path(), core, core.parent().unwrap_or(core)] {
        match std::fs::symlink_metadata(path) {
            Ok(m) if m.file_type().is_symlink() => {
                return Some(format!("{} is a symbolic link, which is not granted", path.display()))
            }
            Ok(m) if std::os::unix::fs::MetadataExt::uid(&m) != 0 => {
                return Some(format!(
                    "{} is not owned by root, so another program could stand in for Nunya; \
                     VPN mode needs Nunya installed from its package",
                    path.display()
                ))
            }
            Ok(_) => {}
            Err(e) => return Some(format!("{}: {e}", path.display())),
        }
    }
    if !Path::new(PKEXEC).exists() {
        return Some("VPN mode needs pkexec (polkit) to ask for the password, and it is not installed".into());
    }
    if setcap().is_none() {
        return Some("VPN mode needs setcap (libcap) to grant network rights, and it is not installed".into());
    }
    None
}

fn setcap() -> Option<&'static str> {
    [SETCAP, SETCAP_ALT].into_iter().find(|p| Path::new(p).exists())
}

/// Gives the bundled core its capabilities, behind the system's password prompt.
pub fn grant(core: &Path) -> Result<Granted, String> {
    if let Some(why) = grant_blocked(core) {
        return Err(why);
    }
    let setcap = setcap().ok_or("setcap is not installed")?;
    // Arguments, never a shell line, so no path can be read as anything but a path.
    let out = Command::new(PKEXEC)
        .arg(setcap)
        .arg(CAPS)
        .arg(core)
        .output()
        .map_err(|e| e.to_string())?;
    match out.status.code() {
        Some(0) => Ok(Granted::RestartCore),
        // pkexec's own codes: the prompt dismissed, or no authorization.
        Some(126) | Some(127) => Err("the password prompt was cancelled, so VPN mode is still off".into()),
        _ => Err(format!(
            "could not give the tunnel engine network rights: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )),
    }
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

/// There is no popover here; the tray has a menu.
pub fn hide_popover(_app: &tauri::AppHandle) {}

/// Nothing here needs an application event the shared handler does not already take.
pub fn on_run_event(_app: &tauri::AppHandle, _event: &tauri::RunEvent) {}

/// No NetworkExtension outside macOS: the subprocess transport is the only one.
pub const NETWORK_EXTENSION: bool = false;

pub fn network_extension_transport() -> Option<std::sync::Arc<dyn crate::transport::TunnelTransport>> {
    None
}

pub const OS: &str = "linux";

/// WebKitGTK's DMA-BUF renderer on Wayland with NVIDIA's driver: the window dies at startup with
/// "Error 71 (Protocol error) dispatching to Wayland display" (tauri-apps/tauri#8541, #9394). It is
/// switched off here, and only where it breaks — Wayland and the NVIDIA driver loaded — since it
/// costs other machines GPU buffer sharing for nothing. A value the user set is left alone.
pub fn before_webview() {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some()
        && std::env::var("GDK_BACKEND").map_or(true, |b| b.contains("wayland"));
    let nvidia = Path::new("/proc/driver/nvidia/version").exists();
    if wayland && nvidia && std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        // Before any thread exists: `run` calls this first, and the webview reads it later.
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    }
}

/// Nunya on Linux is a package (.deb, .rpm, Arch), and a package belongs to its package
/// manager: replacing `/usr/bin/Nunya` from inside the app would need root, and would leave the
/// package manager's records describing files that are no longer there.
pub const IN_APP_UPDATES: bool = false;
