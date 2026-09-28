//! macOS: VPN mode without the packet tunnel extension is a setuid-root core.

use std::path::Path;

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
