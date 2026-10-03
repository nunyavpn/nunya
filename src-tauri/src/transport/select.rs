//! Chooses which tunnel transport this build uses.
//!
//! The two macOS transports are not a temporary fork in the road — they are different answers to
//! "who is allowed to create a utun", and the choice is deliberate:
//!
//! - **`subprocess`** runs the core as a child process that creates its own TUN, so *something*
//!   has to be privileged. This is what the client uses today, and what Linux and Windows will use.
//! - **`networkextension`** hands the job to the system, which creates the utun and launches a
//!   bundled `.appex`. Nothing is privileged. This is where macOS is going, and it is blocked only
//!   on signing with the `packet-tunnel-provider` entitlement.
//!
//! The default stays `subprocess` until the extension can actually be signed, so development is
//! never blocked on an Apple Developer membership. Switching is one environment variable, not a
//! rewrite, because everything above `TunnelTransport` is shared.

use std::sync::Arc;

use super::subprocess::SubprocessTransport;
use super::TunnelTransport;
use crate::platform;
use crate::rpc::CoreLink;

/// Overrides the default. Accepts `subprocess` or `networkextension`.
pub const ENV_VAR: &str = "NUNYA_TRANSPORT";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Subprocess,
    /// Only where the platform has one (`platform::NETWORK_EXTENSION`); `parse` refuses it elsewhere.
    NetworkExtension,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Subprocess => "subprocess",
            Kind::NetworkExtension => "networkextension",
        }
    }

    /// What this build uses unless told otherwise.
    ///
    /// Deliberately not NetworkExtension yet: an unsigned extension cannot load, so defaulting to
    /// it would leave every developer without a membership staring at a tunnel that never starts.
    pub fn default_for_platform() -> Self {
        Kind::Subprocess
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "subprocess" | "core" => Some(Kind::Subprocess),
            "networkextension" | "ne" | "appex" if platform::NETWORK_EXTENSION => {
                Some(Kind::NetworkExtension)
            }
            _ => None,
        }
    }

    /// Reads the override, falling back to the platform default.
    pub fn from_env() -> Self {
        match std::env::var(ENV_VAR) {
            Ok(v) if !v.trim().is_empty() => Kind::parse(&v).unwrap_or_else(|| {
                log::warn!("ignoring {ENV_VAR}={v:?}: not a transport this build knows");
                Kind::default_for_platform()
            }),
            _ => Kind::default_for_platform(),
        }
    }
}

/// Builds the selected transport.
///
/// `link` is only used by the subprocess transport; the NetworkExtension one talks to the system
/// instead, and the core it drives lives inside the extension rather than in a child process.
pub fn build(kind: Kind, link: Arc<CoreLink>) -> Arc<dyn TunnelTransport> {
    match kind {
        Kind::Subprocess => Arc::new(SubprocessTransport::new(link)),
        Kind::NetworkExtension => platform::network_extension_transport()
            .expect("parse yields NetworkExtension only where the platform has one"),
    }
}

#[cfg(test)]
mod tests;
