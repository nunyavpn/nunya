//! What differs between macOS, Linux and Windows, behind one interface.
//!
//! Each operating system has its own file here, and only the target's is compiled: shared code
//! calls `platform::…` and never asks which system it is on. The alternative, `#[cfg]` lines inside
//! shared functions, is what the rest of the tree still does, and it means reading every file to
//! learn what one platform does, and stepping around two platforms' branches to change the third.
//! Each file must provide every item `pub use`d below, so a platform left behind is a compile
//! error on that platform's CI job rather than a gap found by a user.
//!
//! Only VPN-mode privilege lives here so far; the rest moves in one concern at a time (issue #100).

use std::path::Path;

use serde::Serialize;

#[cfg_attr(target_os = "macos", path = "macos.rs")]
#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(windows, path = "windows.rs")]
mod imp;

/// What the "Allow VPN mode" sheet says on this platform, or `None` where the app has no way to
/// obtain privilege for VPN mode and must not offer a button that always fails.
pub use imp::GRANT;

/// Obtains privilege for VPN mode (blocking: it waits on the system's own prompt). `core` is the
/// bundled core binary. Called only where `GRANT` is `Some`.
pub use imp::grant;

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

/// Pins `grant`'s signature, so each platform's file has to match it and not merely name it.
#[allow(dead_code)]
fn _signature_check(core: &Path) -> Result<Granted, String> {
    grant(core)
}
