//! The seam between the UI and whatever actually carries the tunnel.
//!
//! Each platform has its own sanctioned way to own a TUN device, and they are not variations on a
//! theme — they differ in who runs the core, who holds the privilege, and how the two talk:
//!
//! | Platform | Mechanism | Core runs | Privilege |
//! | --- | --- | --- | --- |
//! | macOS | `NEPacketTunnelProvider` in a bundled `.appex` | linked into the extension | none: the system owns the utun |
//! | Windows | a Windows service with WinTun | subprocess | the service |
//! | Linux | systemd unit or `CAP_NET_ADMIN` | subprocess | capability on the core |
//!
//! Everything above this trait — config generation, share-link parsing, the whole UI — is shared.
//! Everything below it is per-platform. Keeping the boundary explicit is what stops the macOS
//! design from quietly becoming an assumption the other two have to work around.

use async_trait::async_trait;

use crate::config::{BuildRequest, Mode};

pub mod select;
pub mod subprocess;

#[cfg(target_os = "macos")]
pub mod network_extension;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TunnelState {
    Disconnected,
    Connecting,
    Connected,
    /// The transport is installed but the user has not yet granted permission. On macOS this is
    /// the state before the VPN configuration is approved in System Settings.
    NeedsPermission,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Throughput {
    /// Cumulative bytes since the tunnel came up, not a rate. The UI differentiates.
    pub uplink: i64,
    pub downlink: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("{0}")]
    Core(String),
    #[error("the tunnel transport is not available: {0}")]
    Unavailable(String),
    #[error("permission has not been granted for the VPN configuration")]
    PermissionDenied,
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// What the UI needs from a tunnel, and nothing more.
///
/// Deliberately free of anything platform-shaped: no process handles, no file descriptors, no
/// socket paths. A transport that cannot be described through this is a transport that will leak
/// its assumptions into the UI.
#[async_trait]
pub trait TunnelTransport: Send + Sync {
    /// Whether this transport can run at all — the extension is installed, the service exists, the
    /// capability is set. Checked before the user is offered a connect button.
    ///
    /// Takes the mode because the answer depends on it: a TUN needs privilege or a system
    /// approval, and a local listener needs neither. Asking without it would refuse proxy mode
    /// for want of a permission it never uses.
    async fn availability(&self, mode: Mode) -> Result<TunnelState, TransportError>;

    /// Asks the user for whatever consent the platform requires, once.
    ///
    /// On macOS this saves a `NETunnelProviderManager` configuration, which raises the standard
    /// "would like to add VPN configurations" prompt and puts the entry in System Settings.
    /// Transports that need no consent return immediately.
    async fn request_permission(&self) -> Result<(), TransportError>;

    async fn start(&self, request: &BuildRequest) -> Result<(), TransportError>;

    async fn stop(&self) -> Result<(), TransportError>;

    async fn state(&self) -> Result<TunnelState, TransportError>;

    async fn throughput(&self) -> Result<Throughput, TransportError>;
}
