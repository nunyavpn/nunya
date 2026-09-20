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
    pub const TUN_IN: &str = "tun-in";
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

/// A VLESS server. The slice only needs one protocol to prove the path end to end; VMess, Trojan,
/// Shadowsocks, Hysteria2 and TUIC slot in beside this as separate variants.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VlessProfile {
    pub name: String,
    pub server: String,
    pub port: u16,
    pub uuid: String,
    #[serde(default)]
    pub flow: String,
    #[serde(default)]
    pub tls: TlsOptions,
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
    pub profile: VlessProfile,
    #[serde(default)]
    pub tun: TunOptions,
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

fn vless_outbound(p: &VlessProfile) -> Value {
    let mut ob = Map::new();
    ob.insert("type".into(), json!("vless"));
    ob.insert("tag".into(), json!(tags::PROXY));
    ob.insert("server".into(), json!(p.server));
    ob.insert("server_port".into(), json!(p.port));
    ob.insert("uuid".into(), json!(p.uuid));
    if !p.flow.is_empty() {
        ob.insert("flow".into(), json!(p.flow));
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
    // interface_name is left unset on macOS so the system assigns the next utun number, which is
    // what generate.cpp does too.
    #[cfg(not(target_os = "macos"))]
    tun.insert("interface_name".into(), json!("nunya-tun"));

    // ---------------------------------------------------------------- route
    let mut route_rules: Vec<Value> = vec![
        // Sniff first: the rules below match on a domain that only exists after sniffing.
        json!({ "action": "sniff" }),
        json!({ "protocol": "dns", "action": "hijack-dns" }),
        // The tunnel carries everything, so without this the LAN becomes unreachable.
        json!({ "ip_is_private": true, "outbound": tags::DIRECT }),
    ];
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

    json!({
        "log": { "level": req.log_level, "timestamp": true },
        "dns": dns,
        "inbounds": [Value::Object(tun)],
        "outbounds": [
            vless_outbound(&req.profile),
            { "type": "direct", "tag": tags::DIRECT },
        ],
        "route": {
            "rules": route_rules,
            "final": tags::PROXY,
            "auto_detect_interface": true,
        },
        // The mere presence of clash_api is what makes sing-box build its traffic manager
        // (see needClashAPI in the core's `internal/boxbox/box.go`), which is what QueryStats reads.
        // Leaving external_controller unset gets the counters without opening a control port.
        "experimental": { "clash_api": {} },
    })
}

/// Tag for the nth server in a latency-test config.
///
/// Results come back keyed by tag, so this is how a measurement finds its way back to the row that
/// asked for it.
pub fn test_tag(index: usize) -> String {
    format!("t{index}")
}

/// Builds a config for measuring latency, and the tags to measure.
///
/// Deliberately not the tunnel config: there is no TUN inbound, because a latency test must not
/// touch the system's routing. It creates an interface, changes the default route, and would
/// interrupt whatever the user is doing — to answer a question that only needs an outbound dial.
///
/// The core dials each tagged outbound directly, so no route table is needed either.
pub fn build_test(profiles: &[VlessProfile]) -> (Value, Vec<String>) {
    let mut outbounds: Vec<Value> = Vec::with_capacity(profiles.len() + 1);
    let mut tags = Vec::with_capacity(profiles.len());

    for (index, profile) in profiles.iter().enumerate() {
        let tag = test_tag(index);
        let mut outbound = vless_outbound(profile);
        if let Some(object) = outbound.as_object_mut() {
            object.insert("tag".into(), json!(tag));
        }
        outbounds.push(outbound);
        tags.push(tag);
    }

    outbounds.push(json!({ "type": "direct", "tag": tags::DIRECT }));

    let config = json!({
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

    (config, tags)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> BuildRequest {
        BuildRequest {
            profile: VlessProfile {
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
}
