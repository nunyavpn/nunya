//! Builds the sing-box configuration the core is handed.
//!
//! This is deliberately small. The Qt build generates configs in `src/configs/generate.cpp`
//! (~2700 lines) because it supports 28 protocols, three transport modes and chained outbounds.
//! A TUN-only client with a handful of protocols needs far less, and the long-term plan is to move
//! generation into the core behind a `GenerateConfig` RPC so every client shares one implementation.
//! Until then, this covers exactly what the vertical slice needs and nothing more.
//!
//! Anything emitted here is validated by the core's own `CheckConfig` before it is started, so a
//! mistake surfaces as a readable error rather than a half-up tunnel.

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

/// Outbound tags. The route table and the stats reader both key off these, so they live in one
/// place rather than being spelled out at each use.
pub mod tags {
    pub const PROXY: &str = "proxy";
    pub const DIRECT: &str = "direct";
    pub const BLOCK: &str = "block";
    pub const TUN_IN: &str = "tun-in";
    pub const MIXED_IN: &str = "mixed-in";
    pub const DNS_REMOTE: &str = "dns-remote";
    pub const DNS_DIRECT: &str = "dns-direct";
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Reality {
    pub public_key: String,
    #[serde(default)]
    pub short_id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TlsOptions {
    pub enabled: bool,
    #[serde(default)]
    pub sni: String,
    #[serde(default)]
    pub insecure: bool,
    #[serde(default)]
    pub alpn: Vec<String>,
    #[serde(default)]
    pub fingerprint: String,
    #[serde(default)]
    pub reality: Option<Reality>,
}

/// The two protocols this build runs.
///
/// They share a shape — a UUID, a TLS layer and a V2Ray transport — which is why one struct covers
/// both. Trojan, Shadowsocks, Hysteria2 and TUIC are different outbound types and would each need
/// their own fields, so they belong in their own variants rather than here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    #[default]
    Vless,
    Vmess,
    /// Same stream shape as the two above — TLS and a V2Ray transport — but authenticated with a
    /// password rather than a UUID.
    Trojan,
    /// Not a variation on the two above: no UUID, no TLS, no V2Ray transport, and it is an
    /// interface rather than a dialer. Its fields live in `Profile::wireguard`.
    Wireguard,
}

/// The V2Ray transports sing-box implements.
///
/// `Tcp` is the absence of a transport rather than one of them: sing-box expects no `transport` key
/// at all for plain TCP, and emitting `{"type":"tcp"}` is rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportKind {
    #[default]
    Tcp,
    Ws,
    Grpc,
    Http,
    Httpupgrade,
    Quic,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Transport {
    #[serde(default)]
    pub kind: TransportKind,
    #[serde(default)]
    pub path: String,
    /// The Host header for ws and httpupgrade; the authority for http.
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub service_name: String,
    #[serde(default)]
    pub method: String,
    #[serde(default)]
    pub max_early_data: u32,
    #[serde(default)]
    pub early_data_header: String,
}

/// One server.
///
/// What a WireGuard peer needs that nothing else does.
///
/// Kept in its own struct hanging off `Profile` rather than flattened into it: none of these
/// fields mean anything to VLESS or VMess, and a profile saved before WireGuard existed has no
/// business growing six empty strings.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireguardOptions {
    pub private_key: String,
    pub peer_public_key: String,
    /// The interface's own addresses, as CIDRs. WireGuard has no DHCP; the peer assigns these
    /// out of band and the client must state them.
    #[serde(default)]
    pub local_address: Vec<String>,
    /// Cloudflare WARP's client identifier. Three bytes prepended to every handshake; empty for
    /// an ordinary peer, and wrong values are simply dropped by the server with no error.
    #[serde(default)]
    pub reserved: Vec<u8>,
    #[serde(default)]
    pub mtu: u32,
    /// Seconds between keepalives, or 0 to leave it to the core.
    #[serde(default)]
    pub keepalive: u32,
}

/// Every field added since the first version carries `serde(default)`, because a profile saved
/// before it existed is still on disk and must not fail to load: an old payload deserialises as
/// VLESS over plain TCP, which is exactly what it was.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    #[serde(default)]
    pub protocol: Protocol,
    pub name: String,
    pub server: String,
    pub port: u16,
    pub uuid: String,
    /// VLESS only.
    #[serde(default)]
    pub flow: String,
    /// VMess only: the cipher.
    #[serde(default)]
    pub security: String,
    /// VMess only.
    #[serde(default)]
    pub alter_id: u32,
    /// Trojan only.
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub tls: TlsOptions,
    #[serde(default)]
    pub transport: Transport,
    /// WireGuard only.
    #[serde(default)]
    pub wireguard: Option<WireguardOptions>,
}

/// How the user's traffic reaches the core.
///
/// These are not two settings of one thing; they are different claims about coverage. A TUN takes
/// the whole device, so "you are in the tunnel" is true of everything. A local listener takes only
/// what is pointed at it, so the same sentence would be a lie — which is why the mode is carried
/// all the way to the status card rather than being a detail of config generation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// A TUN interface: every connection on the device, whether or not it knows about the proxy.
    Vpn,
    /// A local SOCKS and HTTP listener. Needs no privilege, and covers only what is configured
    /// to use it.
    #[default]
    Proxy,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Vpn => "vpn",
            Mode::Proxy => "proxy",
        }
    }
}

/// The local listener, in proxy mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyOptions {
    pub port: u16,
    /// Binds every interface rather than loopback, so other machines can use it too. Off by
    /// default: an open proxy on a shared network is one anyone on it can route through.
    pub allow_lan: bool,
}

impl Default for ProxyOptions {
    fn default() -> Self {
        Self {
            // What most clients in this family listen on, so an existing browser profile or
            // shell alias pointed at a previous client keeps working.
            port: 2080,
            allow_lan: false,
        }
    }
}

impl ProxyOptions {
    fn listen(&self) -> &'static str {
        if self.allow_lan {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunOptions {
    pub ipv4_cidr: String,
    pub mtu: u32,
    /// "system" is faster; "gvisor" is the portable fallback.
    pub stack: String,
    pub strict_route: bool,
    pub ipv6: bool,
}

impl Default for TunOptions {
    fn default() -> Self {
        Self {
            // Matches the Qt build's default so an existing user's routing assumptions hold.
            ipv4_cidr: "172.19.0.1/24".to_string(),
            mtu: 1500,
            stack: "system".to_string(),
            strict_route: true,
            ipv6: false,
        }
    }
}

/// One entry from the bypass list: traffic that must leave on the physical link.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "lowercase")]
pub enum BypassRule {
    /// Matches the name and everything under it.
    Domain(String),
    /// A single address.
    Address(String),
    /// A CIDR range.
    Range(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildRequest {
    pub profile: Profile,
    #[serde(default)]
    pub mode: Mode,
    #[serde(default)]
    pub tun: TunOptions,
    #[serde(default)]
    pub proxy: ProxyOptions,
    #[serde(default)]
    pub bypass: Vec<BypassRule>,
    /// Resolver used for names that are not bypassed. Queries go through the tunnel.
    #[serde(default = "default_dns")]
    pub dns: String,
    #[serde(default = "default_log_level")]
    pub log_level: String,
}

fn default_dns() -> String {
    "https://1.1.1.1/dns-query".to_string()
}

fn default_log_level() -> String {
    "info".to_string()
}

/// Splits a DNS address into the object shape this sing-box expects.
///
/// Since 1.12 a DNS server is `{type, server, server_port?, path?}` rather than a bare URL string,
/// which is what `src/configs/generate.cpp` emits too.
///
/// `detour` of `None` means the resolver is dialled directly. sing-box treats an absent detour as
/// "do not route this", so naming the direct outbound explicitly is a no-op it rejects outright:
/// *"detour to an empty direct outbound makes no sense"*. Only the resolver that must travel
/// through the tunnel names one.
fn dns_server(address: &str, tag: &str, detour: Option<&str>) -> Value {
    let (kind, rest) = match address {
        a if a.starts_with("https://") => ("https", &a[8..]),
        a if a.starts_with("h3://") => ("h3", &a[5..]),
        a if a.starts_with("tls://") => ("tls", &a[6..]),
        a if a.starts_with("quic://") => ("quic", &a[7..]),
        a if a.starts_with("udp://") => ("udp", &a[6..]),
        a => ("udp", a),
    };

    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], Some(&rest[i..])),
        None => (rest, None),
    };
    // Bare IPv6 would need bracket handling; the resolvers we offer are v4 or hostnames.
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) if p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => {
            (h, p.parse::<u16>().ok())
        }
        _ => (hostport, None),
    };

    let mut obj = Map::new();
    obj.insert("tag".into(), json!(tag));
    obj.insert("type".into(), json!(kind));
    obj.insert("server".into(), json!(host));
    if let Some(p) = port {
        obj.insert("server_port".into(), json!(p));
    }
    if let Some(p) = path {
        obj.insert("path".into(), json!(p));
    }
    if let Some(detour) = detour {
        obj.insert("detour".into(), json!(detour));
    }
    Value::Object(obj)
}

/// The `transport` object, or `None` for plain TCP.
///
/// sing-box takes the absence of the key as "plain TCP"; there is no `{"type":"tcp"}` to emit, and
/// sending one fails validation. Empty fields are omitted rather than sent blank, because a blank
/// `path` is not the same as no path to a server matching on it.
fn transport_value(t: &Transport) -> Option<Value> {
    let mut m = Map::new();

    match t.kind {
        TransportKind::Tcp => return None,
        TransportKind::Ws => {
            m.insert("type".into(), json!("ws"));
            if !t.path.is_empty() {
                m.insert("path".into(), json!(t.path));
            }
            if !t.host.is_empty() {
                m.insert("headers".into(), json!({ "Host": t.host }));
            }
            // Early data is carried in a header whose name both ends have to agree on, and the
            // default is the one every implementation settled on without it being specified.
            if t.max_early_data > 0 {
                m.insert("max_early_data".into(), json!(t.max_early_data));
                let header = if t.early_data_header.is_empty() {
                    "Sec-WebSocket-Protocol"
                } else {
                    &t.early_data_header
                };
                m.insert("early_data_header_name".into(), json!(header));
            }
        }
        TransportKind::Grpc => {
            m.insert("type".into(), json!("grpc"));
            if !t.service_name.is_empty() {
                m.insert("service_name".into(), json!(t.service_name));
            }
        }
        TransportKind::Http => {
            m.insert("type".into(), json!("http"));
            if !t.host.is_empty() {
                // A list, because the HTTP transport accepts several and picks one per request.
                m.insert("host".into(), json!([t.host]));
            }
            if !t.path.is_empty() {
                m.insert("path".into(), json!(t.path));
            }
            if !t.method.is_empty() {
                m.insert("method".into(), json!(t.method));
            }
        }
        TransportKind::Httpupgrade => {
            m.insert("type".into(), json!("httpupgrade"));
            if !t.host.is_empty() {
                m.insert("host".into(), json!(t.host));
            }
            if !t.path.is_empty() {
                m.insert("path".into(), json!(t.path));
            }
        }
        TransportKind::Quic => {
            m.insert("type".into(), json!("quic"));
        }
    }

    Some(Value::Object(m))
}

/// Where a proxy node belongs in the config.
///
/// sing-box moved WireGuard out of `outbounds` and into `endpoints`: it is an interface with its
/// own addresses rather than a dialer, and this core rejects the old outbound form outright
/// (*unknown field "local_address"*). Everything else this build runs is still an outbound, so
/// the distinction is made once here instead of at each call site.
pub enum ProxyNode {
    Outbound(Value),
    Endpoint(Value),
}

/// Builds the node that carries the user's traffic, under the tag the router will name.
pub fn proxy_node(p: &Profile, tag: &str) -> ProxyNode {
    match p.protocol {
        Protocol::Wireguard => ProxyNode::Endpoint(wireguard_endpoint(p, tag)),
        _ => ProxyNode::Outbound(proxy_outbound(p, tag)),
    }
}

/// A WireGuard interface and its single peer.
///
/// `allowed_ips` is everything, because this is the tunnel: a peer that claimed less would leave
/// the rest of the traffic with nowhere to go, and the route table above already decides what
/// reaches the interface in the first place.
fn wireguard_endpoint(p: &Profile, tag: &str) -> Value {
    let wg = p.wireguard.clone().unwrap_or_default();

    let mut peer = Map::new();
    peer.insert("address".into(), json!(p.server));
    peer.insert("port".into(), json!(p.port));
    peer.insert("public_key".into(), json!(wg.peer_public_key));
    peer.insert("allowed_ips".into(), json!(["0.0.0.0/0", "::/0"]));
    if !wg.reserved.is_empty() {
        peer.insert("reserved".into(), json!(wg.reserved));
    }
    if wg.keepalive > 0 {
        peer.insert(
            "persistent_keepalive_interval".into(),
            json!(wg.keepalive),
        );
    }

    let mut ep = Map::new();
    ep.insert("type".into(), json!("wireguard"));
    ep.insert("tag".into(), json!(tag));
    ep.insert("address".into(), json!(wg.local_address));
    ep.insert("private_key".into(), json!(wg.private_key));
    ep.insert("peers".into(), json!([Value::Object(peer)]));
    if wg.mtu > 0 {
        ep.insert("mtu".into(), json!(wg.mtu));
    }

    Value::Object(ep)
}

fn proxy_outbound(p: &Profile, tag: &str) -> Value {
    let mut ob = Map::new();
    ob.insert("tag".into(), json!(tag));
    ob.insert("server".into(), json!(p.server));
    ob.insert("server_port".into(), json!(p.port));

    match p.protocol {
        Protocol::Vless => {
            ob.insert("type".into(), json!("vless"));
            ob.insert("uuid".into(), json!(p.uuid));
            if !p.flow.is_empty() {
                ob.insert("flow".into(), json!(p.flow));
            }
        }
        // Routed to `wireguard_endpoint` before reaching here.
        Protocol::Wireguard => unreachable!("WireGuard is an endpoint, not an outbound"),
        Protocol::Trojan => {
            ob.insert("type".into(), json!("trojan"));
            ob.insert("password".into(), json!(p.password));
        }
        Protocol::Vmess => {
            ob.insert("type".into(), json!("vmess"));
            ob.insert("uuid".into(), json!(p.uuid));
            // "auto" is what a link means when it says nothing, and what sing-box would pick.
            let cipher = if p.security.is_empty() { "auto" } else { &p.security };
            ob.insert("security".into(), json!(cipher));
            // Only sent when non-zero: a present alter_id selects the pre-AEAD scheme, which modern
            // servers reject outright.
            if p.alter_id > 0 {
                ob.insert("alter_id".into(), json!(p.alter_id));
            }
        }
    }

    if p.tls.enabled {
        let mut tls = Map::new();
        tls.insert("enabled".into(), json!(true));
        if !p.tls.sni.is_empty() {
            tls.insert("server_name".into(), json!(p.tls.sni));
        }
        if p.tls.insecure {
            tls.insert("insecure".into(), json!(true));
        }
        if !p.tls.alpn.is_empty() {
            tls.insert("alpn".into(), json!(p.tls.alpn));
        }
        if !p.tls.fingerprint.is_empty() {
            tls.insert(
                "utls".into(),
                json!({ "enabled": true, "fingerprint": p.tls.fingerprint }),
            );
        }
        if let Some(r) = &p.tls.reality {
            let mut reality = Map::new();
            reality.insert("enabled".into(), json!(true));
            reality.insert("public_key".into(), json!(r.public_key));
            if !r.short_id.is_empty() {
                reality.insert("short_id".into(), json!(r.short_id));
            }
            tls.insert("reality".into(), Value::Object(reality));
        }
        ob.insert("tls".into(), Value::Object(tls));
    }

    if let Some(transport) = transport_value(&p.transport) {
        ob.insert("transport".into(), transport);
    }

    Value::Object(ob)
}

/// Splits the bypass list into the two shapes the router matches on.
fn partition_bypass(rules: &[BypassRule]) -> (Vec<String>, Vec<String>) {
    let mut domains = Vec::new();
    let mut cidrs = Vec::new();
    for r in rules {
        match r {
            BypassRule::Domain(d) => domains.push(d.trim_start_matches("*.").to_string()),
            // A bare address is just a /32; keeping one field avoids a second rule clause.
            BypassRule::Address(a) => cidrs.push(format!("{a}/32")),
            BypassRule::Range(c) => cidrs.push(c.clone()),
        }
    }
    (domains, cidrs)
}

/// Produces the full config object.
pub fn build(req: &BuildRequest) -> Value {
    let (bypass_domains, bypass_cidrs) = partition_bypass(&req.bypass);

    // ---------------------------------------------------------------- dns
    let mut dns_rules: Vec<Value> = Vec::new();
    if !bypass_domains.is_empty() {
        // A bypassed domain must also RESOLVE outside the tunnel. Route it direct but resolve it
        // through the proxy and the query itself leaks the very name being kept local.
        dns_rules.push(json!({
            "domain_suffix": bypass_domains,
            "server": tags::DNS_DIRECT,
        }));
    }

    let dns = json!({
        "servers": [
            dns_server(&req.dns, tags::DNS_REMOTE, Some(tags::PROXY)),
            // Bootstrap for the bypass list only; never the final resolver.
            dns_server("udp://1.1.1.1", tags::DNS_DIRECT, None),
        ],
        "rules": dns_rules,
        "final": tags::DNS_REMOTE,
        "strategy": if req.tun.ipv6 { "prefer_ipv4" } else { "ipv4_only" },
        "independent_cache": true,
    });

    // ---------------------------------------------------------------- inbound
    let inbound = match req.mode {
        Mode::Vpn => {
            let mut addresses = vec![req.tun.ipv4_cidr.clone()];
            if req.tun.ipv6 {
                addresses.push("fdfe:dcba:9876::1/126".to_string());
            }

            let mut tun = Map::new();
            tun.insert("type".into(), json!("tun"));
            tun.insert("tag".into(), json!(tags::TUN_IN));
            tun.insert("address".into(), json!(addresses));
            tun.insert("mtu".into(), json!(req.tun.mtu));
            tun.insert("auto_route".into(), json!(true));
            tun.insert("strict_route".into(), json!(req.tun.strict_route));
            tun.insert("stack".into(), json!(req.tun.stack));
            // interface_name is left unset on macOS so the system assigns the next utun number,
            // which is what generate.cpp does too.
            #[cfg(not(target_os = "macos"))]
            tun.insert("interface_name".into(), json!("nunya-tun"));
            Value::Object(tun)
        }
        // One listener speaking both SOCKS and HTTP, which is what `mixed` is. Two inbounds on
        // two ports would be two things for the user to configure and two ways to get it wrong.
        Mode::Proxy => {
            let mut mixed = Map::new();
            mixed.insert("type".into(), json!("mixed"));
            mixed.insert("tag".into(), json!(tags::MIXED_IN));
            mixed.insert("listen".into(), json!(req.proxy.listen()));
            mixed.insert("listen_port".into(), json!(req.proxy.port));
            Value::Object(mixed)
        }
    };

    // ---------------------------------------------------------------- route
    let mut route_rules: Vec<Value> = vec![
        // Sniff first: the rules below match on a domain that only exists after sniffing.
        json!({ "action": "sniff" }),
    ];

    // Only a TUN sees the system's DNS traffic to hijack. A local listener is handed destinations
    // that are already resolved, or names it resolves itself through the `dns` block below.
    if req.mode == Mode::Vpn {
        route_rules.push(json!({ "protocol": "dns", "action": "hijack-dns" }));
    }

    // The tunnel carries everything, so without this the LAN becomes unreachable. Harmless in
    // proxy mode, and it keeps a browser configured to use the proxy able to reach a local
    // service through it.
    route_rules.push(json!({ "ip_is_private": true, "outbound": tags::DIRECT }));
    if !bypass_domains.is_empty() {
        route_rules.push(json!({
            "domain_suffix": bypass_domains,
            "outbound": tags::DIRECT,
        }));
    }
    if !bypass_cidrs.is_empty() {
        route_rules.push(json!({
            "ip_cidr": bypass_cidrs,
            "outbound": tags::DIRECT,
        }));
    }

    // ---------------------------------------------------------------- proxy
    // An endpoint is routed by tag exactly as an outbound is, so `route.final` below does not
    // care which of the two the profile produced.
    let mut outbounds: Vec<Value> = Vec::with_capacity(2);
    let mut endpoints: Vec<Value> = Vec::new();
    match proxy_node(&req.profile, tags::PROXY) {
        ProxyNode::Outbound(o) => outbounds.push(o),
        ProxyNode::Endpoint(e) => endpoints.push(e),
    }
    outbounds.push(json!({ "type": "direct", "tag": tags::DIRECT }));

    let mut config = json!({
        "log": { "level": req.log_level, "timestamp": true },
        "dns": dns,
        "inbounds": [inbound],
        "outbounds": outbounds,
        "route": {
            "rules": route_rules,
            "final": tags::PROXY,
            "auto_detect_interface": true,
        },
        // The mere presence of clash_api is what makes sing-box build its traffic manager
        // (see needClashAPI in the core's `internal/boxbox/box.go`), which is what QueryStats reads.
        // Leaving external_controller unset gets the counters without opening a control port.
        "experimental": { "clash_api": {} },
    });

    // Only when there is one: an empty `endpoints` is noise in a config a user may be reading to
    // decide whether to trust it.
    if !endpoints.is_empty() {
        config
            .as_object_mut()
            .expect("the config is an object")
            .insert("endpoints".into(), json!(endpoints));
    }

    config
}

/// Tag for the nth server in a latency-test config.
///
/// Results come back keyed by tag, so this is how a measurement finds its way back to the row that
/// asked for it.
pub fn test_tag(index: usize) -> String {
    format!("t{index}")
}

/// Tag for the nth probe inbound.
fn probe_tag(index: usize) -> String {
    format!("probe{index}")
}

/// Builds a config that puts every profile behind its own local port.
///
/// This exists because the core has no RPC that fetches a URL through a chosen outbound and hands
/// back the body. Its own geo calls (`IPTest`, `SpeedTest` with `only_country`) do that
/// internally, but against endpoints compiled into the core — `api.ip2location.io` and
/// `speedtest.net`, both behind Cloudflare, and therefore both unreachable from a Cloudflare
/// Workers proxy, which is what most free subscriptions are. Measured against a real
/// subscription they answered for two servers out of ten.
///
/// So the client asks the question itself, and needs somewhere to send the request. One `mixed`
/// inbound per profile, each pinned by a route rule to that profile's outbound, is that.
///
/// There is no TUN and no `final` to the proxy: a port that is not addressed carries nothing, so
/// this config cannot disturb anything even while it is running.
pub fn build_probe(profiles: &[Profile], ports: &[u16]) -> Value {
    let mut inbounds: Vec<Value> = Vec::with_capacity(profiles.len());
    let mut outbounds: Vec<Value> = Vec::with_capacity(profiles.len() + 1);
    let mut endpoints: Vec<Value> = Vec::new();
    let mut rules: Vec<Value> = Vec::with_capacity(profiles.len());

    for (index, (profile, port)) in profiles.iter().zip(ports).enumerate() {
        let outbound_tag = test_tag(index);
        let inbound_tag = probe_tag(index);

        inbounds.push(json!({
            "type": "mixed",
            "tag": inbound_tag,
            // Loopback only. These ports are open proxies for as long as the probe runs, and
            // that is a few seconds too long to offer them to the network.
            "listen": "127.0.0.1",
            "listen_port": port,
        }));

        match proxy_node(profile, &outbound_tag) {
            ProxyNode::Outbound(o) => outbounds.push(o),
            ProxyNode::Endpoint(e) => endpoints.push(e),
        }

        // What makes the port mean "this server" rather than "some server".
        rules.push(json!({ "inbound": [inbound_tag], "outbound": outbound_tag }));
    }

    // Not `direct`. A request that slips past the rules above would otherwise leave from this
    // machine, and the probe would report the user's own address as the server's exit — which it
    // once did. Refusing it makes that server's measurement fail, which is the truth.
    outbounds.push(json!({ "type": "block", "tag": tags::BLOCK }));

    let mut config = json!({
        "log": { "level": "error" },
        "dns": {
            "servers": [dns_server("udp://1.1.1.1", tags::DNS_DIRECT, None)],
            "final": tags::DNS_DIRECT,
            "strategy": "ipv4_only",
        },
        "inbounds": inbounds,
        "outbounds": outbounds,
        "route": { "rules": rules, "final": tags::BLOCK },
    });

    if !endpoints.is_empty() {
        config
            .as_object_mut()
            .expect("the config is an object")
            .insert("endpoints".into(), json!(endpoints));
    }

    config
}

/// Builds a config for measuring latency, and the tags to measure.
///
/// Deliberately not the tunnel config: there is no TUN inbound, because a latency test must not
/// touch the system's routing. It creates an interface, changes the default route, and would
/// interrupt whatever the user is doing — to answer a question that only needs an outbound dial.
///
/// The core dials each tagged outbound directly, so no route table is needed either.
pub fn build_test(profiles: &[Profile]) -> (Value, Vec<String>) {
    let mut outbounds: Vec<Value> = Vec::with_capacity(profiles.len() + 1);
    let mut tags = Vec::with_capacity(profiles.len());

    let mut endpoints: Vec<Value> = Vec::new();

    for (index, profile) in profiles.iter().enumerate() {
        let tag = test_tag(index);
        match proxy_node(profile, &tag) {
            ProxyNode::Outbound(o) => outbounds.push(o),
            ProxyNode::Endpoint(e) => endpoints.push(e),
        }
        tags.push(tag);
    }

    outbounds.push(json!({ "type": "direct", "tag": tags::DIRECT }));

    let mut config = json!({
        // Quiet: a test of fifty servers at info level buries the log it shares with the tunnel.
        "log": { "level": "error" },
        "dns": {
            "servers": [dns_server("udp://1.1.1.1", tags::DNS_DIRECT, None)],
            "final": tags::DNS_DIRECT,
            "strategy": "ipv4_only",
        },
        "outbounds": outbounds,
        "route": { "final": tags::DIRECT },
    });

    if !endpoints.is_empty() {
        config
            .as_object_mut()
            .expect("the config is an object")
            .insert("endpoints".into(), json!(endpoints));
    }

    (config, tags)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> BuildRequest {
        BuildRequest {
            mode: Mode::Vpn,
            proxy: ProxyOptions::default(),
            profile: Profile {
                name: "test".into(),
                server: "example.net".into(),
                port: 443,
                uuid: "8f3c9d2e-4a17-4b8e-9c21-7de5f0a63b14".into(),
                flow: "xtls-rprx-vision".into(),
                tls: TlsOptions {
                    enabled: true,
                    sni: "www.cloudflare.com".into(),
                    fingerprint: "chrome".into(),
                    reality: Some(Reality {
                        public_key: "abc".into(),
                        short_id: "0123".into(),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            tun: TunOptions::default(),
            bypass: vec![],
            dns: default_dns(),
            log_level: default_log_level(),
        }
    }

    #[test]
    fn doh_address_splits_into_the_object_form() {
        let v = dns_server("https://1.1.1.1/dns-query", "dns-remote", Some("proxy"));
        assert_eq!(v["type"], "https");
        assert_eq!(v["server"], "1.1.1.1");
        assert_eq!(v["path"], "/dns-query");
        assert_eq!(v["detour"], "proxy");
        assert!(v.get("server_port").is_none());
    }

    #[test]
    fn bare_address_defaults_to_udp() {
        let v = dns_server("8.8.8.8", "dns-direct", None);
        assert_eq!(v["type"], "udp");
        assert_eq!(v["server"], "8.8.8.8");
    }

    #[test]
    fn explicit_port_is_split_out() {
        let v = dns_server("udp://9.9.9.9:5353", "t", None);
        assert_eq!(v["server"], "9.9.9.9");
        assert_eq!(v["server_port"], 5353);
    }

    #[test]
    fn reality_and_utls_land_under_tls() {
        let cfg = build(&sample());
        let tls = &cfg["outbounds"][0]["tls"];
        assert_eq!(tls["reality"]["enabled"], true);
        assert_eq!(tls["reality"]["short_id"], "0123");
        assert_eq!(tls["utls"]["fingerprint"], "chrome");
        assert_eq!(cfg["outbounds"][0]["flow"], "xtls-rprx-vision");
    }

    /// sing-box rejects a detour that names a bare direct outbound, so only the tunnelled resolver
    /// carries one. Getting this wrong passes CheckConfig and then fails at Start.
    #[test]
    fn only_the_remote_resolver_names_a_detour() {
        let cfg = build(&sample());
        let servers = cfg["dns"]["servers"].as_array().unwrap();

        let remote = servers.iter().find(|s| s["tag"] == "dns-remote").unwrap();
        assert_eq!(remote["detour"], "proxy");

        let direct = servers.iter().find(|s| s["tag"] == "dns-direct").unwrap();
        assert!(
            direct.get("detour").is_none(),
            "dns-direct must not name a detour: {direct}"
        );
    }

    #[test]
    fn the_test_config_resolver_names_no_detour_either() {
        let (cfg, _) = build_test(&[sample().profile]);
        let servers = cfg["dns"]["servers"].as_array().unwrap();
        assert!(servers.iter().all(|s| s.get("detour").is_none()));
    }

    #[test]
    fn private_ranges_stay_direct() {
        let cfg = build(&sample());
        let rules = cfg["route"]["rules"].as_array().unwrap();
        assert!(rules
            .iter()
            .any(|r| r["ip_is_private"] == true && r["outbound"] == "direct"));
    }

    #[test]
    fn sniff_precedes_every_domain_rule() {
        let mut req = sample();
        req.bypass = vec![BypassRule::Domain("aparat.com".into())];
        let cfg = build(&req);
        let rules = cfg["route"]["rules"].as_array().unwrap();
        let sniff = rules.iter().position(|r| r["action"] == "sniff").unwrap();
        let domain = rules
            .iter()
            .position(|r| r.get("domain_suffix").is_some())
            .unwrap();
        assert!(sniff < domain, "domain rules cannot match before sniffing");
    }

    #[test]
    fn a_bypassed_domain_also_resolves_directly() {
        let mut req = sample();
        req.bypass = vec![BypassRule::Domain("*.bank.ir".into())];
        let cfg = build(&req);

        // Routed direct...
        let routed = cfg["route"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["domain_suffix"][0] == "bank.ir" && r["outbound"] == "direct");
        // ...and resolved direct, or the DNS query leaks the name we are keeping local.
        let resolved = cfg["dns"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["domain_suffix"][0] == "bank.ir" && r["server"] == "dns-direct");

        assert!(routed, "bypassed domain must route direct");
        assert!(resolved, "bypassed domain must also resolve direct");
    }

    #[test]
    fn a_bare_address_becomes_a_host_route() {
        let mut req = sample();
        req.bypass = vec![BypassRule::Address("8.8.8.8".into())];
        let cfg = build(&req);
        let has = cfg["route"]["rules"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["ip_cidr"][0] == "8.8.8.8/32");
        assert!(has);
    }

    #[test]
    fn a_test_config_has_one_tagged_outbound_per_server() {
        let a = sample().profile;
        let mut b = a.clone();
        b.server = "second.example.net".into();

        let (config, tags) = build_test(&[a, b]);
        assert_eq!(tags, vec!["t0", "t1"]);

        let outbounds = config["outbounds"].as_array().unwrap();
        // Two servers plus the direct outbound the DNS resolver uses.
        assert_eq!(outbounds.len(), 3);
        assert_eq!(outbounds[0]["tag"], "t0");
        assert_eq!(outbounds[1]["tag"], "t1");
        assert_eq!(outbounds[1]["server"], "second.example.net");
    }

    /// A latency test must not create a TUN: that changes the default route and would interrupt
    /// whatever the user is doing, to answer a question that only needs an outbound dial.
    #[test]
    fn a_test_config_never_creates_a_tunnel() {
        let (config, _) = build_test(&[sample().profile]);
        assert!(
            config.get("inbounds").is_none(),
            "a test config must not declare inbounds: {config}"
        );
    }

    #[test]
    fn testing_nothing_still_produces_a_valid_config() {
        let (config, tags) = build_test(&[]);
        assert!(tags.is_empty());
        // The direct outbound survives, so the config is still one the core will accept.
        assert_eq!(config["outbounds"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn clash_api_is_present_so_stats_exist_but_no_port_is_opened() {
        let cfg = build(&sample());
        let clash = &cfg["experimental"]["clash_api"];
        assert!(clash.is_object());
        assert!(
            clash.get("external_controller").is_none(),
            "a control port must not be opened just to read counters"
        );
    }
    /// A WebSocket link is the common CDN shape, and the one the first version rejected outright.
    #[test]
    fn websocket_carries_its_path_and_host_header() {
        let mut req = sample();
        req.profile.transport = Transport {
            kind: TransportKind::Ws,
            path: "/meeting".into(),
            host: "cdn.example.com".into(),
            ..Default::default()
        };

        let t = &build(&req)["outbounds"][0]["transport"];
        assert_eq!(t["type"], "ws");
        assert_eq!(t["path"], "/meeting");
        // The Host header is what a CDN routes on; sent as a header, not as a field of its own.
        assert_eq!(t["headers"]["Host"], "cdn.example.com");
        assert!(t.get("max_early_data").is_none());
    }

    #[test]
    fn early_data_brings_its_header_name_with_it() {
        let mut req = sample();
        req.profile.transport = Transport {
            kind: TransportKind::Ws,
            path: "/x".into(),
            max_early_data: 2048,
            ..Default::default()
        };

        let t = &build(&req)["outbounds"][0]["transport"];
        assert_eq!(t["max_early_data"], 2048);
        assert_eq!(t["early_data_header_name"], "Sec-WebSocket-Protocol");
    }

    /// sing-box takes a missing `transport` as plain TCP. Emitting `{"type":"tcp"}` fails
    /// validation, so the absence has to be real rather than an empty object.
    #[test]
    fn plain_tcp_emits_no_transport_key_at_all() {
        let cfg = build(&sample());
        assert!(
            cfg["outbounds"][0].get("transport").is_none(),
            "a TCP profile must not carry a transport: {}",
            cfg["outbounds"][0]
        );
    }

    #[test]
    fn grpc_sends_only_its_service_name() {
        let mut req = sample();
        req.profile.transport = Transport {
            kind: TransportKind::Grpc,
            service_name: "TunService".into(),
            // Set, and must not leak into a gRPC transport that has no use for them.
            path: "/ignored".into(),
            host: "ignored.example.com".into(),
            ..Default::default()
        };

        let t = &build(&req)["outbounds"][0]["transport"];
        assert_eq!(t["type"], "grpc");
        assert_eq!(t["service_name"], "TunService");
        assert!(t.get("path").is_none());
        assert!(t.get("headers").is_none());
    }

    #[test]
    fn the_http_transport_takes_a_list_of_hosts() {
        let mut req = sample();
        req.profile.transport = Transport {
            kind: TransportKind::Http,
            host: "h.example.com".into(),
            path: "/p".into(),
            ..Default::default()
        };

        let t = &build(&req)["outbounds"][0]["transport"];
        assert_eq!(t["type"], "http");
        assert_eq!(t["host"][0], "h.example.com");
        assert_eq!(t["path"], "/p");
    }

    #[test]
    fn httpupgrade_takes_a_bare_host() {
        let mut req = sample();
        req.profile.transport = Transport {
            kind: TransportKind::Httpupgrade,
            host: "h.example.com".into(),
            path: "/up".into(),
            ..Default::default()
        };

        let t = &build(&req)["outbounds"][0]["transport"];
        assert_eq!(t["type"], "httpupgrade");
        // A string here, unlike the http transport's list. Getting this wrong fails at Start.
        assert_eq!(t["host"], "h.example.com");
    }

    #[test]
    fn vmess_carries_a_cipher_and_no_flow() {
        let mut req = sample();
        req.profile.protocol = Protocol::Vmess;
        req.profile.security = "aes-128-gcm".into();
        req.profile.flow = "xtls-rprx-vision".into();

        let ob = &build(&req)["outbounds"][0];
        assert_eq!(ob["type"], "vmess");
        assert_eq!(ob["security"], "aes-128-gcm");
        // Flow is a VLESS concept; sending it on a VMess outbound is a config error.
        assert!(ob.get("flow").is_none());
    }

    #[test]
    fn vmess_defaults_its_cipher_rather_than_sending_an_empty_one() {
        let mut req = sample();
        req.profile.protocol = Protocol::Vmess;
        req.profile.security = String::new();
        assert_eq!(build(&req)["outbounds"][0]["security"], "auto");
    }

    /// A zero alter_id is the modern AEAD scheme. Sending the field at all selects the legacy one,
    /// which current servers reject, so absence is the correct encoding of zero.
    #[test]
    fn a_zero_alter_id_is_omitted() {
        let mut req = sample();
        req.profile.protocol = Protocol::Vmess;
        req.profile.alter_id = 0;
        assert!(build(&req)["outbounds"][0].get("alter_id").is_none());

        req.profile.alter_id = 4;
        assert_eq!(build(&req)["outbounds"][0]["alter_id"], 4);
    }

    /// Profiles saved before transports existed are still on disk. They must load as what they
    /// were — VLESS over plain TCP — rather than failing and emptying the user's server list.
    #[test]
    fn a_profile_saved_before_transports_still_deserialises() {
        let old = serde_json::json!({
            "name": "old",
            "server": "example.net",
            "port": 443,
            "uuid": "8f3c9d2e-4a17-4b8e-9c21-7de5f0a63b14",
            "flow": "xtls-rprx-vision",
            "tls": { "enabled": true, "sni": "example.net" }
        });

        let profile: Profile = serde_json::from_value(old).expect("old payload must still load");
        assert_eq!(profile.protocol, Protocol::Vless);
        assert_eq!(profile.transport.kind, TransportKind::Tcp);
        assert_eq!(profile.flow, "xtls-rprx-vision");
    }

    /// The exact payload the frontend sends, deserialised here.
    ///
    /// This is the seam most likely to break silently. Every field crossing it is renamed —
    /// `alterId` to `alter_id`, `maxEarlyData` to `max_early_data` — and because every new field
    /// carries `serde(default)` so old saved profiles still load, a name that stops matching does
    /// not error. It falls back to the default, and a WebSocket profile quietly becomes a TCP one
    /// that connects to nothing. Pasted verbatim from what `share.ts` emits.
    #[test]
    fn the_frontends_json_survives_the_crossing() {
        let from_ui = serde_json::json!({
            "protocol": "vless",
            "name": "UK",
            "server": "203.0.113.9",
            "port": 443,
            "uuid": "00000000-0000-4000-8000-000000000001",
            "flow": "",
            "security": "",
            "alterId": 0,
            "tls": {
                "enabled": true,
                "sni": "cdn.example.net",
                "insecure": false,
                "alpn": ["h3"],
                "fingerprint": "firefox",
                "reality": null
            },
            "transport": {
                "kind": "ws",
                "path": "/meeting-room",
                "host": "cdn.example.net",
                "serviceName": "",
                "method": "",
                "maxEarlyData": 0,
                "earlyDataHeader": ""
            }
        });

        let profile: Profile =
            serde_json::from_value(from_ui).expect("the frontend's shape must deserialise");

        assert_eq!(profile.protocol, Protocol::Vless);
        assert_eq!(profile.transport.kind, TransportKind::Ws);
        assert_eq!(profile.transport.path, "/meeting-room");
        assert_eq!(profile.transport.host, "cdn.example.net");
        assert_eq!(profile.tls.alpn, vec!["h3"]);
        assert_eq!(profile.tls.fingerprint, "firefox");

        // ...and comes out the far side as a transport the core will read.
        let mut req = sample();
        req.profile = profile;
        let t = &build(&req)["outbounds"][0]["transport"];
        assert_eq!(t["type"], "ws");
        assert_eq!(t["path"], "/meeting-room");
        assert_eq!(t["headers"]["Host"], "cdn.example.net");
    }

    /// The latency test dials the same outbound the tunnel would, or it measures something else.
    #[test]
    fn a_test_outbound_keeps_the_transport() {
        let mut profile = sample().profile;
        profile.transport = Transport {
            kind: TransportKind::Ws,
            path: "/x".into(),
            ..Default::default()
        };

        let (cfg, _) = build_test(&[profile]);
        assert_eq!(cfg["outbounds"][0]["transport"]["type"], "ws");
    }

    fn wireguard_profile() -> Profile {
        Profile {
            protocol: Protocol::Wireguard,
            name: "Warp".into(),
            server: "engage.cloudflareclient.com".into(),
            port: 2408,
            wireguard: Some(WireguardOptions {
                private_key: "SECRET".into(),
                peer_public_key: "PUBLIC".into(),
                local_address: vec!["172.16.0.2/32".into()],
                reserved: vec![216, 253, 3],
                mtu: 1280,
                keepalive: 5,
            }),
            ..Default::default()
        }
    }

    /// sing-box moved WireGuard into `endpoints`, and rejects the old outbound form outright.
    /// Emitting it under `outbounds` would fail at connect time with an opaque decode error.
    #[test]
    fn wireguard_is_an_endpoint_and_not_an_outbound() {
        let cfg = build(&BuildRequest {
            mode: Mode::Vpn,
            proxy: ProxyOptions::default(),
            profile: wireguard_profile(),
            tun: TunOptions::default(),
            bypass: vec![],
            dns: default_dns(),
            log_level: "info".into(),
        });

        let endpoint = &cfg["endpoints"][0];
        assert_eq!(endpoint["type"], "wireguard");
        assert_eq!(endpoint["tag"], tags::PROXY);
        assert_eq!(endpoint["private_key"], "SECRET");
        assert_eq!(endpoint["mtu"], 1280);
        assert_eq!(endpoint["address"][0], "172.16.0.2/32");

        let peer = &endpoint["peers"][0];
        assert_eq!(peer["address"], "engage.cloudflareclient.com");
        assert_eq!(peer["port"], 2408);
        assert_eq!(peer["public_key"], "PUBLIC");
        assert_eq!(peer["reserved"], serde_json::json!([216, 253, 3]));
        assert_eq!(peer["persistent_keepalive_interval"], 5);
        // Everything, because this is the tunnel.
        assert_eq!(peer["allowed_ips"], serde_json::json!(["0.0.0.0/0", "::/0"]));

        // The router names it exactly as it would an outbound.
        assert_eq!(cfg["route"]["final"], tags::PROXY);
        // And it must not also appear among the outbounds.
        let outbounds = cfg["outbounds"].as_array().unwrap();
        assert!(outbounds.iter().all(|o| o["type"] != "wireguard"));
    }

    /// Proxy mode is a different inbound, not a TUN with a flag flipped. Emitting a `tun`
    /// inbound here would create an interface and demand privilege for a mode whose entire point
    /// is needing neither.
    #[test]
    fn proxy_mode_listens_instead_of_building_a_tun() {
        let mut req = sample();
        req.mode = Mode::Proxy;
        let cfg = build(&req);

        let inbound = &cfg["inbounds"][0];
        assert_eq!(inbound["type"], "mixed");
        assert_eq!(inbound["tag"], tags::MIXED_IN);
        assert_eq!(inbound["listen"], "127.0.0.1");
        assert_eq!(inbound["listen_port"], 2080);
        assert_eq!(cfg["inbounds"].as_array().unwrap().len(), 1);

        // Nothing about a TUN survives into it.
        assert!(inbound.get("auto_route").is_none());
        assert!(inbound.get("strict_route").is_none());
        assert!(inbound.get("stack").is_none());
    }

    /// Only a TUN sees the system's DNS traffic, so hijacking it in proxy mode would be a rule
    /// matching something that never arrives.
    #[test]
    fn dns_is_hijacked_only_where_there_is_a_tun_to_hijack_it_from() {
        let vpn = build(&sample());
        let rules = vpn["route"]["rules"].as_array().unwrap();
        assert!(rules.iter().any(|r| r["action"] == "hijack-dns"));

        let mut req = sample();
        req.mode = Mode::Proxy;
        let proxy = build(&req);
        let rules = proxy["route"]["rules"].as_array().unwrap();
        assert!(!rules.iter().any(|r| r["action"] == "hijack-dns"));
        // Sniffing and the private-range rule still apply.
        assert!(rules.iter().any(|r| r["action"] == "sniff"));
        assert!(rules.iter().any(|r| r["ip_is_private"] == true));
    }

    /// Off by default, because an open proxy is one anyone on the network can route through.
    #[test]
    fn allow_lan_is_what_moves_the_listener_off_loopback() {
        let mut req = sample();
        req.mode = Mode::Proxy;
        assert_eq!(build(&req)["inbounds"][0]["listen"], "127.0.0.1");

        req.proxy.allow_lan = true;
        req.proxy.port = 1080;
        let cfg = build(&req);
        assert_eq!(cfg["inbounds"][0]["listen"], "0.0.0.0");
        assert_eq!(cfg["inbounds"][0]["listen_port"], 1080);
    }

    /// An empty `endpoints` key is noise in a config a user may be reading to decide whether to
    /// trust it.
    #[test]
    fn a_config_without_wireguard_has_no_endpoints_key() {
        let cfg = build(&BuildRequest {
            mode: Mode::Vpn,
            proxy: ProxyOptions::default(),
            profile: Profile::default(),
            tun: TunOptions::default(),
            bypass: vec![],
            dns: default_dns(),
            log_level: "info".into(),
        });
        assert!(cfg.get("endpoints").is_none());
    }

    /// Trojan authenticates with a password where VLESS and VMess carry a UUID. Sending a `uuid`
    /// field too would be an unknown field to the core, which refuses the whole config.
    #[test]
    fn trojan_carries_a_password_and_no_uuid() {
        let profile = Profile {
            protocol: Protocol::Trojan,
            server: "edge.example.net".into(),
            port: 443,
            password: "hunter2".into(),
            uuid: "should-not-be-emitted".into(),
            ..Default::default()
        };
        let cfg = build(&BuildRequest {
            mode: Mode::Vpn,
            proxy: ProxyOptions::default(),
            profile,
            tun: TunOptions::default(),
            bypass: vec![],
            dns: default_dns(),
            log_level: "info".into(),
        });

        let proxy = &cfg["outbounds"][0];
        assert_eq!(proxy["type"], "trojan");
        assert_eq!(proxy["password"], "hunter2");
        assert!(proxy.get("uuid").is_none());
    }
}
