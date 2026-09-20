//! Finding out where a server actually exits.
//!
//! The list shows a flag per server, and until now that flag was guessed from the share link's
//! name — which is whatever the provider typed, frequently wrong and sometimes deliberately so.
//! The only honest source is the address the destination sees.
//!
//! The core cannot supply it. `IPTest` and `SpeedTest` both do this internally, but against
//! endpoints compiled into the core (`api.ip2location.io`, `speedtest.net`), both fronted by
//! Cloudflare and therefore unreachable from a Cloudflare Workers proxy — which is what most
//! free subscriptions are. Neither request message carries a URL field, so there is nothing to
//! override from here. Against a real subscription they answered for two servers out of ten,
//! while `ipinfo.io` answered for all ten.
//!
//! So the client asks the question itself, which needs a proxy port per server to ask through.
//! That is what [`crate::config::build_probe`] produces, and this module runs it in **its own
//! short-lived core process**: a second instance with its own socket, its own configuration and
//! its own lifetime. The alternative — reconfiguring the running core — would mean tearing down
//! whatever the user is connected to in order to draw some flags, which is not a trade a list
//! refresh is allowed to make.

use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::config::{self, Profile};
use crate::core_proc::CoreProcess;
use crate::rpc::{gen, method, CoreLink};

/// Where the probe asks. Reachable from every server in the sample that failed the core's own
/// endpoints, and it answers over plain HTTP with a two-letter country code.
const LOOKUP_URL: &str = "http://ipinfo.io/json";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
/// How long to wait for the scratch core to dial back before giving up on it.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
/// How many lookups are in flight at once.
///
/// Not tuned down for politeness: measured against a real subscription, dropping it to 3 made the
/// sweep twice as slow *and* found fewer servers, so the misses are per-request flakiness through
/// the proxies rather than the endpoint objecting to the pace.
const MAX_CONCURRENCY: usize = 8;

/// How many times each server is asked before its guess is left alone.
///
/// Two, because a single pass reliably missed two or three of ten while the same servers answered
/// on a later run. A retry is far cheaper than it looks: the scratch core and its ports are
/// already up, so only the servers that failed are asked again.
const ATTEMPTS: usize = 2;

/// One server's actual exit, keyed back to the caller's ordering.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Located {
    pub index: usize,
    /// Two-letter country code, uppercase, as the list expects it.
    pub country: String,
    pub ip: String,
}

/// What `ipinfo.io/json` gives back. Everything else it returns is ignored.
#[derive(Deserialize)]
struct Lookup {
    #[serde(default)]
    ip: String,
    #[serde(default)]
    country: String,
}

/// Asks the OS for a free loopback port.
///
/// Binding and immediately dropping leaves a window in which something else could take the port,
/// which is why every port is taken before any is released: overlapping binds cannot collide with
/// each other, only with an unrelated process, and the core reports a bind failure rather than
/// silently mismatching.
fn free_ports(count: usize) -> std::io::Result<Vec<u16>> {
    let mut held = Vec::with_capacity(count);
    for _ in 0..count {
        held.push(TcpListener::bind("127.0.0.1:0")?);
    }
    held.iter().map(|l| Ok(l.local_addr()?.port())).collect()
}

/// Waits until every probe port is accepting, so the first lookup does not race the core.
async fn wait_for_listeners(ports: &[u16]) -> bool {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let all_up = ports.iter().all(|&port| {
            let addr = SocketAddr::from(([127, 0, 0, 1], port));
            std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
        });
        if all_up {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// One lookup, through one local port.
///
/// Blocking: `ureq` is, so this runs on a blocking worker. A failure is not an error worth
/// reporting upward — a server that cannot reach the endpoint simply keeps the country it had.
fn lookup_through(port: u16) -> Option<Lookup> {
    let proxy = ureq::Proxy::new(format!("http://127.0.0.1:{port}")).ok()?;
    let agent = ureq::AgentBuilder::new()
        .proxy(proxy)
        .timeout(REQUEST_TIMEOUT)
        .build();

    let body = agent.get(LOOKUP_URL).call().ok()?.into_string().ok()?;
    let parsed: Lookup = serde_json::from_str(&body).ok()?;
    (!parsed.country.is_empty()).then_some(parsed)
}

/// A scratch core, torn down however this function leaves.
struct Scratch {
    process: CoreProcess,
    dir: PathBuf,
}

impl Scratch {
    async fn shut_down(self) {
        // Stop first so the core closes its listeners, then kill it. Leaving it running would
        // leave open proxy ports on loopback for as long as the app lives.
        self.process.stop().await;
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Measures where each profile exits, in its own core process.
///
/// Profiles that cannot be reached are simply absent from the result: the caller keeps whatever
/// country it had for them, which is the guess from the server's name.
pub async fn locate(core_path: &Path, profiles: &[Profile]) -> Result<Vec<Located>, String> {
    if profiles.is_empty() {
        return Ok(Vec::new());
    }

    let ports = free_ports(profiles.len()).map_err(|e| format!("no free local ports: {e}"))?;
    let cfg = config::build_probe(profiles, &ports);

    // A directory of its own, so this cannot collide with the running core's socket.
    let dir = std::env::temp_dir().join(format!(
        "nunya-probe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    ));
    std::fs::create_dir_all(&dir).map_err(|e| format!("could not create a scratch dir: {e}"))?;
    let socket = dir.join("core.sock");

    let (link, listener) =
        CoreLink::bind(&socket).map_err(|e| format!("could not bind the probe socket: {e}"))?;
    let process = CoreProcess::spawn(core_path, &socket, &dir, |line| {
        log::debug!("probe core: {line}");
    })
    .map_err(|e| format!("could not start a core for the probe: {e}"))?;

    // Before the first accept, exactly as the main core's link does it: the peer check has to
    // have something to compare against.
    link.expect_core_pid(process.pid);
    let scratch = Scratch {
        process,
        dir: dir.clone(),
    };

    let accept_link = Arc::clone(&link);
    tokio::spawn(async move {
        accept_link.accept_loop(listener, |_| {}).await;
    });

    let deadline = tokio::time::Instant::now() + STARTUP_TIMEOUT;
    while !link.is_connected().await {
        if tokio::time::Instant::now() >= deadline {
            scratch.shut_down().await;
            return Err("the probe core never connected".into());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let started: Result<gen::ErrorResp, _> = link
        .call(
            method::START,
            &gen::LoadConfigReq {
                core_config: Some(cfg.to_string()),
                disable_stats: Some(true),
                tun_ipv4_cidr: Some(String::new()),
                // Unset optionals are dereferenced by the core's Start without a nil check, so
                // omitting these panics it rather than returning an error. See the same note in
                // `transport/subprocess.rs`.
                need_extra_process: Some(false),
                need_xray: Some(false),
                ..Default::default()
            },
        )
        .await;

    match started {
        Err(e) => {
            scratch.shut_down().await;
            return Err(format!("the probe core refused to start: {e}"));
        }
        Ok(resp) => {
            if let Some(error) = resp.error.filter(|e| !e.is_empty()) {
                scratch.shut_down().await;
                return Err(format!("the probe config was rejected: {error}"));
            }
        }
    }

    if !wait_for_listeners(&ports).await {
        scratch.shut_down().await;
        return Err("the probe core did not open its ports".into());
    }

    let mut located: Vec<Located> = Vec::new();
    // Indices still to ask about. Carried between attempts so a retry costs only the misses.
    let mut remaining: Vec<usize> = (0..ports.len()).collect();

    for attempt in 0..ATTEMPTS {
        if remaining.is_empty() {
            break;
        }
        if attempt > 0 {
            log::info!("asking {} server(s) again", remaining.len());
        }

        let mut still_missing = Vec::new();
        for chunk in remaining.chunks(MAX_CONCURRENCY) {
            // Each task carries its own index, so a failure can never shift a later result onto
            // the wrong server — which would be worse than no flag at all.
            let handles: Vec<_> = chunk
                .iter()
                .map(|&index| {
                    let port = ports[index];
                    (
                        index,
                        tokio::task::spawn_blocking(move || lookup_through(port)),
                    )
                })
                .collect();

            for (index, handle) in handles {
                match handle.await {
                    Ok(Some(found)) => located.push(Located {
                        index,
                        country: found.country.to_ascii_uppercase(),
                        ip: found.ip,
                    }),
                    Ok(None) => still_missing.push(index),
                    Err(e) => {
                        log::debug!("probe task for index {index} failed: {e}");
                        still_missing.push(index);
                    }
                }
            }
        }
        remaining = still_missing;
    }

    scratch.shut_down().await;
    log::info!(
        "located {} of {} servers",
        located.len(),
        profiles.len()
    );
    Ok(located)
}
