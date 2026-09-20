//! The subprocess transport: the core runs as a child process and we drive it over a unix socket.
//!
//! This is the Linux and Windows path, and it is what the client used on macOS before the
//! NetworkExtension work. It needs the core itself to hold the privilege required to create a TUN,
//! which is exactly the property macOS gets to avoid.

use async_trait::async_trait;

use super::{Throughput, TransportError, TunnelState, TunnelTransport};
use crate::config::{self, BuildRequest, Mode};
use crate::rpc::{gen, method, CoreLink, LinkError};

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
    link: std::sync::Arc<CoreLink>,
}

impl SubprocessTransport {
    pub fn new(link: std::sync::Arc<CoreLink>) -> Self {
        Self { link }
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
        if !self.link.is_connected().await {
            return Err(TransportError::Unavailable("the core is not running".into()));
        }

        // A local listener binds an unprivileged port and creates no interface, so the privilege
        // question below simply does not arise. This is the whole reason proxy mode works today
        // on a machine where the TUN path does not.
        if mode == Mode::Proxy {
            return Ok(TunnelState::Disconnected);
        }

        let resp: gen::IsPrivilegedResponse = self
            .link
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
        let cfg = config::build(request);
        let resp: gen::ErrorResp = self
            .link
            .call(
                method::START,
                &gen::LoadConfigReq {
                    core_config: Some(cfg.to_string()),
                    disable_stats: Some(false),
                    tun_ipv4_cidr: Some(request.tun.ipv4_cidr.clone()),
                    // Both are sent explicitly because the core's Start dereferences them without
                    // a nil check (`*in.NeedExtraProcess` and `*in.NeedXray` in its
                    // internal/rpc/lifecycle.go), so leaving either unset panics the core rather
                    // than returning an error. Its CheckConfig reads the same fields through the
                    // generated nil-safe getters, which is why a config can validate cleanly and
                    // then bring the whole core down at Start.
                    //
                    // Neither feature is one this client uses: there is no extra process, and Xray
                    // is not wired up.
                    need_extra_process: Some(false),
                    need_xray: Some(false),
                    ..Default::default()
                },
            )
            .await?;
        Self::or_err(resp)
    }

    async fn stop(&self) -> Result<(), TransportError> {
        let resp: gen::ErrorResp = self.link.call(method::STOP, &gen::EmptyReq {}).await?;
        Self::or_err(resp)
    }

    async fn state(&self) -> Result<TunnelState, TransportError> {
        if !self.link.is_connected().await {
            return Ok(TunnelState::Disconnected);
        }
        // The core exposes no "is the tunnel up" RPC; a running box answers QueryStats with the
        // outbound tags it built, so the proxy tag's presence stands in for it.
        let resp: gen::QueryStatsResp = self
            .link
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
            .link
            .call(method::QUERY_STATS, &gen::EmptyReq {})
            .await?;
        Ok(Throughput {
            uplink: resp.ups.get(config::tags::PROXY).copied().unwrap_or(0),
            downlink: resp.downs.get(config::tags::PROXY).copied().unwrap_or(0),
        })
    }
}
