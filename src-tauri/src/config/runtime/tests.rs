use super::*;
use crate::config::{Reality, TlsOptions, Transport};

fn profile() -> Profile {
    Profile {
        server: "example.net".into(),
        port: 443,
        uuid: "8f3c9d2e-4a17-4b8e-9c21-7de5f0a63b14".into(),
        transport: Transport {
            kind: TransportKind::Xhttp,
            path: "/x?token=a&b".into(),
            host: "cdn.example.net".into(),
            mode: "packet-up".into(),
            extra: serde_json::from_value(json!({ "headers": { "X-Test": "yes" },
                "xmux": { "maxConcurrency": "8-16" } }))
            .unwrap(),
            ..Default::default()
        },
        tls: TlsOptions {
            enabled: true,
            sni: "cdn.example.net".into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn xhttp_keeps_settings_behind_an_authenticated_loopback_bridge_in_every_config() {
    let p = profile();
    let (runtime, tags) = build_test(&[p.clone(), Profile::default()]).unwrap();
    assert_eq!(tags, ["t0", "t1"]);
    let xray = runtime.xray.as_ref().unwrap();
    assert_eq!(
        xray["outbounds"][0]["streamSettings"]["xhttpSettings"]["extra"],
        json!(p.transport.extra)
    );
    assert_eq!(
        xray["outbounds"][0]["streamSettings"]["xhttpSettings"]["path"],
        p.transport.path
    );
    assert_eq!(xray["inbounds"][0]["listen"], "127.0.0.1");
    assert_eq!(
        runtime.core["outbounds"][0]["server_port"],
        xray["inbounds"][0]["port"]
    );
    assert_eq!(
        runtime.core["outbounds"][0]["password"],
        xray["inbounds"][0]["settings"]["accounts"][0]["pass"]
    );
    assert_eq!(
        runtime.core["outbounds"][0]["password"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
    assert_eq!(runtime.core["outbounds"][1]["type"], "vless");
    assert_eq!(runtime.load_request().need_xray, Some(true));
    assert_eq!(
        runtime.test_request().xray_config,
        runtime.load_request().xray_config
    );
    assert_eq!(
        build_probe(&[p], &[12345]).unwrap().core["outbounds"][0]["type"],
        "socks"
    );
    assert!(build_test(&[Profile::default()]).unwrap().0.xray.is_none());
}

#[test]
fn xhttp_rejects_unsupported_options_and_carries_reality_and_protocol_credentials() {
    let mut p = profile();
    p.transport
        .extra
        .insert("downloadSettings".into(), json!({}));
    assert!(build_test(&[p.clone()])
        .err()
        .unwrap()
        .contains("downloadSettings"));
    p.transport.extra.clear();
    p.transport.extra.insert("xmux".into(), json!({"typo": 1}));
    assert!(build_test(&[p.clone()]).err().unwrap().contains("typo"));
    p.transport.extra.clear();
    p.flow = "xtls-rprx-vision".into();
    assert!(build_test(&[p.clone()]).err().unwrap().contains("flow"));
    p.flow.clear();
    p.tls.reality = Some(Reality {
        public_key: "key".into(),
        short_id: "ab".into(),
    });
    let outbound = xray_outbound(&p, "test").unwrap();
    assert_eq!(
        outbound["streamSettings"]["realitySettings"]["publicKey"],
        "key"
    );
    p.protocol = Protocol::Trojan;
    p.password = "secret".into();
    assert_eq!(
        xray_outbound(&p, "test").unwrap()["settings"]["servers"][0]["password"],
        "secret"
    );
    p.protocol = Protocol::Vmess;
    p.security = "chacha20-poly1305".into();
    assert_eq!(
        xray_outbound(&p, "test").unwrap()["settings"]["vnext"][0]["users"][0]["security"],
        p.security
    );
}
