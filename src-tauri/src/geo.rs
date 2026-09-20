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

use serde::Serialize;

use crate::config::{self, Profile};
use crate::core_proc::CoreProcess;
use crate::rpc::{gen, method, CoreLink};

/// Endpoints that report the caller's address, asked **through** each server.
///
/// Deliberately not a geo endpoint. The obvious design — ask something like `ipinfo.io/json`
/// through every server — collapses in practice, because servers share exits: four of ten in a
/// real subscription came back on one Cloudflare address. A geo service rate-limits per client
/// IP, so a sweep puts ten requests on three or four addresses and most come back `429`.
/// Measured, that design located two servers out of ten while all eight reachable ones could
/// reach the endpoint perfectly well.
///
/// These return the address and nothing else, are not rate-limited at the volumes a sweep
/// produces, and are not Cloudflare-fronted — which matters because a Cloudflare Workers proxy,
/// the shape most free subscriptions take, cannot make a subrequest to Cloudflare.
const EXIT_IP_URLS: [&str; 2] = ["http://checkip.amazonaws.com/", "http://ifconfig.me/ip"];

/// Turns an address into a country, asked **directly** rather than through a server.
///
/// This is the half that is rate-limited, so it is the half that runs from the user's own
/// connection and only once per *distinct* exit: ten servers on four exits cost four lookups.
///
/// A chain rather than one endpoint, because the free tiers are exhaustible and a day of testing
/// is enough to exhaust one — at which point every flag silently stops updating. The first two
/// answer in plain text and JSON respectively; both were reachable when `ipinfo.io` had started
/// returning 429 to this machine.
///
/// Expect them to disagree occasionally. A Cloudflare anycast address has no single physical
/// location, and the databases behind these services place the same address in different
/// countries. The flag is "where this looks like it comes out", not a certificate.
const COUNTRY_URLS: [fn(&str) -> String; 3] = [
    |ip| format!("http://ip-api.com/line/{ip}?fields=countryCode"),
    |ip| format!("https://api.country.is/{ip}"),
    |ip| format!("http://ipinfo.io/{ip}/country"),
];

/// Per request, and deliberately short.
///
/// A sweep's wall time is set by its failures, not its successes: every reachable server answered
/// in well under a second, while an unreachable one costs this much twice over — once per
/// endpoint in the chain — on each attempt.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
/// How long to wait for the scratch core to dial back before giving up on it.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
/// How many lookups are in flight at once.
///
/// Not tuned down for politeness: measured against a real subscription, dropping it to 3 made the
/// sweep twice as slow and found fewer servers, so the misses are per-request flakiness through
/// the proxies rather than the endpoints objecting to the pace.
const MAX_CONCURRENCY: usize = 8;

/// How many times each server is asked before its guess is left alone.
const ATTEMPTS: usize = 2;

/// Gap between launching one lookup and the next, to spread servers that share an exit.
const STAGGER: Duration = Duration::from_millis(120);

/// Pause before asking the servers that failed again.
const BACKOFF: Duration = Duration::from_secs(2);

/// One server's actual exit, keyed back to the caller's ordering.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Located {
    pub index: usize,
    /// Two-letter country code, uppercase, as the list expects it.
    pub country: String,
    pub ip: String,
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

/// Asks one server what address it comes out of.
///
/// Blocking: `ureq` is, so this runs on a blocking worker. Tries each endpoint in turn, because
/// a server that cannot reach one may well reach the other, and a single unreachable endpoint
/// should not be reported as a server with no country.
fn exit_ip_through(port: u16) -> Result<String, String> {
    let proxy = ureq::Proxy::new(format!("http://127.0.0.1:{port}"))
        .map_err(|e| format!("bad proxy address: {e}"))?;
    let agent = ureq::AgentBuilder::new()
        .proxy(proxy)
        .timeout(REQUEST_TIMEOUT)
        .build();

    let mut last = String::from("no endpoint was tried");
    for url in EXIT_IP_URLS {
        match agent.get(url).call() {
            Ok(response) => match response.into_string() {
                Ok(body) => {
                    let candidate = body.trim();
                    // Validated rather than trusted: a captive portal or an error page would
                    // otherwise become an "address" and then a nonsense flag.
                    if candidate.parse::<std::net::IpAddr>().is_ok() {
                        return Ok(candidate.to_string());
                    }
                    last = format!("{url} did not return an address");
                }
                Err(e) => last = format!("could not read {url}: {e}"),
            },
            // A status is the endpoint answering; a transport error is the server failing. Both
            // are worth trying the next endpoint for, but they mean different things in a log.
            Err(ureq::Error::Status(status, _)) => last = format!("{url} returned {status}"),
            Err(e) => last = format!("{url}: {e}"),
        }
    }
    Err(last)
}

/// Pulls a two-letter country code out of whatever shape the endpoint answered in.
///
/// Plain text for one, JSON for another, so this looks for the code rather than parsing a schema
/// per endpoint — there are only so many ways to write two letters.
fn code_from(body: &str) -> Option<String> {
    let trimmed = body.trim();

    let candidate = if trimmed.starts_with('{') {
        let value: serde_json::Value = serde_json::from_str(trimmed).ok()?;
        value
            .get("country")
            .or_else(|| value.get("countryCode"))
            .and_then(|v| v.as_str())?
            .to_string()
    } else {
        trimmed.to_string()
    };

    let code = candidate.trim().to_ascii_uppercase();
    (code.len() == 2 && code.chars().all(|c| c.is_ascii_alphabetic())).then_some(code)
}

/// Turns an address into a two-letter country code, from this machine's own connection.
fn country_of(ip: &str) -> Result<String, String> {
    let agent = ureq::AgentBuilder::new().timeout(REQUEST_TIMEOUT).build();

    let mut last = String::from("no endpoint was tried");
    for url in COUNTRY_URLS {
        let target = url(ip);
        match agent.get(&target).call() {
            Ok(response) => match response.into_string() {
                Ok(body) => match code_from(&body) {
                    Some(code) => return Ok(code),
                    None => last = format!("{target} named no country"),
                },
                Err(e) => last = format!("could not read {target}: {e}"),
            },
            // A 429 here means this endpoint's free tier is spent, which is exactly the case the
            // chain exists for: move on rather than leave the server unplaced.
            Err(ureq::Error::Status(status, _)) => last = format!("{target} returned {status}"),
            Err(e) => last = format!("{target}: {e}"),
        }
    }
    Err(last)
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

    // ---------------------------------------------------------------- exits
    // Through the proxies: what address does each server come out of?
    let mut exits: Vec<Option<String>> = vec![None; ports.len()];
    let mut remaining: Vec<usize> = (0..ports.len()).collect();

    for attempt in 0..ATTEMPTS {
        if remaining.is_empty() {
            break;
        }
        if attempt > 0 {
            log::info!("asking {} server(s) again", remaining.len());
            tokio::time::sleep(BACKOFF).await;
        }

        let mut still_missing = Vec::new();
        for chunk in remaining.chunks(MAX_CONCURRENCY) {
            // Each task carries its own index, so a failure can never shift a later result onto
            // the wrong server — which would be worse than no flag at all.
            let mut handles = Vec::with_capacity(chunk.len());
            for (position, &index) in chunk.iter().enumerate() {
                if position > 0 {
                    tokio::time::sleep(STAGGER).await;
                }
                let port = ports[index];
                handles.push((
                    index,
                    tokio::task::spawn_blocking(move || exit_ip_through(port)),
                ));
            }

            for (index, handle) in handles {
                match handle.await {
                    Ok(Ok(ip)) => exits[index] = Some(ip),
                    Ok(Err(why)) => {
                        log::info!("no exit address for server {index}: {why}");
                        still_missing.push(index);
                    }
                    Err(e) => {
                        log::debug!("probe task for index {index} failed: {e}");
                        still_missing.push(index);
                    }
                }
            }
        }
        remaining = still_missing;
    }

    // The scratch core has done its part; the country lookups do not go through it.
    scratch.shut_down().await;

    // ---------------------------------------------------------------- countries
    // One lookup per distinct exit, from this machine. Servers sharing an address — which is
    // the norm behind a CDN — share the one answer, which is what keeps this under the rate
    // limit that the naive design kept tripping.
    let mut countries: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let distinct: Vec<String> = {
        let mut seen: Vec<String> = exits.iter().flatten().cloned().collect();
        seen.sort();
        seen.dedup();
        seen
    };
    log::info!(
        "{} distinct exit(s) across {} server(s)",
        distinct.len(),
        profiles.len()
    );

    for chunk in distinct.chunks(MAX_CONCURRENCY) {
        let handles: Vec<_> = chunk
            .iter()
            .map(|ip| {
                let ip = ip.clone();
                (
                    ip.clone(),
                    tokio::task::spawn_blocking(move || country_of(&ip)),
                )
            })
            .collect();

        for (ip, handle) in handles {
            match handle.await {
                Ok(Ok(code)) => {
                    countries.insert(ip, code);
                }
                Ok(Err(why)) => log::info!("could not place {ip}: {why}"),
                Err(e) => log::debug!("country task for {ip} failed: {e}"),
            }
        }
    }

    let located: Vec<Located> = exits
        .into_iter()
        .enumerate()
        .filter_map(|(index, exit)| {
            let ip = exit?;
            let country = countries.get(&ip)?.clone();
            Some(Located { index, country, ip })
        })
        .collect();

    log::info!(
        "located {} of {} servers",
        located.len(),
        profiles.len()
    );
    Ok(located)
}
