//! End-to-end test against the real core binary.
//!
//! Everything else in this crate is tested against a socketpair or a hand-built frame. This exercises
//! the thing that actually matters: that a core built from the nunya-core repository connects to our listener, passes
//! the peer check, and answers real RPCs with configs we generated.
//!
//! Needs a built core, so it is ignored by default:
//!
//! ```sh
//! ./scripts/fetch-core.sh --source ../nunya-core
//! NUNYA_CORE_PATH="$PWD/vendor/core/bin/nunya-core" \
//!   cargo test --manifest-path src-tauri/Cargo.toml -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use nunya_lib::config::{
    self, BuildRequest, Profile, Protocol, Reality, TlsOptions, Transport, TransportKind,
    TunOptions, WireguardOptions, Mode, ProxyOptions};
use nunya_lib::core_proc::CoreProcess;
use nunya_lib::rpc::{gen, method, CoreLink};

fn core_path() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var("NUNYA_CORE_PATH").ok()?);
    p.exists().then_some(p)
}

fn sample_request() -> BuildRequest {
    BuildRequest {
        mode: Mode::Vpn,
        proxy: ProxyOptions::default(),
        profile: Profile {
            name: "smoke".into(),
            server: "example.net".into(),
            port: 443,
            uuid: "8f3c9d2e-4a17-4b8e-9c21-7de5f0a63b14".into(),
            flow: "xtls-rprx-vision".into(),
            tls: TlsOptions {
                enabled: true,
                sni: "www.cloudflare.com".into(),
                fingerprint: "chrome".into(),
                reality: Some(Reality {
                    // A syntactically valid x25519 public key; no handshake happens here.
                    public_key: "jNXHt1yRo0vDuchQlIP6Z0ZvjT3KtzVI-T4E7RoLJS0".into(),
                    short_id: "0123abcd".into(),
                }),
                ..Default::default()
            },
            ..Default::default()
        },
        tun: TunOptions::default(),
        bypass: vec![],
        dns: "https://1.1.1.1/dns-query".into(),
        log_level: "warn".into(),
        block: Default::default(),
        block_lists: vec![],
    }
}

/// Brings up the link and the core, and waits for the core to dial in.
async fn connect_core() -> (Arc<CoreLink>, CoreProcess, tempdir::Guard) {
    let core = core_path().expect("NUNYA_CORE_PATH must point at a built core");

    let dir = tempdir::Guard::new();
    let socket = dir.path().join("core.sock");

    let (link, listener) = CoreLink::bind(&socket).expect("bind core socket");

    let proc = CoreProcess::spawn(&core, &socket, dir.path(), |line| {
        eprintln!("[core] {line}");
    })
    .expect("spawn core");

    link.expect_core_pid(proc.pid);

    let accept_link = link.clone();
    tokio::spawn(async move {
        accept_link.accept_loop(listener, |_| {}).await;
    });

    // The core prints its banner, checks its parent, then dials in with retries.
    for _ in 0..100 {
        if link.is_connected().await {
            return (link, proc, dir);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("core never connected to the socket");
}

#[tokio::test]
#[ignore = "needs a built core; set NUNYA_CORE_PATH"]
async fn core_connects_and_answers() {
    let (link, proc, _dir) = connect_core().await;

    // IsPrivileged is the cheapest round trip that proves framing, dispatch and decoding all work.
    let resp: gen::IsPrivilegedResponse = link
        .call(method::IS_PRIVILEGED, &gen::EmptyReq {})
        .await
        .expect("IsPrivileged");
    eprintln!("privileged: {:?}", resp.has_privilege);

    proc.stop().await;
}

/// Every transport and both protocols, put in front of the core's own validator.
///
/// The unit tests in `config.rs` assert the shape this client emits; only the core can say whether
/// that shape is one sing-box accepts. The two disagree in ways invisible from here — a `host` that
/// is a string for one transport and a list for another, a `transport` key that must be absent
/// rather than empty for TCP — and each of those produces a config that passes our own tests and
/// then fails at Start.
#[tokio::test]
#[ignore = "needs a built core; set NUNYA_CORE_PATH"]
async fn every_transport_is_accepted_by_the_core() {
    let (link, proc, _dir) = connect_core().await;

    let base = sample_request().profile;
    let cases: Vec<(&str, Profile)> = vec![
        ("vless tcp", base.clone()),
        (
            "vless ws",
            Profile {
                transport: Transport {
                    kind: TransportKind::Ws,
                    path: "/meeting".into(),
                    host: "cdn.example.net".into(),
                    max_early_data: 2048,
                    ..Default::default()
                },
                ..base.clone()
            },
        ),
        (
            "vless grpc",
            Profile {
                transport: Transport {
                    kind: TransportKind::Grpc,
                    service_name: "TunService".into(),
                    ..Default::default()
                },
                ..base.clone()
            },
        ),
        (
            "vless http",
            Profile {
                transport: Transport {
                    kind: TransportKind::Http,
                    host: "h.example.net".into(),
                    path: "/p".into(),
                    ..Default::default()
                },
                ..base.clone()
            },
        ),
        (
            "vless httpupgrade",
            Profile {
                transport: Transport {
                    kind: TransportKind::Httpupgrade,
                    host: "h.example.net".into(),
                    path: "/up".into(),
                    ..Default::default()
                },
                ..base.clone()
            },
        ),
        (
            "vmess ws",
            Profile {
                protocol: Protocol::Vmess,
                // Vision is VLESS-only, and a CDN link is plain TLS rather than Reality.
                flow: String::new(),
                security: "auto".into(),
                tls: TlsOptions {
                    enabled: true,
                    sni: "cdn.example.net".into(),
                    ..Default::default()
                },
                transport: Transport {
                    kind: TransportKind::Ws,
                    path: "/vm".into(),
                    host: "cdn.example.net".into(),
                    ..Default::default()
                },
                ..base.clone()
            },
        ),
        (
            "vmess tcp with a legacy alter_id",
            Profile {
                protocol: Protocol::Vmess,
                flow: String::new(),
                security: "aes-128-gcm".into(),
                alter_id: 4,
                tls: TlsOptions {
                    enabled: true,
                    sni: "cdn.example.net".into(),
                    ..Default::default()
                },
                ..base.clone()
            },
        ),
    ];

    let mut failures = Vec::new();

    for (label, profile) in cases {
        let mut req = sample_request();
        req.profile = profile;
        let cfg = config::build(&req);

        let resp: gen::ErrorResp = link
            .call(
                method::CHECK_CONFIG,
                &gen::LoadConfigReq {
                    core_config: Some(cfg.to_string()),
                    ..Default::default()
                },
            )
            .await
            .expect("CheckConfig round trip");

        let err = resp.error.unwrap_or_default();
        if err.is_empty() {
            eprintln!("ok   {label}");
        } else {
            failures.push(format!(
                "{label}: {err}\n{}",
                serde_json::to_string_pretty(&cfg["outbounds"][0]).unwrap()
            ));
        }
    }

    proc.stop().await;

    assert!(
        failures.is_empty(),
        "the core rejected:\n\n{}",
        failures.join("\n\n")
    );
}

#[tokio::test]
#[ignore = "needs a built core; set NUNYA_CORE_PATH"]
async fn generated_config_is_accepted_by_the_core() {
    let (link, proc, _dir) = connect_core().await;

    let cfg = config::build(&sample_request());
    let resp: gen::ErrorResp = link
        .call(
            method::CHECK_CONFIG,
            &gen::LoadConfigReq {
                core_config: Some(cfg.to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("CheckConfig round trip");

    proc.stop().await;

    let err = resp.error.unwrap_or_default();
    assert!(
        err.is_empty(),
        "the core rejected our generated config:\n{err}\n\n{}",
        serde_json::to_string_pretty(&cfg).unwrap()
    );
}

/// Both protocols added after the first two, checked against the core rather than against our own
/// idea of the schema.
///
/// The shapes differ from the originals in ways a unit test here cannot catch. Trojan carries a
/// password where VLESS carries a UUID, and sending both is an unknown field the core refuses.
/// WireGuard is not an outbound at all: sing-box moved it to `endpoints`, and the old form fails
/// with *unknown field "local_address"* — at connect time, where it reads as a broken tunnel
/// rather than a bad config.
///
/// The latency-test config is checked too, because it assembles the same nodes into a different
/// document and an endpoint has to survive being tagged `t0` with no TUN inbound around it.
#[tokio::test]
#[ignore = "needs a built core; set NUNYA_CORE_PATH"]
async fn wireguard_and_trojan_are_accepted_by_the_core() {
    let (link, proc, _dir) = connect_core().await;

    let trojan = Profile {
        protocol: Protocol::Trojan,
        name: "Trojan".into(),
        server: "edge.example.net".into(),
        port: 443,
        password: "E1wy;,GYGPPhYEYXn".into(),
        tls: TlsOptions {
            enabled: true,
            sni: "edge.example.net".into(),
            alpn: vec!["http/1.1".into()],
            fingerprint: "chrome".into(),
            ..Default::default()
        },
        transport: Transport {
            kind: TransportKind::Ws,
            path: "/tr/x".into(),
            host: "edge.example.net".into(),
            ..Default::default()
        },
        ..Default::default()
    };

    let wireguard = Profile {
        protocol: Protocol::Wireguard,
        name: "Warp".into(),
        server: "engage.cloudflareclient.com".into(),
        port: 2408,
        wireguard: Some(WireguardOptions {
            private_key: "f7m/C8NHWPWIkGAbxTBAMhYHlzu3Ya7lSCbOSqfGu68=".into(),
            peer_public_key: "bmXOC+F1FxEMF9dyiK2H5/1SUtzH0JuVo51h2wPfgyo=".into(),
            local_address: vec![
                "172.16.0.2/32".into(),
                "2606:4700:110:8e2a:e673:f46d:6bee:e6e0/128".into(),
            ],
            // WARP's client id. Wrong values are dropped by the server without an error.
            reserved: vec![216, 253, 3],
            mtu: 1280,
            keepalive: 5,
        }),
        ..Default::default()
    };

    let mut failures = Vec::new();

    for profile in [trojan, wireguard] {
        let label = format!("{:?}", profile.protocol);
        let mut req = sample_request();
        req.profile = profile.clone();

        for (kind, cfg) in [
            ("tunnel", config::build(&req)),
            (
                "latency test",
                config::build_test(std::slice::from_ref(&profile)).0,
            ),
        ] {
            let resp: gen::ErrorResp = link
                .call(
                    method::CHECK_CONFIG,
                    &gen::LoadConfigReq {
                        core_config: Some(cfg.to_string()),
                        ..Default::default()
                    },
                )
                .await
                .expect("CheckConfig round trip");

            if let Some(err) = resp.error.filter(|e| !e.is_empty()) {
                failures.push(format!(
                    "{label} / {kind} rejected: {err}\n{}",
                    serde_json::to_string_pretty(&cfg).unwrap()
                ));
            }
        }
    }

    proc.stop().await;
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Proxy mode against the real core.
///
/// The two modes produce different documents — a `mixed` inbound instead of a `tun` one, and one
/// fewer route rule — and the core is the only thing that can say whether the shape is right.
/// This matters more than the VPN case today, because proxy mode is the default: it is the one
/// that needs no privilege, and so the one a new user actually reaches.
#[tokio::test]
#[ignore = "needs a built core; set NUNYA_CORE_PATH"]
async fn proxy_mode_is_accepted_by_the_core() {
    let (link, proc, _dir) = connect_core().await;

    let mut failures = Vec::new();
    for (label, proxy) in [
        ("loopback", ProxyOptions::default()),
        (
            "lan",
            ProxyOptions {
                port: 1080,
                allow_lan: true,
            },
        ),
    ] {
        let mut req = sample_request();
        req.mode = Mode::Proxy;
        req.proxy = proxy;
        let cfg = config::build(&req);

        let resp: gen::ErrorResp = link
            .call(
                method::CHECK_CONFIG,
                &gen::LoadConfigReq {
                    core_config: Some(cfg.to_string()),
                    ..Default::default()
                },
            )
            .await
            .expect("CheckConfig round trip");

        if let Some(err) = resp.error.filter(|e| !e.is_empty()) {
            failures.push(format!(
                "{label} rejected: {err}\n{}",
                serde_json::to_string_pretty(&cfg).unwrap()
            ));
        }
    }

    proc.stop().await;
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// The location probe's config — one `mixed` port per server, and a catch-all that refuses rather
/// than going direct — is a shape no other config here has, so the core has to see it too.
#[tokio::test]
#[ignore = "needs a built core; set NUNYA_CORE_PATH"]
async fn the_location_probe_is_accepted_by_the_core() {
    let (link, proc, _dir) = connect_core().await;

    let profiles = vec![sample_request().profile, sample_request().profile];
    let cfg = config::build_probe(&profiles, &[20801, 20802]);
    let resp: gen::ErrorResp = link
        .call(
            method::CHECK_CONFIG,
            &gen::LoadConfigReq {
                core_config: Some(cfg.to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("CheckConfig round trip");

    proc.stop().await;
    if let Some(err) = resp.error.filter(|e| !e.is_empty()) {
        panic!("rejected: {err}\n{}", serde_json::to_string_pretty(&cfg).unwrap());
    }
}

#[tokio::test]
#[ignore = "needs a built core; set NUNYA_CORE_PATH"]
async fn bypass_rules_survive_validation() {
    let (link, proc, _dir) = connect_core().await;

    let mut req = sample_request();
    req.bypass = vec![
        config::BypassRule::Domain("*.bank.ir".into()),
        config::BypassRule::Address("8.8.8.8".into()),
        config::BypassRule::Range("79.127.0.0/16".into()),
    ];

    let cfg = config::build(&req);
    let resp: gen::ErrorResp = link
        .call(
            method::CHECK_CONFIG,
            &gen::LoadConfigReq {
                core_config: Some(cfg.to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("CheckConfig round trip");

    proc.stop().await;

    let err = resp.error.unwrap_or_default();
    assert!(
        err.is_empty(),
        "bypass rules were rejected:\n{err}\n\n{}",
        serde_json::to_string_pretty(&cfg).unwrap()
    );
}

#[tokio::test]
#[ignore = "needs a built core; set NUNYA_CORE_PATH"]
async fn a_broken_config_is_reported_not_swallowed() {
    let (link, proc, _dir) = connect_core().await;

    let resp: gen::ErrorResp = link
        .call(
            method::CHECK_CONFIG,
            &gen::LoadConfigReq {
                core_config: Some(r#"{"outbounds":[{"type":"nonsense"}]}"#.to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("CheckConfig round trip");

    proc.stop().await;

    assert!(
        !resp.error.unwrap_or_default().is_empty(),
        "an invalid config must come back as an error, not silently pass"
    );
}

/// A latency-test config has to be one the core will accept, and the test RPC has to answer for
/// every tag it was given.
///
/// The servers are unreachable, so every result is a failure — which is the point: a dead server
/// must come back as an error rather than as 0ms, which would sort as the fastest one there is.
#[tokio::test]
#[ignore = "needs a built core; set NUNYA_CORE_PATH"]
async fn latency_testing_answers_for_every_server() {
    let (link, proc, _dir) = connect_core().await;

    let mut second = sample_request().profile;
    second.server = "second.invalid".into();
    let profiles = vec![sample_request().profile, second];

    let (cfg, tags) = config::build_test(&profiles);

    // Accepted before it is run, so a malformed test config fails loudly here.
    let checked: gen::ErrorResp = link
        .call(
            method::CHECK_CONFIG,
            &gen::LoadConfigReq {
                core_config: Some(cfg.to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("CheckConfig round trip");
    let err = checked.error.clone().unwrap_or_default();
    assert!(
        err.is_empty(),
        "the core rejected the test config:\n{err}\n\n{}",
        serde_json::to_string_pretty(&cfg).unwrap()
    );

    let resp: gen::TestResp = link
        .call(
            method::TEST,
            &gen::TestReq {
                config: Some(cfg.to_string()),
                outbound_tags: tags.clone(),
                url: Some("http://cp.cloudflare.com/".into()),
                test_timeout_ms: Some(2000),
                max_concurrency: Some(4),
                ..Default::default()
            },
        )
        .await
        .expect("Test round trip");

    proc.stop().await;

    assert_eq!(
        resp.results.len(),
        tags.len(),
        "every submitted tag must come back with a verdict"
    );
    for result in &resp.results {
        let tag = result.outbound_tag.clone().unwrap_or_default();
        assert!(tags.contains(&tag), "unexpected tag {tag}");
        assert!(
            !result.error.clone().unwrap_or_default().is_empty(),
            "{tag} resolves nowhere, so it should have reported an error"
        );
    }
}

/// Minimal scratch directory that cleans up after itself, so the tests do not pull in a dependency
/// for something this small.
/// Asks the core to check a config, and returns its complaint if it has one.
async fn core_check(link: &CoreLink, cfg: &serde_json::Value) -> Option<String> {
    let resp: gen::ErrorResp = link
        .call(
            method::CHECK_CONFIG,
            &gen::LoadConfigReq {
                core_config: Some(cfg.to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("CheckConfig round trip");
    resp.error.filter(|e| !e.is_empty())
}

/// The tunnel config with both block lists on disk, in VPN and proxy mode.
///
/// Both lists are source files here, which needs no download; the binary ad list's shape differs
/// only in `format`, and `the_published_block_lists_download_and_the_core_reads_them` covers it.
#[tokio::test]
#[ignore = "needs a built core; set NUNYA_CORE_PATH"]
async fn a_config_with_block_lists_is_accepted_by_the_core() {
    let (link, proc, dir) = connect_core().await;

    let mut lists = Vec::new();
    for tag in ["block-ads", "block-trackers"] {
        let path = dir.path().join(format!("{tag}.json"));
        std::fs::write(
            &path,
            r#"{"version":2,"rules":[{"domain_suffix":["ads.example.net","metrics.example.net"]}]}"#,
        )
        .unwrap();
        lists.push(config::BlockList {
            tag,
            format: "source",
            path: path.to_string_lossy().into_owned(),
        });
    }

    for mode in [Mode::Vpn, Mode::Proxy] {
        let mut req = sample_request();
        req.mode = mode;
        req.block = config::BlockOptions { ads: true, trackers: true };
        req.block_lists = lists.clone();
        let complaint = core_check(&link, &config::build(&req)).await;
        assert_eq!(complaint, None, "{mode:?}");
    }

    proc.stop().await;
}

/// The lists as published today: downloaded, and read by the core the way `update_blocklist` has
/// it read them. Also that an error page in their place is refused rather than installed.
#[tokio::test]
#[ignore = "needs a built core and the network; set NUNYA_CORE_PATH"]
async fn the_published_block_lists_download_and_the_core_reads_them() {
    use nunya_lib::blocklists::{self, List};

    let (link, proc, dir) = connect_core().await;

    for list in List::ALL {
        let bytes = tokio::task::spawn_blocking(move || blocklists::download(list, None))
            .await
            .unwrap()
            .unwrap_or_else(|e| panic!("{} list: {e}", list.name()));
        eprintln!("{} list: {} bytes", list.name(), bytes.len());
        let staged = blocklists::stage(dir.path(), list, &bytes).unwrap();
        let complaint = core_check(&link, &blocklists::check_config(list, &staged)).await;
        assert_eq!(complaint, None, "{} list", list.name());

        let page = blocklists::stage(dir.path(), list, b"<html>429 Too Many Requests</html>").unwrap();
        let complaint = core_check(&link, &blocklists::check_config(list, &page)).await;
        assert!(complaint.is_some(), "an error page passed as the {} list", list.name());
    }

    proc.stop().await;
}

mod tempdir {
    use std::path::{Path, PathBuf};

    pub struct Guard(PathBuf);

    impl Guard {
        pub fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "nunya-test-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&p).expect("create scratch dir");
            Self(p)
        }

        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
}

/// A real local XHTTP server, not a mock of the config schema. The Test RPC, probe sessions and
/// the normal subprocess transport must all deliver HTTP through it, with no external service.
#[tokio::test]
#[ignore = "needs a development core; set NUNYA_CORE_PATH"]
async fn xhttp_carries_traffic_through_connect_latency_and_probe_paths() {
    use nunya_lib::transport::{subprocess::SubprocessTransport, TunnelTransport};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use serde_json::json;

    let (server_link, server_proc, _server_dir) = connect_core().await;
    let (client_link, client_proc, _client_dir) = connect_core().await;
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let server_port = listener.local_addr().unwrap().port();
    drop(listener);
    let uuid = "8f3c9d2e-4a17-4b8e-9c21-7de5f0a63b14";
    let server_xray = json!({
        "log": { "loglevel": "warning" },
        "inbounds": [{ "listen": "127.0.0.1", "port": server_port, "protocol": "vless",
            "settings": { "clients": [{ "id": uuid }], "decryption": "none" },
            "streamSettings": { "network": "xhttp", "xhttpSettings": { "path": "/nunya-test" } } }],
        "outbounds": [{ "protocol": "freedom", "settings": { "finalRules": [{ "ip": ["127.0.0.1/32"], "action": "allow" }] } }]
    });
    let resp: gen::ErrorResp = server_link.call(method::START, &gen::LoadConfigReq {
        core_config: Some(json!({"outbounds": [{"type":"direct"}]}).to_string()),
        need_extra_process: Some(false), need_xray: Some(true), xray_config: Some(server_xray.to_string()),
        ..Default::default()
    }).await.unwrap();
    assert_eq!(resp.error.unwrap_or_default(), "");

    let http = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/xhttp", http.local_addr().unwrap());
    let http_task = tokio::spawn(async move {
        while let Ok((mut stream, _)) = http.accept().await {
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                let _ = stream.read(&mut buffer).await;
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nxhttp-ok").await;
            });
        }
    });
    // The supplied provider format, using documentation addresses and fixture credentials.
    let sample = Profile {
        server: "192.0.2.10".into(), port: 443, uuid: uuid.into(),
        tls: TlsOptions { enabled: true, sni: "cdn.example.net".into(),
            fingerprint: "firefox".into(), alpn: vec!["h2".into(), "h3".into()], ..Default::default() },
        transport: Transport { kind: TransportKind::Xhttp, path: "/".into(), host: "cdn.example.net".into(),
            mode: "auto".into(), extra: serde_json::from_value(json!({"mode":"auto", "xPaddingBytes":"100-1000"})).unwrap(),
            ..Default::default() }, ..Default::default()
    };
    config::runtime::build_test(&[sample]).unwrap().0.check(&client_link).await.unwrap();
    let mut p = Profile { server: "127.0.0.1".into(), port: server_port, uuid: uuid.into(),
        transport: Transport { kind: TransportKind::Xhttp, path: "/nunya-test".into(), ..Default::default() },
        ..Default::default() };
    for mode in ["auto", "packet-up", "stream-up", "stream-one"] {
        p.transport.mode = mode.into();
        let (cfg, tags) = config::runtime::build_test(&[p.clone()]).unwrap();
        cfg.check(&client_link).await.expect("both configs validate");
        let result: gen::TestResp = client_link.call(method::TEST, &gen::TestReq {
            outbound_tags: tags, url: Some(url.clone()), test_timeout_ms: Some(5000),
            max_concurrency: Some(1), ..cfg.test_request()
        }).await.unwrap();
        assert_eq!(result.results.len(), 1, "{mode}");
        assert_eq!(result.results[0].error.as_deref().unwrap_or_default(), "", "{mode}");
    }
    p.transport.mode = "packet-up".into();
    let probe = nunya_lib::geo::ProbeSession::start(&core_path().unwrap(), &[p.clone()]).await.unwrap();
    async fn fetch(port: u16, url: String) {
        let body = tokio::task::spawn_blocking(move || {
            let proxy = ureq::Proxy::new(&format!("http://127.0.0.1:{port}")).unwrap();
            let agent: ureq::Agent = ureq::Agent::config_builder().proxy(Some(proxy))
                .timeout_global(Some(Duration::from_secs(5))).build().into();
            agent.get(&url).call().unwrap().body_mut().read_to_string().unwrap()
        }).await.unwrap();
        assert_eq!(body, "xhttp-ok");
    }
    fetch(probe.ports[0], url.clone()).await;
    probe.shut_down().await;

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut req = sample_request();
    req.profile = p;
    req.mode = Mode::Proxy;
    req.proxy.port = proxy_port;
    let transport = SubprocessTransport::new(client_link.clone());
    transport.start(&req).await.unwrap();
    fetch(proxy_port, url.clone()).await;
    assert!(matches!(transport.state().await.unwrap(), nunya_lib::transport::TunnelState::Connected));
    transport.stop().await.unwrap();
    // The production LAN bypass correctly sends the loopback HTTP target direct. Remove only
    // that rule for this fixture so the normal runtime's counters also see actual XHTTP traffic.
    let mut cfg = config::runtime::build(&req).unwrap();
    cfg.core["route"]["rules"] = json!([]);
    let resp: gen::ErrorResp = client_link.call(method::START, &gen::LoadConfigReq {
        disable_stats: Some(false), ..cfg.load_request()
    }).await.unwrap();
    assert_eq!(resp.error.unwrap_or_default(), "");
    fetch(proxy_port, url.clone()).await;
    assert!(transport.throughput().await.unwrap().downlink > 0);
    transport.stop().await.unwrap();
    server_proc.stop().await;
    client_proc.stop().await;
    http_task.abort();
}

/// Every shareable shape, exported as Xray JSON and as sing-box JSON, checked by the engines that
/// would run it. The core embeds Xray, so a config another client's Xray would refuse fails here.
#[tokio::test]
#[ignore = "needs a built core; set NUNYA_CORE_PATH"]
async fn exported_configs_are_accepted_by_both_engines() {
    use config::export::{export, Format};
    let (link, proc, _dir) = connect_core().await;

    let base = sample_request().profile;
    let tls = TlsOptions {
        enabled: true,
        sni: "cdn.example.net".into(),
        alpn: vec!["http/1.1".into()],
        fingerprint: "chrome".into(),
        ..Default::default()
    };
    let with = |kind, path: &str| Profile {
        flow: String::new(),
        tls: tls.clone(),
        transport: Transport {
            kind,
            path: path.into(),
            host: "cdn.example.net".into(),
            service_name: "TunService".into(),
            max_early_data: 2048,
            ..Default::default()
        },
        ..base.clone()
    };
    let cases = vec![
        ("vless tcp reality", base.clone()),
        ("vless ws", with(TransportKind::Ws, "/ws")),
        ("vless grpc", with(TransportKind::Grpc, "")),
        ("vless httpupgrade", with(TransportKind::Httpupgrade, "/up")),
        ("vmess ws", Profile { protocol: Protocol::Vmess, security: "auto".into(), ..with(TransportKind::Ws, "/vm") }),
        ("trojan tcp", Profile { protocol: Protocol::Trojan, password: "secret".into(), ..with(TransportKind::Tcp, "") }),
        (
            "wireguard",
            Profile {
                protocol: Protocol::Wireguard,
                server: "162.159.192.1".into(),
                port: 2408,
                wireguard: Some(WireguardOptions {
                    private_key: "yAnz5TF+lXXJte14tji3zlMNq+hd2rYUIgJBgB3fBmk=".into(),
                    peer_public_key: "bmXOC+F1FxEMF9dyiK2H5/1SUtzH0JuVo51h2wPfgyo=".into(),
                    local_address: vec!["172.16.0.2/32".into()],
                    reserved: vec![1, 2, 3],
                    ..Default::default()
                }),
                ..Profile::default()
            },
        ),
    ];

    let runtime_core = config::build(&sample_request()).to_string();
    let mut failures = Vec::new();
    for (label, profile) in cases {
        let xray = export(&profile, Format::Xray, "https://1.1.1.1/dns-query").unwrap();
        let resp: gen::ErrorResp = link
            .call(
                method::CHECK_CONFIG,
                &gen::LoadConfigReq {
                    core_config: Some(runtime_core.clone()),
                    need_xray: Some(true),
                    xray_config: Some(xray.clone()),
                    ..Default::default()
                },
            )
            .await
            .expect("CheckConfig round trip");
        if let Some(err) = resp.error.filter(|e| !e.is_empty()) {
            failures.push(format!("{label} (xray): {err}\n{xray}"));
        }
        let sing_box: serde_json::Value =
            serde_json::from_str(&export(&profile, Format::SingBox, "https://1.1.1.1/dns-query").unwrap()).unwrap();
        if let Some(err) = core_check(&link, &sing_box).await {
            failures.push(format!("{label} (sing-box): {err}"));
        }
    }

    proc.stop().await;
    assert!(failures.is_empty(), "the core rejected:\n\n{}", failures.join("\n\n"));
}
