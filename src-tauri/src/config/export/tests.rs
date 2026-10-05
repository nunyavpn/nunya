use super::*;
use crate::config::{TlsOptions, Transport, WireguardOptions, Protocol};

fn ws_profile() -> Profile {
    Profile {
        name: "Frankfurt".into(),
        server: "example.net".into(),
        port: 443,
        uuid: "8f3c9d2e-4a17-4b8e-9c21-7de5f0a63b14".into(),
        tls: TlsOptions { enabled: true, sni: "example.net".into(), ..Default::default() },
        transport: Transport {
            kind: TransportKind::Ws,
            path: "/vl".into(),
            host: "cdn.example.net".into(),
            max_early_data: 2560,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn parse(text: &str) -> Value {
    serde_json::from_str(text).unwrap()
}

#[test]
fn the_xray_export_carries_the_server_behind_the_ports_v2rayng_expects() {
    let c = parse(&export(&ws_profile(), Format::Xray, "https://1.1.1.1/dns-query").unwrap());
    assert_eq!(c["remarks"], "Frankfurt");
    assert_eq!(c["inbounds"][0]["port"], 10808);
    let ob = &c["outbounds"][0];
    assert_eq!(ob["protocol"], "vless");
    assert_eq!(ob["streamSettings"]["wsSettings"]["path"], "/vl?ed=2560");
    assert_eq!(ob["streamSettings"]["wsSettings"]["host"], "cdn.example.net");
    assert_eq!(ob["streamSettings"]["tlsSettings"]["serverName"], "example.net");
    assert_eq!(c["dns"]["servers"][0], "https://1.1.1.1/dns-query");
}

#[test]
fn a_resolver_xray_cannot_read_is_left_out_rather_than_written() {
    let c = parse(&export(&ws_profile(), Format::Xray, "tls://1.1.1.1").unwrap());
    assert!(c.get("dns").is_none());
}

#[test]
fn transports_xray_dropped_are_refused_by_name() {
    for kind in [TransportKind::Http, TransportKind::Quic] {
        let mut p = ws_profile();
        p.transport.kind = kind;
        assert!(export(&p, Format::Xray, "1.1.1.1").unwrap_err().contains("no longer runs"));
    }
}

#[test]
fn wireguard_becomes_an_xray_outbound_with_its_reserved_bytes() {
    let p = Profile {
        protocol: Protocol::Wireguard,
        server: "2001:db8::1".into(),
        port: 2408,
        wireguard: Some(WireguardOptions {
            private_key: "priv".into(),
            peer_public_key: "pub".into(),
            local_address: vec!["172.16.0.2/32".into()],
            reserved: vec![1, 2, 3],
            ..Default::default()
        }),
        ..Default::default()
    };
    let c = parse(&export(&p, Format::Xray, "1.1.1.1").unwrap());
    let s = &c["outbounds"][0]["settings"];
    assert_eq!(s["peers"][0]["endpoint"], "[2001:db8::1]:2408");
    assert_eq!(s["reserved"], json!([1, 2, 3]));
    assert!(c["outbounds"][0].get("streamSettings").is_none());
}

#[test]
fn the_sing_box_export_is_a_tun_config_without_what_is_ours_alone() {
    let c = parse(&export(&ws_profile(), Format::SingBox, "https://1.1.1.1/dns-query").unwrap());
    assert_eq!(c["inbounds"][0]["type"], "tun");
    assert!(c["inbounds"][0].get("interface_name").is_none());
    assert!(c.get("experimental").is_none());
    assert_eq!(c["outbounds"][0]["transport"]["max_early_data"], 2560);
}

#[test]
fn sing_box_refuses_xhttp_by_name() {
    let mut p = ws_profile();
    p.transport.kind = TransportKind::Xhttp;
    assert!(export(&p, Format::SingBox, "1.1.1.1").unwrap_err().contains("XHTTP"));
}
