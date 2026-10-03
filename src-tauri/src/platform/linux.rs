//! Linux: no way yet for the app to obtain privilege for VPN mode.
//!
//! The core needs `CAP_NET_ADMIN` for a TUN. An AppImage runs from a read-only, nosuid mount, so
//! neither a capability nor setuid can be put on the core inside it, and the release core's parent
//! check forbids running a copy of it from anywhere else. Until that is designed, VPN mode is not
//! offered a grant here, and proxy mode is the Linux build's mode.

use std::path::Path;
use std::process::Command;

pub use super::unix::{no_console_window, restrict_dir, restrict_file, tighten};
use super::{GrantCopy, Granted};

pub static GRANT: Option<GrantCopy> = None;

pub fn grant(_core: &Path) -> Result<Granted, String> {
    Err("this build cannot obtain privilege for VPN mode on Linux yet; use proxy mode".into())
}

pub fn browser_command(url: &str) -> Command {
    let mut c = Command::new("xdg-open");
    c.arg(url);
    c
}
