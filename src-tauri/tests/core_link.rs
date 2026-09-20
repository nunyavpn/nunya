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

use nunya_lib::config::{self, BuildRequest, Reality, TlsOptions, TunOptions, VlessProfile};
use nunya_lib::core_proc::CoreProcess;
use nunya_lib::rpc::{gen, method, CoreLink};

fn core_path() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var("NUNYA_CORE_PATH").ok()?);
    p.exists().then_some(p)
}

fn sample_request() -> BuildRequest {
    BuildRequest {
        profile: VlessProfile {
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
        },
        tun: TunOptions::default(),
        bypass: vec![],
        dns: "https://1.1.1.1/dns-query".into(),
        log_level: "warn".into(),
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
