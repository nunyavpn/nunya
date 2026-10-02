//! Builds sing-box routing. `runtime` prepares the final sing-box/Xray pair for the core.
//!
//! This is deliberately small. The Qt build generates configs in `src/configs/generate.cpp`
//! (~2700 lines) because it supports 28 protocols, three transport modes and chained outbounds.
//! A TUN-only client with a handful of protocols needs far less, and the long-term plan is to move
//! generation into the core behind a `GenerateConfig` RPC so every client shares one implementation.
//! Until then, this covers exactly what the vertical slice needs and nothing more.
//!
//! Anything emitted here is validated by the core's own `CheckConfig` before it is started, so a
//! mistake surfaces as a readable error rather than a half-up tunnel.

pub mod runtime;

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

/// The V2Ray transports the client runs (XHTTP is delegated to Xray by `runtime`).
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
    Xhttp,
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
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub extra: Map<String, Value>,
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

/// The ad blocker's and the anti-tracker's switches, as the settings have them.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct BlockOptions {
    #[serde(default)]
    pub ads: bool,
    #[serde(default)]
    pub trackers: bool,
}

/// A block list on disk, which the core reads for itself (`blocklists.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockList {
    pub tag: &'static str,
    /// `binary` for a `.srs` file, `source` for JSON.
    pub format: &'static str,
    pub path: String,
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
    /// Which blockers are switched on.
    #[serde(default)]
    pub block: BlockOptions,
    /// The lists those switches name that are on disk, filled in on this side by
    /// `blocklists::on_disk`: the frontend knows the switches, only this side knows which lists
    /// have arrived. A switch whose list has not is left out rather than failing the connection.
    #[serde(skip)]
    pub block_lists: Vec<BlockList>,
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
        // runtime::prepare replaces this with the authenticated Xray bridge before any RPC.
        // Keeping its real type here makes an unprepared config fail instead of falling back to TCP.
        TransportKind::Xhttp => { m.insert("type".into(), json!("xhttp")); }
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

    let block_tags: Vec<&str> = req.block_lists.iter().map(|l| l.tag).collect();

    // ---------------------------------------------------------------- dns
    let mut dns_rules: Vec<Value> = Vec::new();
    if !block_tags.is_empty() {
        // First, so a blocked name is refused even when the bypass list would resolve it. NXDOMAIN
        // rather than the REFUSED a plain `reject` answers: a resolver told the name does not
        // exist stops there, one told REFUSED goes on to ask its next server.
        dns_rules.push(json!({
            "rule_set": block_tags,
            "action": "predefined",
            "rcode": "NXDOMAIN",
        }));
    }
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

    // By the sniffed name, for connections made without asking the resolver: an address cached or
    // hard-coded, or a name handed straight to the proxy listener. Before the bypass list, which
    // decides how traffic leaves, not whether it may.
    if !block_tags.is_empty() {
        route_rules.push(json!({ "rule_set": block_tags, "action": "reject" }));
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
    if !req.block_lists.is_empty() {
        config["route"]["rule_set"] = json!(req
            .block_lists
            .iter()
            .map(block_rule_set)
            .collect::<Vec<_>>());
    }

    config
}

/// A list on disk as `route.rule_set` names it. Local, not `remote`: see `blocklists.rs`.
pub fn block_rule_set(list: &BlockList) -> Value {
    json!({
        "type": "local",
        "tag": list.tag,
        "format": list.format,
        "path": list.path,
    })
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
mod tests;
