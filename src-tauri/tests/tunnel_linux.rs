//! Tunnel assertions that need a network namespace to be safe.
//!
//! These bring a real TUN up, let it take the default route, and check it goes away again. On a
//! developer's own machine that means `sudo` and a rewrite of the routing table they are currently
//! using; in a container it means `CAP_NET_ADMIN`, a route table nobody else is using, and nothing
//! left behind when the container exits. So they are Linux-only by construction rather than by
//! preference — see `scripts/dev-linux.sh`.
//!
//! What they deliberately do not need is a reachable server. sing-box builds outbounds lazily, so
//! the interface, the addresses and the routes all appear with an outbound that would fail if
//! anything tried to use it. That keeps these tests about routing, and keeps them offline.
//!
//! The harness below is a copy of the one in `core_link.rs`. Rust integration tests are separate
//! crates and cannot see each other's helpers without a shared `common` module; duplicating thirty
//! lines is cheaper here than reworking a test file that already passes.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use nunya_lib::config::{self, BuildRequest, Profile, TlsOptions, TunOptions, Mode, ProxyOptions};
use nunya_lib::core_proc::CoreProcess;
use nunya_lib::rpc::CoreLink;
use nunya_lib::transport::subprocess::SubprocessTransport;
use nunya_lib::transport::TunnelTransport;

/// The name `config.rs` gives the interface on everything that is not macOS.
const IFACE: &str = "nunya-tun";

fn core_path() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var("NUNYA_CORE_PATH").ok()?);
    p.exists().then_some(p)
}

/// `CAP_NET_ADMIN` without `/dev/net/tun` still cannot open a tunnel, so both are checked.
fn can_open_a_tun() -> bool {
    PathBuf::from("/dev/net/tun").exists()
}

mod tempdir {
    use std::path::{Path, PathBuf};

    pub struct Guard(PathBuf);

    impl Guard {
        pub fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "nunya-tun-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&p).expect("scratch dir");
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

async fn connect_core() -> (Arc<CoreLink>, CoreProcess, tempdir::Guard) {
    let core = core_path().expect("NUNYA_CORE_PATH must point at a built core");
    let dir = tempdir::Guard::new();
    let socket = dir.path().join("core.sock");

    let (link, listener) = CoreLink::bind(&socket).expect("bind core socket");
    let proc = CoreProcess::spawn(&core, &socket, dir.path(), |line| eprintln!("[core] {line}"))
        .expect("spawn core");
    link.expect_core_pid(proc.pid);

    let accept_link = link.clone();
    tokio::spawn(async move {
        accept_link.accept_loop(listener, |_| {}).await;
    });

    for _ in 0..100 {
        if link.is_connected().await {
            return (link, proc, dir);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("core never connected to the socket");
}

// ---------------------------------------------------------------- reading the network

fn ip(args: &[&str]) -> String {
    let out = Command::new("ip").args(args).output().expect("run ip");
    // stderr too: `ip route get` reports "Network is unreachable" there, and silently treating that
    // as an empty answer would make a broken route look like a passing assertion.
    let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    text
}

/// The device the kernel would actually send a packet to this address on.
///
/// `ip route get` resolves through the whole policy-routing stack, which is the only check that
/// works on both platforms: sing-box takes traffic on Linux with `ip rule` plus a dedicated table,
/// and on macOS by splitting the default into two /1 routes. Reading the main table directly finds
/// the second and misses the first entirely.
fn route_device_for(addr: &str) -> Option<String> {
    let out = ip(&["route", "get", addr]);
    out.split(" dev ")
        .nth(1)?
        .split_whitespace()
        .next()
        .map(str::to_string)
}

fn interface_exists(name: &str) -> bool {
    Command::new("ip")
        .args(["link", "show", name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// A public address stands in for "ordinary internet traffic". Never contacted, only routed.
const OFF_LAN: &str = "1.1.1.1";

fn tunnel_holds_the_default_route() -> bool {
    route_device_for(OFF_LAN).as_deref() == Some(IFACE)
}

async fn wait_for(mut predicate: impl FnMut() -> bool, what: &str) {
    for _ in 0..100 {
        if predicate() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!(
        "timed out waiting for {what}\n\nrules:\n{}\nmain table:\n{}\nall tables:\n{}\nroute to {OFF_LAN}:\n{}",
        ip(&["rule", "show"]),
        ip(&["route", "show"]),
        ip(&["route", "show", "table", "all"]),
        ip(&["route", "get", OFF_LAN]),
    );
}

// ---------------------------------------------------------------- the request under test

/// A profile that will never connect, which is the point: this is about the interface, not traffic.
fn unreachable_profile() -> Profile {
    Profile {
        name: "offline".into(),
        server: "unreachable.example.net".into(),
        port: 443,
        uuid: "8f3c9d2e-4a17-4b8e-9c21-7de5f0a63b14".into(),
        tls: TlsOptions {
            enabled: true,
            sni: "unreachable.example.net".into(),
            ..Default::default()
        },
        ..Default::default()
    }
}

fn tunnel_request() -> BuildRequest {
    BuildRequest {
        mode: Mode::Vpn,
        proxy: ProxyOptions::default(),
        profile: unreachable_profile(),
        tun: TunOptions::default(),
        bypass: vec![],
        dns: "https://1.1.1.1/dns-query".into(),
        log_level: "warn".into(),
        block: Default::default(),
        block_lists: vec![],
    }
}

/// Drives the real `SubprocessTransport` rather than hand-building an RPC request.
///
/// An earlier version of this file assembled its own `LoadConfigReq`, and that was a mistake worth
/// recording: the request the client actually sends had to be fixed to stop the core panicking on
/// unset optional fields, and a test with its own copy of that request sailed straight past the
/// fix and kept reproducing the bug. A test of the tunnel should exercise the code that opens the
/// tunnel, not a replica of it.
fn transport(link: Arc<CoreLink>) -> SubprocessTransport {
    SubprocessTransport::new(link)
}

async fn start(t: &SubprocessTransport, req: &BuildRequest) {
    if let Err(e) = t.start(req).await {
        panic!(
            "the core refused to start: {e}\n\n{}",
            serde_json::to_string_pretty(&config::build(req)).unwrap()
        );
    }
}

async fn stop(t: &SubprocessTransport) {
    t.stop().await.expect("the core refused to stop");
}

// ---------------------------------------------------------------- tests

/// The claim the whole app is built around: the device is in the tunnel.
///
/// Not "the core started" — an interface that exists but holds no route carries nothing, and would
/// let the UI say "you're protected" over a tunnel that is not being used.
#[tokio::test]
#[ignore = "needs CAP_NET_ADMIN and /dev/net/tun; run via scripts/dev-linux.sh"]
async fn the_tunnel_takes_the_default_route_and_gives_it_back() {
    assert!(can_open_a_tun(), "/dev/net/tun is missing: run this with --device /dev/net/tun");

    let before = route_device_for(OFF_LAN);
    assert_ne!(
        before.as_deref(),
        Some(IFACE),
        "a tunnel is already up before the test started"
    );

    let (link, proc, _dir) = connect_core().await;
    let tunnel = transport(link);
    let req = tunnel_request();

    start(&tunnel, &req).await;

    wait_for(|| interface_exists(IFACE), "the tun interface to appear").await;
    wait_for(tunnel_holds_the_default_route, "the tunnel to take the default route").await;

    // The address the config asked for, so a changed ipv4_cidr is not silently ignored.
    let addrs = ip(&["-4", "addr", "show", IFACE]);
    let wanted = req.tun.ipv4_cidr.split('/').next().unwrap();
    assert!(
        addrs.contains(wanted),
        "{IFACE} does not carry {wanted}:\n{addrs}"
    );

    stop(&tunnel).await;

    // Giving the route back matters as much as taking it: a tunnel that leaves the table rewritten
    // after Stop leaves the machine without working networking.
    wait_for(|| !interface_exists(IFACE), "the tun interface to go away").await;
    assert_eq!(
        route_device_for(OFF_LAN),
        before,
        "traffic did not go back to its original device after Stop"
    );

    proc.stop().await;
}

/// Everything goes into the tunnel, including the ranges the bypass list names.
///
/// This is not the obvious design, and getting it wrong sends you looking for a bug that is not
/// there. A bypass rule is **not** a kernel route exclusion: `config.rs` installs `auto_route`,
/// which on Linux points one `ip rule` at a dedicated table and swallows the lot, and the decision
/// to send a bypassed address out on the physical link is taken inside sing-box, per connection,
/// by the `ip_is_private` and `ip_cidr` route rules. Hence the comment in `config.rs` — "the
/// tunnel carries everything, so without this the LAN becomes unreachable".
///
/// So this asserts the kernel half, which is all the kernel can see. That the core then routes
/// those packets to `direct` is covered by the config tests, and would need the core's own
/// connection reporting to observe from out here.
///
/// It is worth pinning: if anyone ever teaches the client to install kernel-level exclusions, this
/// test fails and makes them say so deliberately.
#[tokio::test]
#[ignore = "needs CAP_NET_ADMIN and /dev/net/tun; run via scripts/dev-linux.sh"]
async fn a_bypassed_range_still_enters_the_tunnel_and_is_sorted_out_inside_it() {
    assert!(can_open_a_tun(), "/dev/net/tun is missing");

    let (link, proc, _dir) = connect_core().await;
    let tunnel = transport(link);

    let mut req = tunnel_request();
    req.bypass = vec![config::BypassRule::Range("10.0.0.0/8".into())];

    start(&tunnel, &req).await;
    wait_for(|| interface_exists(IFACE), "the tun interface to appear").await;
    wait_for(tunnel_holds_the_default_route, "the tunnel to take the route").await;

    assert_eq!(
        route_device_for("10.0.0.1").as_deref(),
        Some(IFACE),
        "a private address no longer enters the tunnel, so the bypass is being done by the kernel \
         rather than by the core — which is a design change, not a passing test\n\nrules:\n{}",
        ip(&["rule", "show"]),
    );

    stop(&tunnel).await;
    proc.stop().await;
}

/// Stopping a tunnel that was never started must not leave an interface behind.
#[tokio::test]
#[ignore = "needs CAP_NET_ADMIN and /dev/net/tun; run via scripts/dev-linux.sh"]
async fn stopping_without_starting_is_harmless() {
    let (link, proc, _dir) = connect_core().await;
    let tunnel = transport(link);
    stop(&tunnel).await;
    assert!(!interface_exists(IFACE));
    proc.stop().await;
}
