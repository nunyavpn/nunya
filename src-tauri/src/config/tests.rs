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
        block: BlockOptions::default(),
        block_lists: vec![],
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
fn every_config_looks_server_addresses_up_with_the_system_resolver() {
    let mut proxy = sample();
    proxy.mode = Mode::Proxy;
    let configs = [
        build(&sample()),
        build(&proxy),
        build_test(&[sample().profile]).0,
        build_probe(&[sample().profile], &[20000]),
    ];
    for cfg in configs {
        let resolver = &cfg["route"]["default_domain_resolver"];
        assert_eq!(resolver["server"], tags::DNS_DIRECT);
        // Not the DNS block's ipv4_only, which would make an IPv6-only server unreachable.
        assert_eq!(resolver["strategy"], "prefer_ipv4");
        let direct = cfg["dns"]["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["tag"] == tags::DNS_DIRECT)
            .unwrap();
        assert_eq!(direct["type"], "local", "not a public resolver: {direct}");
    }
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

fn with_lists(mut req: BuildRequest) -> BuildRequest {
    req.block = BlockOptions { ads: true, trackers: true };
    req.block_lists = vec![
        BlockList { tag: "block-ads", format: "binary", path: "/data/blocklists/ads.srs".into() },
        BlockList {
            tag: "block-trackers",
            format: "source",
            path: "/data/blocklists/trackers.json".into(),
        },
    ];
    req
}

#[test]
fn a_blocked_name_is_refused_by_the_resolver_and_by_the_router() {
    let cfg = build(&with_lists(sample()));

    let sets = cfg["route"]["rule_set"].as_array().unwrap();
    assert_eq!(sets.len(), 2);
    assert_eq!(sets[0]["type"], "local", "never `remote`: a failed download would fail Start");
    assert_eq!(sets[0]["format"], "binary");
    assert_eq!(sets[1]["path"], "/data/blocklists/trackers.json");

    let dns = &cfg["dns"]["rules"][0];
    assert_eq!(dns["rule_set"], json!(["block-ads", "block-trackers"]));
    assert_eq!(dns["action"], "predefined");
    assert_eq!(dns["rcode"], "NXDOMAIN");

    let rules = cfg["route"]["rules"].as_array().unwrap();
    let sniff = rules.iter().position(|r| r["action"] == "sniff").unwrap();
    let reject = rules.iter().position(|r| r["action"] == "reject").unwrap();
    assert!(sniff < reject, "the rule matches sniffed names, so it must follow the sniff");
}

#[test]
fn blocking_wins_over_the_bypass_list() {
    let mut req = with_lists(sample());
    req.bypass = vec![BypassRule::Domain("ads.example.ir".into())];
    let cfg = build(&req);

    let dns = cfg["dns"]["rules"].as_array().unwrap();
    assert_eq!(dns[0]["action"], "predefined", "refused before it could be resolved directly");

    let rules = cfg["route"]["rules"].as_array().unwrap();
    let reject = rules.iter().position(|r| r["action"] == "reject").unwrap();
    let bypass = rules.iter().position(|r| r.get("domain_suffix").is_some()).unwrap();
    assert!(reject < bypass);
}

#[test]
fn a_switch_with_no_list_on_disk_adds_nothing() {
    let mut req = sample();
    req.block = BlockOptions { ads: true, trackers: true };
    let cfg = build(&req);
    assert!(cfg["route"].get("rule_set").is_none());
    assert!(!cfg["route"]["rules"].as_array().unwrap().iter().any(|r| r["action"] == "reject"));
    assert!(cfg["dns"]["rules"].as_array().unwrap().is_empty());
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
        block: BlockOptions::default(),
        block_lists: vec![],
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
        block: BlockOptions::default(),
        block_lists: vec![],
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
        block: BlockOptions::default(),
        block_lists: vec![],
    });

    let proxy = &cfg["outbounds"][0];
    assert_eq!(proxy["type"], "trojan");
    assert_eq!(proxy["password"], "hunter2");
    assert!(proxy.get("uuid").is_none());
}
