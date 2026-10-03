//! What differs between macOS, Linux and Windows, behind one interface.
//!
//! Each operating system has its own file here, and only the target's is compiled: shared code
//! calls `platform::…` and never asks which system it is on. The alternative, `#[cfg]` lines inside
//! shared functions, is what the rest of the tree still does, and it means reading every file to
//! learn what one platform does, and stepping around two platforms' branches to change the third.
//! Each file must provide every item `pub use`d below, so a platform left behind is a compile
//! error on that platform's CI job rather than a gap found by a user.
//!
//! Moved here so far: VPN-mode privilege, owner-only files, the system browser and the core's
//! console window. The rest moves in one concern at a time (issue #100).

use std::fs;
use std::io;
use std::path::Path;
use std::process::Command;

use serde::Serialize;

#[cfg_attr(target_os = "macos", path = "macos.rs")]
#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(windows, path = "windows.rs")]
mod imp;

/// What macOS and Linux share; only their files use it.
#[cfg(unix)]
mod unix;

/// What the "Allow VPN mode" sheet says on this platform, or `None` where the app has no way to
/// obtain privilege for VPN mode and must not offer a button that always fails.
pub use imp::GRANT;

/// Obtains privilege for VPN mode (blocking: it waits on the system's own prompt). `core` is the
/// bundled core binary. Called only where `GRANT` is `Some`.
pub use imp::grant;

/// Narrows a directory to its owner. A no-op on Windows, where a profile's AppData is already
/// private to its user and there is no mode to set.
pub use imp::restrict_dir;

/// Narrows a file just created to its owner, before anything is written to it.
pub use imp::restrict_file;

/// Narrows an existing file that anyone else on the machine can read, saying so in the log.
pub use imp::tighten;

/// The desktop's own command for opening `url` in the system browser, not yet started.
pub use imp::browser_command;

/// Keeps a console program started from this GUI app from opening a window of its own.
pub use imp::no_console_window;

/// What the caller has to do once `grant` has succeeded. Each platform builds only the variant
/// its grant produces, hence the allowance.
#[allow(dead_code)]
#[derive(Debug, PartialEq, Eq)]
pub enum Granted {
    /// The core binary now carries the privilege; the running one was started without it.
    RestartCore,
    /// A privileged copy of the whole app has been started; this one should quit.
    Relaunched,
}

/// The sheet's words, as data, so the view is the same on every platform and the platform's
/// facts (who asks, how often, what changes) live beside the code that makes them true.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantCopy {
    pub lede: &'static str,
    pub why: &'static str,
    pub facts: &'static [Fact],
    /// The button's label while the system's prompt is up.
    pub waiting: &'static str,
    /// Why proxy mode is the lighter choice here.
    pub alt: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Fact {
    pub title: &'static str,
    pub text: &'static str,
}

/// Pins every function's signature, so each platform's file has to match it and not merely name it.
#[allow(dead_code)]
fn _signature_check(
    core: &Path,
    file: &fs::File,
    meta: &fs::Metadata,
    child: &mut tokio::process::Command,
) -> Result<Granted, String> {
    let _: io::Result<()> = restrict_dir(core);
    let _: io::Result<()> = restrict_file(file);
    tighten(core, meta);
    let _: Command = browser_command("");
    no_console_window(child);
    grant(core)
}
