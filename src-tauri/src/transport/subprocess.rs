//! The subprocess transport: the app drives the core it hosts, with the core's own calls.
//!
//! This is the Linux and Windows path, and what macOS uses until its packet tunnel can be signed:
//! the core is a child process on a socket, and needs the privilege required to create a TUN
//! itself, which is exactly the property NetworkExtension gets to avoid. On a phone the same calls
//! reach a core hosted in the app (`platform::Engine`), so the transport is unchanged there.

use async_trait::async_trait;

use super::{Throughput, TransportError, TunnelState, TunnelTransport};
use crate::config::{self, BuildRequest, Mode};
use crate::platform::Engine;
use crate::rpc::{gen, method, LinkError};

impl From<LinkError> for TransportError {
    fn from(e: LinkError) -> Self {
        match e {
            LinkError::NotConnected => {
                TransportError::Unavailable("the core is not connected".into())
            }
            other => TransportError::Core(other.to_string()),
        }
    }
}

pub struct SubprocessTransport {
    core: std::sync::Arc<Engine>,
}

impl SubprocessTransport {
    pub fn new(core: std::sync::Arc<Engine>) -> Self {
        Self { core }
    }

    /// Collapses the core's two failure channels. A transport error means the call never landed;
    /// a populated `ErrorResp.error` means it landed and the core refused.
    fn or_err(resp: gen::ErrorResp) -> Result<(), TransportError> {
        match resp.error {
            Some(e) if !e.is_empty() => Err(TransportError::Core(e)),
            _ => Ok(()),
        }
    }
}

#[async_trait]
impl TunnelTransport for SubprocessTransport {
    async fn availability(&self, mode: Mode) -> Result<TunnelState, TransportError> {
        if !self.core.is_connected().await {
            return Err(TransportError::Unavailable("the core is not running".into()));
        }

        // A local listener binds an unprivileged port and creates no interface, so the privilege
        // question below simply does not arise. This is the whole reason proxy mode works today
        // on a machine where the TUN path does not.
        if mode == Mode::Proxy {
            return Ok(TunnelState::Disconnected);
        }

        let resp: gen::IsPrivilegedResponse = self
            .core
            .call(method::IS_PRIVILEGED, &gen::EmptyReq {})
            .await?;

        // Without privilege the core cannot create a TUN, so this transport is present but unusable
        // — the same shape of problem as an unapproved VPN configuration on macOS.
        if resp.has_privilege.unwrap_or(false) {
            Ok(TunnelState::Disconnected)
        } else {
            Ok(TunnelState::NeedsPermission)
        }
    }

    async fn request_permission(&self) -> Result<(), TransportError> {
        // There is no per-platform consent flow here yet: privilege comes from how the core was
        // installed (a capability, a service), not from something the app can ask for at runtime.
        Err(TransportError::Unavailable(
            "this build cannot grant itself privilege; install the core with the required permission"
                .into(),
        ))
    }

    async fn start(&self, request: &BuildRequest) -> Result<(), TransportError> {
        let cfg = config::runtime::build(request).map_err(TransportError::Core)?;
        cfg.check(&self.core).await.map_err(TransportError::Core)?;
        let resp: gen::ErrorResp = self.core.call(method::START, &gen::LoadConfigReq {
            disable_stats: Some(false),
            tun_ipv4_cidr: Some(if request.mode == Mode::Vpn { request.tun.ipv4_cidr.clone() } else { String::new() }),
            ..cfg.load_request()
        }).await?;
        Self::or_err(resp)
    }

    async fn stop(&self) -> Result<(), TransportError> {
        let resp: gen::ErrorResp = self.core.call(method::STOP, &gen::EmptyReq {}).await?;
        Self::or_err(resp)
    }

    async fn state(&self) -> Result<TunnelState, TransportError> {
        if !self.core.is_connected().await {
            return Ok(TunnelState::Disconnected);
        }
        // The core exposes no "is the tunnel up" RPC; a running box answers QueryStats with the
        // outbound tags it built, so the proxy tag's presence stands in for it.
        let resp: gen::QueryStatsResp = self
            .core
            .call(method::QUERY_STATS, &gen::EmptyReq {})
            .await?;
        Ok(if resp.ups.contains_key(config::tags::PROXY) {
            TunnelState::Connected
        } else {
            TunnelState::Disconnected
        })
    }

    async fn throughput(&self) -> Result<Throughput, TransportError> {
        let resp: gen::QueryStatsResp = self
            .core
            .call(method::QUERY_STATS, &gen::EmptyReq {})
            .await?;
        Ok(Throughput {
            uplink: resp.ups.get(config::tags::PROXY).copied().unwrap_or(0),
            downlink: resp.downs.get(config::tags::PROXY).copied().unwrap_or(0),
        })
    }
}
