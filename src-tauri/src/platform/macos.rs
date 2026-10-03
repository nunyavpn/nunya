//! macOS: VPN mode without the packet tunnel extension is a setuid-root core.

use std::io;
use std::os::unix::io::RawFd;
use std::path::Path;
use std::process::Command;

pub use super::unix::{
    ipc_bind, ipc_unbind, no_console_window, peer_pid, peer_user_ok, restrict_dir, restrict_file,
    tighten, IpcAcceptor, IpcStream, PendingListener,
};
pub use super::tray_icon::{tray_mirror, Tray};
use super::{Fact, GrantCopy, Granted};

pub static GRANT: Option<GrantCopy> = Some(GrantCopy {
    lede: "VPN mode needs your administrator password",
    why: "To send all of this Mac's traffic through the tunnel, Nunya creates a network \
          interface, and macOS allows that only with an administrator's approval.",
    facts: &[
        Fact { title: "macOS asks, not Nunya.", text: "The password goes to macOS; Nunya never sees it." },
        Fact { title: "Once.", text: "You're asked again only after Nunya updates." },
        Fact {
            title: "What changes.",
            text: "Nunya's tunnel engine keeps administrator rights, and runs only when Nunya starts it.",
        },
    ],
    waiting: "Waiting for macOS…",
    alt: "no password, but it covers only apps set to use it.",
});

/// Makes the bundled core setuid root, behind the system's administrator password prompt.
///
/// This is how VPN mode works on macOS without a paid Apple Developer membership, which the packet
/// tunnel extension needs to be signed. A utun needs root, and of the ways to be root this is the
/// only one both ends' checks survive: a core started through `osascript` or `sudo` has *them* as
/// its parent, which fails the core's parent check and our pid check alike. Setuid keeps the core a
/// plain child of `Nunya`. The core expects it — it drops back to the real uid for any child it
/// starts (`applyPrivilegeDrop` in its `internal/process`).
///
/// Only a core beside a binary named `Nunya` is granted, because the release core's parent check is
/// then the lock on it: nothing but `Nunya` in that directory can drive it. A development core
/// (`fetch-core.sh --source`) has that check compiled out, and setuid on it would leave a root
/// binary any local process could drive; `scripts/dev-tunnel.sh` is the way to test VPN mode there.
///
/// ponytail: the app's directory is the user's, so code already running as the user can swap
/// `Nunya` and drive the root core. That is the price of not having an extension; the extension
/// (`transport::network_extension`) is the way out, and nothing here survives it.
///
/// An app update replaces the core and with it the bit, so the prompt comes back after an update.
pub fn grant(core: &Path) -> Result<Granted, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    if exe.file_name() != Some(std::ffi::OsStr::new("Nunya")) || core.parent() != exe.parent() {
        return Err(
            "only the core inside Nunya.app can be given administrator access; \
             for development, use scripts/dev-tunnel.sh"
                .into(),
        );
    }
    if exe.to_string_lossy().contains("/AppTranslocation/") {
        return Err("move Nunya to Applications first: macOS is running it from a read-only copy".into());
    }

    // The path travels as an argument, never spliced into the script, so no quoting can break it.
    // `test ! -L` refuses a symlink, which chown and chmod would otherwise follow to whatever it
    // names — making *that* setuid root.
    let out = std::process::Command::new("/usr/bin/osascript")
        .args([
            "-e", "on run argv",
            "-e", "set p to quoted form of item 1 of argv",
            "-e", "do shell script \"test ! -L \" & p & \" && /usr/sbin/chown root:wheel \" & p & \" && /bin/chmod 4755 \" & p with prompt \"Nunya needs administrator access to create a VPN interface.\" with administrator privileges",
            "-e", "end run",
        ])
        .arg(core)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        return Ok(Granted::RestartCore);
    }
    let err = String::from_utf8_lossy(&out.stderr);
    // -128 is AppleScript's "User canceled".
    if err.contains("(-128)") {
        Err("the password prompt was cancelled, so VPN mode is still off".into())
    } else {
        Err(format!("could not give the core administrator access: {}", err.trim()))
    }
}

pub fn browser_command(url: &str) -> Command {
    let mut c = Command::new("open");
    c.arg(url);
    c
}

/// `getsockopt` level for `AF_UNIX` socket options on Darwin.
const SOL_LOCAL: libc::c_int = 0;

/// Returns the pid of the process on the other end of a connected `AF_UNIX` socket.
const LOCAL_PEERPID: libc::c_int = 0x002;

/// Effective uid of the peer process. `getpeereid` is a BSD interface; Linux answers the same
/// question from `SO_PEERCRED` instead.
pub(super) fn uid_of_peer(fd: RawFd) -> io::Result<u32> {
    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;
    // SAFETY: fd is a live connected AF_UNIX socket; both out-params are valid stack slots.
    let rc = unsafe { libc::getpeereid(fd, &mut uid, &mut gid) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(uid)
}

pub(super) fn pid_of_peer(fd: RawFd) -> io::Result<u32> {
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: fd is live; `pid` and `len` are valid out-params sized for the option.
    let rc = unsafe {
        libc::getsockopt(
            fd,
            SOL_LOCAL,
            LOCAL_PEERPID,
            &mut pid as *mut _ as *mut libc::c_void,
            &mut len,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(pid as u32)
}

/// A status item with no menu, whose clicks open the popover.
///
/// No menu, rather than one moved to the right click: on macOS 27 a menu attached to the status
/// item takes every click, the left one included, before tray-icon sees it — so the popover never
/// opened and the menu did, whatever `show_menu_on_left_click` said. tray-icon 0.25.1 fixes it by
/// attaching the menu only while showing it, but Tauri 2 is held to 0.24. The popover carries all
/// the menu did (the status, Connect/Disconnect, Open Nunya, Quit), so either button opens it, as
/// NordVPN's does.
pub(super) fn install_tray(
    app: &tauri::AppHandle,
    icon: tauri::image::Image<'static>,
    template: bool,
) -> tauri::Result<Tray> {
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconEvent};

    crate::popover::create(app)?;
    let icon = super::tray_icon::base(icon, template)
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
