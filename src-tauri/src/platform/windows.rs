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

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::process::Command;

use windows_sys::Win32::UI::Shell::ShellExecuteW;
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

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

fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}
