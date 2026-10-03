//! Windows: VPN mode runs with Nunya started as administrator.
//!
//! The core creates its TUN (wintun) with administrator rights, and it has them exactly when Nunya
//! does, since it is Nunya's child. So the grant is not a change to the core, as on macOS, but a
//! relaunch: Nunya starts an elevated copy of itself through UAC and quits. Elevating the core
//! alone would not do: it must stay a child of `Nunya.exe` for its parent check, and ours on the
//! pipe, to pass.
//!
//! UAC's consent prompt keeps the privilege with the same user, so the elevated copy reads the same
//! data file. A standard user who types an administrator's credentials instead gets a copy running
//! as that administrator, with that account's (empty) data; the sheet says to use an account that
//! is an administrator.

use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::{AppHandle, Emitter};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;

use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub use super::tray_icon::{tray_mirror, Tray};
use super::tray_icon::MenuLines;
use super::{Fact, GrantCopy, Granted};

pub static GRANT: Option<GrantCopy> = Some(GrantCopy {
    lede: "VPN mode needs Nunya to run as administrator",
    why: "To send all of this PC's traffic through the tunnel, Nunya creates a network adapter, \
          and Windows allows that only to an app running as administrator.",
    facts: &[
        Fact {
            title: "Windows asks, not Nunya.",
            text: "Nunya restarts through Windows' own prompt; it never sees a password.",
        },
        Fact {
            title: "Each time you start it.",
            text: "Windows gives the rights to this run of Nunya only. Start it as administrator to skip this.",
        },
        Fact {
            title: "Use your own account.",
            text: "Nunya's servers and settings belong to the Windows account it runs as.",
        },
    ],
    waiting: "Waiting for Windows…",
    alt: "no administrator rights, but it covers only apps set to use it.",
});

/// ShellExecute's answer when the user declines the UAC prompt (`SE_ERR_ACCESSDENIED`).
const DECLINED: isize = 5;

/// Starts an elevated copy of Nunya. The caller quits this one once it returns.
pub fn grant(_core: &Path) -> Result<Granted, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let verb = wide(OsStr::new("runas"));
    let file = wide(exe.as_os_str());
    // SAFETY: both strings are NUL-terminated and outlive the call; the rest are optional nulls.
    let code = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    // Anything above 32 is success, by ShellExecute's own convention.
    match code {
        c if c > 32 => Ok(Granted::Relaunched),
        DECLINED => Err("the Windows prompt was declined, so VPN mode is still off".into()),
        c => Err(format!("could not restart Nunya as administrator (ShellExecute error {c})")),
    }
}

/// A profile's AppData is already private to its user, and Windows has no mode bits to narrow.
pub fn restrict_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

pub fn restrict_file(_file: &fs::File) -> io::Result<()> {
    Ok(())
}

pub fn tighten(_path: &Path, _meta: &fs::Metadata) {}

pub fn browser_command(url: &str) -> Command {
    // Not `cmd /C start`, which would read the URL's `&` as a command separator.
    let mut c = Command::new("rundll32");
    c.args(["url.dll,FileProtocolHandler", url]);
    c
}

/// The core is a console program, and Windows gives a console program started from a GUI app a
/// window of its own. CREATE_NO_WINDOW runs it without one; its output still comes through the
/// pipes the caller set up.
pub fn no_console_window(cmd: &mut tokio::process::Command) {
    cmd.creation_flags(0x0800_0000);
}

// The core's link: a named pipe, which is what the core dials here (`ConnectIPC` in its
// `internal/ipc/ipc_windows.go`). The core checks it from its side with
// `GetNamedPipeServerProcessId`; `peer_pid` is our half.

/// The pipe's name. tokio creates a pipe only on a runtime, and `ipc_bind` runs outside one, so the
/// first instance is made by `IpcAcceptor::adopt`. The core does not notice the gap: it retries its
/// dial for five seconds (`RunCore` in the core's `main.go`).
pub type PendingListener = OsString;

pub type IpcStream = NamedPipeServer;

/// `socket_path` only names the directory the core works in; the address handed back is the
/// pipe's name, which is what the core must be given.
pub fn ipc_bind(_socket_path: PathBuf) -> io::Result<(PathBuf, PendingListener)> {
    let name = pipe_name();
    Ok((PathBuf::from(&name), name))
}

/// A pipe has no file; it goes when its last handle closes.
pub fn ipc_unbind(_socket_path: &Path) {}

/// A pipe instance carries one connection, so the next one is created before the connected one is
/// handed over, and a core restarted meanwhile finds it.
pub struct IpcAcceptor {
    name: OsString,
    server: NamedPipeServer,
}

impl IpcAcceptor {
    /// `first_pipe_instance` makes creating the pipe fail if something else already made one of
    /// this name, rather than joining it, so the pipe that is served is always ours.
    pub fn adopt(name: PendingListener) -> io::Result<Self> {
        let server = ServerOptions::new().first_pipe_instance(true).create(&name)?;
        Ok(Self { name, server })
    }

    pub async fn accept(&mut self) -> io::Result<IpcStream> {
        self.server.connect().await?;
        let next = ServerOptions::new().create(&self.name).map_err(|e| {
            io::Error::new(e.kind(), format!("could not create the next core pipe: {e}"))
        })?;
        Ok(std::mem::replace(&mut self.server, next))
    }
}

/// There is no uid to compare: the pid is the whole identity, and it is the child we started.
pub fn peer_user_ok(_stream: &IpcStream) -> Result<(), String> {
    Ok(())
}

pub fn peer_pid(stream: &IpcStream) -> io::Result<u32> {
    let mut pid = 0u32;
    // SAFETY: `stream` is borrowed, so its handle is live for this call; `pid` is a valid out-param.
    if unsafe { GetNamedPipeClientProcessId(stream.as_raw_handle(), &mut pid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(pid)
}

/// A pipe name no other link will use: the main core's and every probe core's are bound from one
/// process, and a name reused from a crashed run would fail `first_pipe_instance`.
fn pipe_name() -> OsString {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!(
        r"\\.\pipe\nunya-{}-{nanos}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
    .into()
}

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}

const TOGGLE: &str = "toggle";
const SHOW: &str = "show";
const QUIT: &str = "quit";

/// tray-icon's item with its menu: Windows has neither of the problems that keep the menu off
/// macOS and replace the item on Linux.
pub(super) fn install_tray(app: &AppHandle, icon: Image<'static>, template: bool) -> tauri::Result<Tray> {
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

    let icon = super::tray_icon::base(icon, template)
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            TOGGLE => {
                let _ = app.emit("tray-toggle", ());
            }
            SHOW => crate::tray::show_window(app),
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
