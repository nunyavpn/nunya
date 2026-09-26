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

/// One place an address resolved to: the country always, the city and coordinates when the
/// lookup that answered knew them.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Spot {
    pub ip: String,
    /// Two-letter country code, uppercase, as the list expects it.
    pub country: String,
    pub city: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub asn: Option<u32>,
    /// Who runs the network: the data center or hosting company.
    pub org: Option<String>,
    /// The CDN this address belongs to, if any — `"cloudflare"`, `"fastly"`. On an entry it means
    /// the config is CDN-fronted: the client connects to a CDN edge, which forwards to the real
    /// server. Such configs can be tuned by choosing the edge address, which is what this is
    /// recorded for.
    pub cdn: Option<&'static str>,
}

/// Which CDN an address belongs to, by its network when the geo service reported one, else by the
/// providers' published address ranges.
///
/// Both, because either can be missing: the country-only fallback lookup reports no network, and
/// a provider's ranges are not the whole of its network (Cloudflare's WARP egress is outside its
/// published list). The ranges are the providers' own: Cloudflare's are kept, with everything
/// else about its edges, in `cloudflare.rs`; Fastly's are its public IP list.
///
/// Being Cloudflare's says nothing about *where*: an anycast address is answered by whichever
/// data center is nearest the asker. `cloudflare::observe` finds that out.
pub fn cdn_of(ip: &str, asn: Option<u32>) -> Option<&'static str> {
    match asn {
        Some(13335 | 209242) => return Some("cloudflare"),
        Some(54113) => return Some("fastly"),
        Some(_) => return None,
        None => {}
    }
    let addr: std::net::IpAddr = ip.parse().ok()?;
    if crate::cloudflare::owner(addr).is_some() {
        Some("cloudflare")
    } else if FASTLY_RANGES.iter().any(|r| in_range(addr, r)) {
        Some("fastly")
    } else {
        None
    }
}

const FASTLY_RANGES: [&str; 21] = [
    "23.235.32.0/20", "43.249.72.0/22", "103.244.50.0/24", "103.245.222.0/23", "103.245.224.0/24",
    "104.156.80.0/20", "140.248.64.0/18", "140.248.128.0/17", "146.75.0.0/17", "151.101.0.0/16",
    "157.52.64.0/18", "167.82.0.0/17", "167.82.128.0/20", "167.82.160.0/20", "167.82.224.0/20",
    "172.111.64.0/18", "185.31.16.0/22", "199.27.72.0/21", "199.232.0.0/16", "2a04:4e40::/32",
    "2a04:4e42::/32",
];

/// Whether `addr` is inside `cidr`. Addresses of the other family are never inside.
fn in_range(addr: std::net::IpAddr, cidr: &str) -> bool {
    let Some((net, len)) = cidr.split_once('/') else { return false };
    let (Ok(net), Ok(len)) = (net.parse::<std::net::IpAddr>(), len.parse::<u32>()) else {
        return false;
    };
    match (addr, net) {
        (std::net::IpAddr::V4(a), std::net::IpAddr::V4(n)) if len <= 32 => {
            let mask = if len == 0 { 0 } else { u32::MAX << (32 - len) };
            u32::from(a) & mask == u32::from(n) & mask
        }
        (std::net::IpAddr::V6(a), std::net::IpAddr::V6(n)) if len <= 128 => {
            let mask = if len == 0 { 0 } else { u128::MAX << (128 - len) };
            u128::from(a) & mask == u128::from(n) & mask
        }
        _ => false,
    }
}

/// Where one server is, keyed back to the caller's ordering.
///
/// Two places, because a server routinely has two. `entry` is its address — where the client
/// connects. `exit` is the public address its traffic leaves from, measured end to end through
/// the server. They differ whenever the server is a relay: a Cloudflare Workers proxy entered at
/// the nearest edge and leaving from wherever Cloudflare egresses, a domestic server tunnelled on
/// to a foreign one. The flag shows the exit, since that is what a website sees; the entry says
/// how the traffic got there.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Located {
    pub index: usize,
    pub entry: Option<Spot>,
    pub exit: Option<Spot>,
}

/// Places one address: coordinates if any endpoint in `PLACE_URLS` will give them, otherwise just
/// the country from `COUNTRY_URLS`.
///
/// The fallback keeps the flag working when every coordinate service is out of quota — the flag
/// was the original point of the sweep, and a map dot is not worth losing it for.
fn place_ip(ip: &str) -> Result<Spot, String> {
    match whereabouts(Some(ip)) {
        Ok(w) => Ok(Spot {
            cdn: cdn_of(&w.ip, w.asn),
            ip: w.ip,
            country: w.country,
            city: w.city,
            lat: Some(w.lat),
            lon: Some(w.lon),
            asn: w.asn,
            org: w.org,
        }),
        Err(why) => {
            log::debug!("no coordinates for {ip} ({why}); asking for the country alone");
            country_of(ip).map(|country| Spot {
                ip: ip.to_string(),
                country,
                city: None,
                lat: None,
                lon: None,
                asn: None,
                org: None,
                cdn: cdn_of(ip, None),
            })
        }
    }
}

/// The address a server's host name points at, or the host itself when it is already an address.
///
/// The system resolver, with a deadline: a resolver that hangs must not hold up a sweep. With a
/// VPN-mode tunnel up this resolves through the tunnel, which answers the same question.
async fn entry_ip(host: String, port: u16) -> Option<String> {
    let host = host.trim_matches(|c| c == '[' || c == ']').to_string();
    if host.parse::<std::net::IpAddr>().is_ok() {
        return Some(host);
    }
    let lookup = tokio::task::spawn_blocking(move || {
        use std::net::ToSocketAddrs;
        let addrs: Vec<SocketAddr> = (host.as_str(), port).to_socket_addrs().ok()?.collect();
        // IPv4 first: the geo endpoints place v4 far more reliably, and a dual-stack host is the
        // same machine either way.
        addrs
            .iter()
            .find(|a| a.is_ipv4())
            .or_else(|| addrs.first())
            .map(|a| a.ip().to_string())
    });
    match tokio::time::timeout(REQUEST_TIMEOUT, lookup).await {
        Ok(Ok(found)) => found,
        _ => None,
    }
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
/// What one server's traffic was seen to leave from: the plain answer, and Cloudflare's trace when
/// it answered. See `prefer_trace` for which becomes the exit.
#[derive(Debug, Clone)]
pub struct SeenExit {
    pub plain: String,
    pub trace: Option<String>,
}

/// An agent for the endpoints here: through the local listener on `proxy_port`, or direct.
///
/// Direct means direct. ureq 3 reads `HTTP_PROXY` and `ALL_PROXY` unless told otherwise, and a
/// request that is meant to leave from this machine's own connection — placing the user, placing
/// an address — must not quietly go through whatever the environment names.
pub(crate) fn agent(proxy_port: Option<u16>, timeout: Duration) -> Result<ureq::Agent, String> {
    let proxy = proxy_port
        .map(|port| ureq::Proxy::new(&format!("http://127.0.0.1:{port}")))
        .transpose()
        .map_err(|e| format!("bad proxy address: {e}"))?;
    Ok(ureq::Agent::config_builder()
        .proxy(proxy)
        .timeout_global(Some(timeout))
        .build()
        .into())
}

/// A text answer, or why there was none: a status is the endpoint answering, anything else is
/// the path to it failing, and a log should tell the two apart.
fn text(agent: &ureq::Agent, url: &str) -> Result<String, String> {
    match agent.get(url).call() {
        Ok(mut response) => response
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("could not read {url}: {e}")),
        Err(ureq::Error::StatusCode(status)) => Err(format!("{url} returned {status}")),
        Err(e) => Err(format!("{url}: {e}")),
    }
}

fn exits_through(port: u16) -> Result<SeenExit, String> {
    let plain = exit_ip_through(port)?;
    let agent = agent(Some(port), REQUEST_TIMEOUT)?;
    let trace = cloudflare_exit(&agent).map(|c| c.ip).filter(|ip| *ip != plain);
    Ok(SeenExit { plain, trace })
}

fn exit_ip_through(port: u16) -> Result<String, String> {
    let agent = agent(Some(port), REQUEST_TIMEOUT)?;

    let mut last = String::from("no endpoint was tried");
    for url in EXIT_IP_URLS {
        match text(&agent, url) {
            Ok(body) => {
                let candidate = body.trim();
                // Validated rather than trusted: a captive portal or an error page would
                // otherwise become an "address" and then a nonsense flag.
                if candidate.parse::<std::net::IpAddr>().is_ok() {
                    return Ok(candidate.to_string());
                }
                last = format!("{url} did not return an address");
            }
            // Either way the next endpoint is worth trying.
            Err(e) => last = e,
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
    let agent = agent(None, REQUEST_TIMEOUT)?;

    let mut last = String::from("no endpoint was tried");
    for url in COUNTRY_URLS {
        let target = url(ip);
        match text(&agent, &target) {
            Ok(body) => match code_from(&body) {
                Some(code) => return Ok(code),
                None => last = format!("{target} named no country"),
            },
            // A 429 here means this endpoint's free tier is spent, which is exactly the case the
            // chain exists for: move on rather than leave the server unplaced.
            Err(e) => last = e,
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
pub async fn locate(
    core_path: &Path,
    profiles: &[Profile],
    want_entries: bool,
    probe_exits: bool,
) -> Result<Vec<Located>, String> {
    if profiles.is_empty() {
        return Ok(Vec::new());
    }

    // Entries need nothing but DNS, so they are found for every server — including one that is
    // down, which is exactly when knowing where it is supposed to be helps.
    let entries: Vec<Option<String>> = if !want_entries {
        vec![None; profiles.len()]
    } else {
        let lookups: Vec<_> = profiles
            .iter()
            .map(|p| tokio::spawn(entry_ip(p.server.clone(), p.port)))
            .collect();
        let mut found = Vec::with_capacity(lookups.len());
        for lookup in lookups {
            found.push(lookup.await.ok().flatten());
        }
        found
    };

    // Exits need the server to be up. A probe that cannot even start still leaves the entries.
    let exits = if probe_exits {
        match exit_ips(core_path, profiles).await {
            Ok(exits) => exits,
            Err(e) => {
                log::info!("no exits measured: {e}");
                vec![None; profiles.len()]
            }
        }
    } else {
        vec![None; profiles.len()]
    };

    // ---------------------------------------------------------------- places
    // One lookup per distinct address, entries and exits together, from this machine. Servers
    // sharing an address — the norm behind a CDN, and every relay whose entry is another's exit
    // — share the one answer, which is what keeps this under the rate limit that the naive
    // design kept tripping.
    let distinct: Vec<String> = {
        let mut seen: Vec<String> = entries.iter().flatten().cloned().collect();
        for exit in exits.iter().flatten() {
            seen.push(exit.plain.clone());
            seen.extend(exit.trace.clone());
        }
        seen.sort();
        seen.dedup();
        seen
    };
    log::info!(
        "{} distinct address(es) across {} server(s)",
        distinct.len(),
        profiles.len()
    );

    let mut places: std::collections::HashMap<String, Spot> = std::collections::HashMap::new();
    for chunk in distinct.chunks(MAX_CONCURRENCY) {
        let handles: Vec<_> = chunk
            .iter()
            .map(|ip| {
                let ip = ip.clone();
                (ip.clone(), tokio::task::spawn_blocking(move || place_ip(&ip)))
            })
            .collect();
        for (ip, handle) in handles {
            match handle.await {
                Ok(Ok(spot)) => {
                    places.insert(ip, spot);
                }
                Ok(Err(why)) => log::info!("could not place {ip}: {why}"),
                Err(e) => log::debug!("place task for {ip} failed: {e}"),
            }
        }
    }

    let located: Vec<Located> = entries
        .into_iter()
        .zip(exits)
        .enumerate()
        .filter_map(|(index, (entry, exit))| {
            let entry = entry.and_then(|ip| places.get(&ip).cloned());
            let exit = exit.and_then(|seen| {
                let plain = places.get(&seen.plain).cloned();
                let trace = seen.trace.and_then(|ip| places.get(&ip).cloned());
                choose_exit(plain, trace)
            });
            (entry.is_some() || exit.is_some()).then_some(Located { index, entry, exit })
        })
        .collect();

    log::info!(
        "placed {} entries and {} exits across {} servers",
        located.iter().filter(|l| l.entry.is_some()).count(),
        located.iter().filter(|l| l.exit.is_some()).count(),
        profiles.len()
    );
    Ok(located)
}

/// Asks each server, through a scratch core, what public address its traffic leaves from.
///
/// `None` where a server could not be reached; the caller keeps whatever it had.
/// Picks the exit from what was placed: the trace answer when `prefer_trace` says so, the plain
/// one otherwise, and whichever exists when only one could be placed.
fn choose_exit(plain: Option<Spot>, trace: Option<Spot>) -> Option<Spot> {
    match (plain, trace) {
        (Some(p), Some(t)) if prefer_trace(p.asn, t.asn) => Some(t),
        (Some(p), _) => Some(p),
        (None, t) => t,
    }
}

/// Places addresses once per run, however many servers share them — the rate-limit discipline
/// `locate` keeps with its distinct set, for checks that arrive one server at a time.
#[derive(Default)]
pub struct PlaceCache(std::sync::Mutex<std::collections::HashMap<String, Option<Spot>>>);

impl PlaceCache {
    fn get(&self, ip: &str) -> Option<Spot> {
        if let Some(hit) = self.0.lock().unwrap_or_else(|p| p.into_inner()).get(ip) {
            return hit.clone();
        }
        let placed = place_ip(ip).map_err(|e| log::info!("could not place {ip}: {e}")).ok();
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(ip.to_string(), placed.clone());
        placed
    }

    /// Where a server's traffic left from, by `choose_exit`. `None` when neither address could be
    /// placed — the server still carried traffic; the geo services just did not say where.
    pub fn exit(&self, seen: &SeenExit) -> Option<Spot> {
        let plain = self.get(&seen.plain);
        let trace = seen.trace.as_deref().and_then(|ip| self.get(ip));
        choose_exit(plain, trace)
    }
}

/// Servers one probe core takes at once; see `ProbeSession::start`.
pub const MAX_PROBE: usize = 256;

/// A scratch core with one local proxy port per server, alive for as long as a check needs it.
///
/// One per run, not one per server: starting a core costs far more than asking through one, and
/// a run asks through every server. The link is held so the core's control connection stays
/// open — the core treats losing it as being orphaned.
pub struct ProbeSession {
    scratch: Scratch,
    pub ports: Vec<u16>,
    _link: Arc<CoreLink>,
}

impl ProbeSession {
    pub async fn start(core_path: &Path, profiles: &[Profile]) -> Result<ProbeSession, String> {
        // A port and a listener per server. Asked for thousands at once — a public list of twenty
        // thousand, all in one call — this would exhaust the process's file descriptors and hand
        // the core a config it cannot start. Callers batch; this makes a caller that forgot fail
        // with a reason instead of taking the app down.
        if profiles.len() > MAX_PROBE {
            return Err(format!(
                "{} servers in one probe; at most {MAX_PROBE} at a time",
                profiles.len()
            ));
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

        Ok(ProbeSession {
            scratch,
            ports,
            _link: link,
        })
    }

    /// What server `index`'s traffic was seen to leave from. Blocking; run it off the runtime.
    pub fn exit_of(&self, index: usize) -> Result<SeenExit, String> {
        let port = *self.ports.get(index).ok_or("no probe port for that server")?;
        exits_through(port)
    }

    pub async fn shut_down(self) {
        self.scratch.shut_down().await;
    }
}

async fn exit_ips(core_path: &Path, profiles: &[Profile]) -> Result<Vec<Option<SeenExit>>, String> {
    let session = ProbeSession::start(core_path, profiles).await?;
    let ports = session.ports.clone();

    // ---------------------------------------------------------------- exits
    // Through the proxies: what address does each server come out of?
    let mut exits: Vec<Option<SeenExit>> = vec![None; ports.len()];
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
                    tokio::task::spawn_blocking(move || exits_through(port)),
                ));
            }

            for (index, handle) in handles {
                match handle.await {
                    Ok(Ok(seen)) => exits[index] = Some(seen),
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

    // The scratch core has done its part; the lookups do not go through it.
    session.shut_down().await;
    Ok(exits)
}

// ---------------------------------------------------------------- where the user is

/// A point on the map: an address and where a geo database puts it.
///
/// Coordinates rather than only a country, because this is what the map draws the user's own
/// dot and the tunnel's exit from, and a country's centroid puts everyone in Russia in Siberia.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Whereabouts {
    pub ip: String,
    /// Two-letter country code, uppercase.
    pub country: String,
    pub city: Option<String>,
    pub lat: f64,
    pub lon: f64,
    /// The network the address belongs to, when the service said. What tells a Cloudflare egress
    /// address — shared, and placed inconsistently by every geo database — from a real server.
    pub asn: Option<u32>,
    /// Who runs that network — the data center or hosting company ("GTHost", "Cloudflare, Inc.").
    pub org: Option<String>,
}

/// Cloudflare's network. Its Workers and WARP egress addresses belong to it.
pub const CLOUDFLARE_ASN: u32 = 13335;

/// Which of two addresses seen through one server is its exit: the plain answer (asked of a site
/// not on Cloudflare) or Cloudflare's trace (what Cloudflare-hosted sites see).
///
/// They differ for a Cloudflare Workers server — BPB and its kind — which sends Cloudflare-hosted
/// destinations through a fixed "proxy IP" and everything else out of the Worker. The Worker's
/// address is Cloudflare's shared egress pool: neighbouring addresses in it were placed in Sofia,
/// Bucharest and New York by the same service on the same day, so it says nothing about where the
/// server is. The proxy IP is a real machine in a real datacenter, and it is what a "what is my
/// IP" page shows. So when the plain answer is known to be Cloudflare's and the trace is not, the
/// trace is the exit. In every other case the plain answer stands — including when either
/// network is unknown, rather than guessing.
pub fn prefer_trace(plain_asn: Option<u32>, trace_asn: Option<u32>) -> bool {
    plain_asn == Some(CLOUDFLARE_ASN) && trace_asn.is_some_and(|asn| asn != CLOUDFLARE_ASN)
}

/// Endpoints that place an address, or the caller when given none.
///
/// Several for the same reason as `COUNTRY_URLS`: free tiers run out. HTTPS first, since unlike a
/// country code this answer is a city, and the plain-HTTP one is kept because it has outlived the
/// others' quotas before.
///
/// `api.ip.sb` is here for networks that filter: it sits behind Cloudflare, which the networks
/// this client is for cannot block wholesale — too much of their own web is behind it.
const PLACE_URLS: [fn(Option<&str>) -> String; 4] = [
    |ip| format!("https://ipwho.is/{}", ip.unwrap_or("")),
    |ip| match ip {
        Some(ip) => format!("https://ipinfo.io/{ip}/json"),
        None => "https://ipinfo.io/json".to_string(),
    },
    |ip| format!("https://api.ip.sb/geoip/{}", ip.unwrap_or("")),
    |ip| {
        format!(
            "http://ip-api.com/json/{}?fields=status,query,countryCode,city,lat,lon,as",
            ip.unwrap_or("")
        )
    },
];

/// Reads whichever of the endpoints' shapes the body is in.
///
/// Three services, three spellings of the same five facts (`ip`/`query`, `country_code`/
/// `countryCode`/`country`, `latitude`/`lat`, or ipinfo's `"loc": "lat,lon"`), so this looks for
/// each fact by its known names rather than keeping a schema per endpoint.
fn whereabouts_from(body: &str) -> Option<Whereabouts> {
    let v: serde_json::Value = serde_json::from_str(body.trim()).ok()?;
    // ipwho.is and ip-api answer 200 with a failure inside.
    if v.get("success").and_then(|s| s.as_bool()) == Some(false)
        || v.get("status").and_then(|s| s.as_str()) == Some("fail")
    {
        return None;
    }

    let text = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| v.get(*k).and_then(|x| x.as_str()))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
    };
    let number = |keys: &[&str]| keys.iter().find_map(|k| v.get(*k).and_then(|x| x.as_f64()));

    let ip = text(&["ip", "query"])?;
    ip.parse::<std::net::IpAddr>().ok()?;

    let country = text(&["country_code", "countryCode", "country"])?.to_ascii_uppercase();
    if country.len() != 2 || !country.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }

    let (lat, lon) = match (number(&["latitude", "lat"]), number(&["longitude", "lon"])) {
        (Some(lat), Some(lon)) => (lat, lon),
        _ => {
            let loc = text(&["loc"])?;
            let (lat, lon) = loc.split_once(',')?;
            (lat.trim().parse().ok()?, lon.trim().parse().ok()?)
        }
    };
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=180.0).contains(&lon) {
        return None;
    }

    // ipwho.is gives the number and the name apart, and so does ip.sb, with names of its own;
    // ipinfo and ip-api give one string, "AS13335 Cloudflare, Inc.", which is split here.
    let as_string = text(&["org", "as"]);
    let org = v
        .get("connection")
        .and_then(|c| c.get("org").or_else(|| c.get("isp")))
        .and_then(|o| o.as_str())
        .map(str::trim)
        .filter(|o| !o.is_empty())
        .map(str::to_string)
        .or_else(|| text(&["asn_organization", "organization", "isp"]))
        .or_else(|| {
            let s = as_string.as_deref()?;
            // Drop the leading "AS13335 "; a string with no number in front is already a name.
            let name = match s.strip_prefix("AS") {
                Some(rest) => rest.trim_start_matches(|c: char| c.is_ascii_digit()).trim(),
                None => s,
            };
            (!name.is_empty()).then(|| name.to_string())
        });
    let asn = v
        .get("connection")
        .and_then(|c| c.get("asn"))
        .and_then(|a| a.as_u64())
        .or_else(|| v.get("asn").and_then(|a| a.as_u64()))
        .and_then(|a| u32::try_from(a).ok())
        .or_else(|| {
            let org = text(&["org", "as"])?;
            let digits: String = org.strip_prefix("AS")?.chars().take_while(char::is_ascii_digit).collect();
            digits.parse().ok()
        });

    Some(Whereabouts {
        ip,
        country,
        city: text(&["city"]),
        lat,
        lon,
        asn,
        org,
    })
}

/// Places an address, or this machine's own public address when given `None`, asking directly.
///
/// "Directly" means over whatever route the OS has: with a VPN-mode tunnel up that is the tunnel,
/// so asking about `None` then answers for the exit. The frontend relies on exactly that.
///
/// **Asking where *this machine* is asks every endpoint at once** and takes the first answer.
/// One at a time is right for placing servers' exits — dozens of addresses, and the free tiers to
/// spare — but this is one address, once a launch, and on a filtered network the endpoints that
/// are blocked do not refuse: they hang until the timeout. Asked in turn, two blocked endpoints
/// cost ten seconds before a reachable one is even tried, which is longer than the launch waits.
pub fn whereabouts(ip: Option<&str>) -> Result<Whereabouts, String> {
    match ip {
        Some(ip) => in_turn(ip),
        None => all_at_once(),
    }
}

fn ask(agent: &ureq::Agent, target: &str, ip: Option<&str>) -> Result<Whereabouts, String> {
    let body = text(agent, target)?;
    match whereabouts_from(&body) {
        // An answer about some other address is not an answer. A service that ignores the path —
        // rate-limited, or misreading an IPv6 literal — describes the *caller*, and this once
        // filed the user's own location as a server's exit.
        Some(found) if !answers_for(ip, &found.ip) => {
            Err(format!("{target} answered about {} instead", found.ip))
        }
        Some(found) => Ok(found),
        None => Err(format!("{target} did not say where")),
    }
}

fn in_turn(ip: &str) -> Result<Whereabouts, String> {
    let agent = agent(None, REQUEST_TIMEOUT)?;
    let mut last = String::from("no endpoint was tried");
    for url in PLACE_URLS {
        match ask(&agent, &url(Some(ip)), Some(ip)) {
            Ok(found) => return Ok(found),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn all_at_once() -> Result<Whereabouts, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    for url in PLACE_URLS {
        let tx = tx.clone();
        std::thread::spawn(move || {
            // The receiver is gone once an answer has been taken; the rest finish unheard.
            let _ = tx.send(agent(None, REQUEST_TIMEOUT).and_then(|a| ask(&a, &url(None), None)));
        });
    }
    drop(tx);

    let mut last = String::from("no endpoint answered");
    // Ends when every endpoint has answered or failed, since each sender is dropped with its
    // thread; the first success leaves at once.
    for outcome in rx {
        match outcome {
            Ok(found) => return Ok(found),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Whether an answer is about the address that was asked about. Compared as addresses, so an
/// IPv6 literal written two ways still matches. Asking about "this machine" accepts any answer.
fn answers_for(asked: Option<&str>, answered: &str) -> bool {
    let Some(asked) = asked else { return true };
    match (asked.parse::<std::net::IpAddr>(), answered.parse::<std::net::IpAddr>()) {
        (Ok(a), Ok(b)) => a == b,
        _ => asked == answered,
    }
}

/// Endpoints that answer only over one address family, so each question gets one family's answer.
///
/// Not Cloudflare-hosted, for the reason `EXIT_IP_URLS` is not: a Cloudflare Workers server, the
/// most common free kind, cannot reach Cloudflare, and every question here would fail through it.
const IPV4_URLS: [&str; 2] = ["http://checkip.amazonaws.com/", "http://v4.ident.me/"];
const IPV6_URLS: [&str; 2] = ["http://v6.ident.me/", "http://api6.ipify.org/"];

/// The running tunnel's public addresses, one per family, and where the main one is.
///
/// Both families, because a tunnel can leave from two quite different places: through a
/// Cloudflare Workers server the IPv6 address is the Worker's, while the IPv4 one can be a relay
/// the provider bolted on. A "what is my IP" page shows both, so the status card does too.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Exit {
    pub ipv4: Option<String>,
    pub ipv6: Option<String>,
    /// Placed from the IPv4 address when there is one, else the IPv6 one.
    pub place: Option<Whereabouts>,
    /// The address Cloudflare-hosted sites see, when Cloudflare answered through the tunnel.
    pub cloudflare: Option<CloudflareExit>,
}

/// What a Cloudflare-hosted site sees of the tunnel.
///
/// Asked separately because for the most common free servers it is a *different* exit. A
/// Cloudflare Workers proxy cannot open a connection to Cloudflare's own addresses, so panels like
/// BPB route Cloudflare-hosted destinations through a "proxy IP" — a relay outside Cloudflare —
/// while everything else leaves from the Worker. The IPv4/IPv6 endpoints above are chosen *not* to
/// be on Cloudflare, so they measure the Worker; this measures the relay. A "what is my IP" page
/// behind Cloudflare shows this one, and without it the status card would seem to disagree.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudflareExit {
    pub ip: String,
    /// Cloudflare's own two-letter country for the address, from the same answer.
    pub country: Option<String>,
}

/// Cloudflare's trace page, which reports the caller's address and country on any Cloudflare
/// domain. Plain HTTP first, as the other endpoints are; HTTPS on a second domain as a fallback.
const CLOUDFLARE_TRACE_URLS: [&str; 2] = [
    "http://www.cloudflare.com/cdn-cgi/trace",
    "https://one.one.one.one/cdn-cgi/trace",
];

/// Asks the tunnel for its public addresses — through the proxy-mode listener on `port`, or
/// directly when a VPN-mode tunnel already carries everything — and places the main one from the
/// user's own connection, for the rate-limit reason `locate` gives.
///
/// The two families are asked at once: the missing one costs a timeout, and a tunnel with no IPv6
/// is common enough that it should not double the wait.
pub fn exit_addresses(port: Option<u16>) -> Result<Exit, String> {
    let agent = agent(port, REQUEST_TIMEOUT)?;

    let (ipv4, ipv6, cloudflare) = std::thread::scope(|scope| {
        let v4 = scope.spawn(|| family_ip(&agent, &IPV4_URLS, false));
        let v6 = scope.spawn(|| family_ip(&agent, &IPV6_URLS, true));
        let cf = scope.spawn(|| cloudflare_exit(&agent));
        (
            v4.join().ok().flatten(),
            v6.join().ok().flatten(),
            cf.join().ok().flatten(),
        )
    });

    let (mut ipv4, mut ipv6, mut cloudflare) = (ipv4, ipv6, cloudflare);
    let main = ipv4.clone().or_else(|| ipv6.clone());
    let Some(main) = main else {
        return Err("the tunnel reported no public address in either family".into());
    };

    // Both placed, then the same rule the per-server probe uses: a Cloudflare egress address is
    // not where the server is when Cloudflare itself reports a real machine behind it. Asked
    // independently, so a failed lookup of the Worker's address cannot hide the proxy IP.
    let plain = whereabouts(Some(&main))
        .map_err(|e| log::info!("could not place the exit {main}: {e}"))
        .ok();
    let trace = cloudflare
        .as_ref()
        .filter(|cf| cf.ip != main)
        .and_then(|cf| whereabouts(Some(&cf.ip)).ok());
    let (place, trace_is_exit) = match (plain, trace) {
        (Some(p), Some(t)) if prefer_trace(p.asn, t.asn) => (Some(t), true),
        (Some(p), _) => (Some(p), false),
        (None, Some(t)) => (Some(t), true),
        (None, None) => (None, false),
    };

    // When the proxy IP is the exit it *is* the public address of its family — the one a "what is
    // my IP" page shows — so it takes the Worker's place in that chip, and stops being a separate
    // "Cloudflare sites" line that would only repeat it.
    if trace_is_exit {
        if let Some(cf) = cloudflare.take() {
            match cf.ip.parse::<std::net::IpAddr>() {
                Ok(ip) if ip.is_ipv4() => ipv4 = Some(cf.ip),
                Ok(_) => ipv6 = Some(cf.ip),
                Err(_) => {}
            }
        }
    }

    Ok(Exit {
        ipv4,
        ipv6,
        place,
        cloudflare,
    })
}

fn cloudflare_exit(agent: &ureq::Agent) -> Option<CloudflareExit> {
    for url in CLOUDFLARE_TRACE_URLS {
        let Ok(body) = text(agent, url) else { continue };
        if let Some(found) = parse_trace(&body) {
            return Some(found);
        }
    }
    None
}

/// Reads `key=value` lines: `ip=` must be an address; `loc=` is kept when it is a country code
/// (Cloudflare writes `XX` for one it does not know, which is not a country).
fn parse_trace(body: &str) -> Option<CloudflareExit> {
    let field = |key: &str| {
        body.lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix('='))
            .map(str::trim)
    };
    let ip = field("ip")?.parse::<std::net::IpAddr>().ok()?.to_string();
    let country = field("loc")
        .map(str::to_ascii_uppercase)
        .filter(|c| c.len() == 2 && c.chars().all(|ch| ch.is_ascii_alphabetic()) && c != "XX");
    Some(CloudflareExit { ip, country })
}

/// The first answer from `urls` that is an address of the wanted family.
fn family_ip(agent: &ureq::Agent, urls: &[&str], v6: bool) -> Option<String> {
    for url in urls {
        let Ok(body) = text(agent, url) else { continue };
        match body.trim().parse::<std::net::IpAddr>() {
            Ok(ip) if ip.is_ipv6() == v6 => return Some(ip.to_string()),
            _ => continue,
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::{whereabouts_from, Whereabouts};

    fn place(ip: &str, country: &str, city: Option<&str>, lat: f64, lon: f64) -> Whereabouts {
        Whereabouts {
            ip: ip.into(),
            country: country.into(),
            city: city.map(Into::into),
            lat,
            lon,
            asn: None,
            org: None,
        }
    }

    #[tokio::test]
    async fn an_address_is_its_own_entry_without_asking_dns() {
        assert_eq!(super::entry_ip("203.0.113.9".into(), 443).await.as_deref(), Some("203.0.113.9"));
        // A share link writes an IPv6 literal in brackets; the brackets are not part of it.
        assert_eq!(super::entry_ip("[2001:db8::7]".into(), 443).await.as_deref(), Some("2001:db8::7"));
    }

    #[tokio::test]
    async fn a_name_that_does_not_resolve_has_no_entry_rather_than_a_wrong_one() {
        // RFC 6761 reserves .invalid so that it never resolves.
        assert_eq!(super::entry_ip("nothing.invalid".into(), 443).await, None);
    }

    #[test]
    fn an_answer_about_another_address_is_not_taken_for_the_one_asked() {
        use super::answers_for;
        assert!(answers_for(Some("203.0.113.7"), "203.0.113.7"));
        // The same IPv6 address, written expanded and compressed.
        assert!(answers_for(Some("2001:db8:0:0:0:0:0:1"), "2001:db8::1"));
        // What a service that ignored the path would send back: the caller.
        assert!(!answers_for(Some("203.0.113.7"), "198.51.100.4"));
        // Asking about this machine takes whatever it is.
        assert!(answers_for(None, "198.51.100.4"));
    }

    #[test]
    fn cloudflares_trace_page_gives_the_address_and_country() {
        let body = "fl=12f\nh=www.cloudflare.com\nip=67.220.82.10\nts=1.2\nloc=DE\nwarp=off\n";
        assert_eq!(
            super::parse_trace(body),
            Some(super::CloudflareExit { ip: "67.220.82.10".into(), country: Some("DE".into()) })
        );
        // `XX` is Cloudflare for "unknown", not a country; the address still counts.
        assert_eq!(
            super::parse_trace("ip=2001:db8::1\nloc=XX\n").map(|c| c.country),
            Some(None)
        );
        // `visit_scheme=` must not be mistaken for `ip=` by a careless prefix match.
        assert_eq!(super::parse_trace("visit_scheme=http\nloc=DE\n"), None);
    }

    #[test]
    fn a_cloudflare_egress_gives_way_to_the_real_machine_behind_it() {
        use super::{prefer_trace, CLOUDFLARE_ASN};
        // BPB: the Worker's address is Cloudflare's, the proxy IP is GTHost's.
        assert!(prefer_trace(Some(CLOUDFLARE_ASN), Some(63023)));
        // A normal server, or one whose trace is also Cloudflare (WARP): the plain answer stands.
        assert!(!prefer_trace(Some(63023), Some(24940)));
        assert!(!prefer_trace(Some(CLOUDFLARE_ASN), Some(CLOUDFLARE_ASN)));
        // Not knowing either network is not a reason to switch.
        assert!(!prefer_trace(None, Some(63023)));
        assert!(!prefer_trace(Some(CLOUDFLARE_ASN), None));
    }

    #[test]
    fn the_network_is_read_from_every_services_spelling_of_it() {
        let ipwhois = r#"{"ip":"104.28.154.231","success":true,"country_code":"BG","city":"Sofia","latitude":42.7,"longitude":23.3,"connection":{"asn":13335,"org":"Cloudflare, Inc."}}"#;
        let w = whereabouts_from(ipwhois).unwrap();
        assert_eq!((w.asn, w.org.as_deref()), (Some(13335), Some("Cloudflare, Inc.")));
        let ipinfo = r#"{"ip":"67.220.82.10","city":"Frankfurt am Main","country":"DE","loc":"50.1109,8.6820","org":"AS63023 GTHost"}"#;
        let w = whereabouts_from(ipinfo).unwrap();
        assert_eq!((w.asn, w.org.as_deref()), (Some(63023), Some("GTHost")));
        let ip_api = r#"{"status":"success","countryCode":"DE","city":"Frankfurt am Main","lat":50.11,"lon":8.68,"as":"AS63023 GTHost","query":"67.220.82.10"}"#;
        let w = whereabouts_from(ip_api).unwrap();
        assert_eq!((w.asn, w.org.as_deref()), (Some(63023), Some("GTHost")));
    }

    #[test]
    fn a_cdn_is_recognised_by_its_network_or_its_published_ranges() {
        use super::cdn_of;
        // The config address from the Nexisci config, a Cloudflare node.
        assert_eq!(cdn_of("172.67.134.220", Some(13335)), Some("cloudflare"));
        assert_eq!(cdn_of("172.67.134.220", None), Some("cloudflare"));
        assert_eq!(cdn_of("2606:4700::6810:84e5", None), Some("cloudflare"));
        assert_eq!(cdn_of("151.101.1.140", None), Some("fastly"));
        assert_eq!(cdn_of("151.101.1.140", Some(54113)), Some("fastly"));
        // A known network that is not a CDN wins over any range guess.
        assert_eq!(cdn_of("67.220.82.10", Some(63023)), None);
        // Just outside Cloudflare's 104.24.0.0/14, which ends at 104.27.255.255.
        assert_eq!(cdn_of("104.28.0.1", None), None);
    }

    /// The whole per-server check against a real server: one probe core, the exit through it,
    /// and where that is placed. Ignored: it needs a core and a server's profile.
    ///
    ///     NUNYA_CORE_PATH=vendor/core/bin/nunya-core NUNYA_PROFILE=profile.json \
    ///       cargo test --manifest-path src-tauri/Cargo.toml live_probe -- --ignored --nocapture
    #[tokio::test]
    #[ignore]
    async fn live_probe_of_one_server() {
        let core = std::path::PathBuf::from(std::env::var("NUNYA_CORE_PATH").expect("NUNYA_CORE_PATH"));
        let text = std::fs::read_to_string(std::env::var("NUNYA_PROFILE").expect("NUNYA_PROFILE")).unwrap();
        let profile: crate::config::Profile = serde_json::from_str(&text).unwrap();
        let session = std::sync::Arc::new(super::ProbeSession::start(&core, &[profile]).await.unwrap());
        let port = session.ports[0];
        let s2 = session.clone();
        let (seen, placed, card) = tokio::task::spawn_blocking(move || {
            let seen = s2.exit_of(0);
            let placed = seen.as_ref().ok().and_then(|seen| super::PlaceCache::default().exit(seen));
            (seen, placed, super::exit_addresses(Some(port)))
        })
        .await
        .unwrap();
        println!("seen through the server: {seen:#?}\nsaved as the exit: {placed:#?}\nstatus card: {card:#?}");
        if let Ok(session) = std::sync::Arc::try_unwrap(session) {
            session.shut_down().await;
        }
    }

    /// What the status card would show for a live tunnel. Ignored: it needs one running.
    ///
    ///     NUNYA_LIVE_PROXY=2080 cargo test --manifest-path src-tauri/Cargo.toml \
    ///       live_exit -- --ignored --nocapture
    /// Where this machine is, asked of every endpoint at once, as a launch does it. Needs the
    /// network, so it is ignored by default:
    ///
    ///     cargo test --manifest-path src-tauri/Cargo.toml live_place -- --ignored --nocapture
    ///
    /// Also says which endpoints answer from here, which is the question when one of them starts
    /// failing for a whole country.
    #[test]
    #[ignore]
    fn live_place_of_this_machine() {
        // Every endpoint first, and the verdict last: this test is run to find out what a network
        // answers, and an assertion in the middle of it hides the half that says so.
        let agent = super::agent(None, super::REQUEST_TIMEOUT).unwrap();
        for url in super::PLACE_URLS {
            let target = url(None);
            let at = std::time::Instant::now();
            let one = super::ask(&agent, &target, None);
            println!(
                "{target}: {} in {:?}",
                match &one {
                    Ok(place) => format!("{}, {}", place.country, place.city.as_deref().unwrap_or("—")),
                    Err(e) => e.clone(),
                },
                at.elapsed()
            );
        }

        let raced = std::time::Instant::now();
        let found = super::whereabouts(None);
        println!("all at once: {:?} in {:?}", found, raced.elapsed());
        assert!(
            found.is_ok(),
            "no endpoint placed this machine; the lines above say why, and scripts/net-check.sh \
             says whether it is the names or the addresses"
        );
    }

    #[test]
    #[ignore]
    fn live_exit_through_a_running_tunnel() {
        let port: u16 = std::env::var("NUNYA_LIVE_PROXY").expect("set NUNYA_LIVE_PROXY").parse().unwrap();
        println!("{:#?}", super::exit_addresses(Some(port)));
    }

    #[test]
    fn ipwhois_answers_are_read() {
        let body = r#"{"ip":"203.0.113.7","success":true,"country_code":"DE","city":"Frankfurt am Main","latitude":50.11,"longitude":8.68}"#;
        assert_eq!(
            whereabouts_from(body),
            Some(place("203.0.113.7", "DE", Some("Frankfurt am Main"), 50.11, 8.68))
        );
    }

    #[test]
    fn ipinfo_puts_both_coordinates_in_one_string_and_that_is_read_too() {
        let body = r#"{"ip":"198.51.100.4","city":"Tehran","country":"IR","loc":"35.6944,51.4215"}"#;
        assert_eq!(
            whereabouts_from(body),
            Some(place("198.51.100.4", "IR", Some("Tehran"), 35.6944, 51.4215))
        );
    }

    #[test]
    fn ip_api_answers_are_read() {
        let body = r#"{"status":"success","countryCode":"NL","city":"Amsterdam","lat":52.37,"lon":4.89,"query":"2001:db8::1"}"#;
        assert_eq!(
            whereabouts_from(body),
            Some(place("2001:db8::1", "NL", Some("Amsterdam"), 52.37, 4.89))
        );
    }

    /// ip.sb spells the network as a number of its own and names it separately; it is the one
    /// endpoint here chosen for reaching through a filtered network, so its shape is guarded.
    #[test]
    fn ip_sb_answers_are_read_including_its_network() {
        let body = r#"{"ip":"203.0.113.9","country_code":"IR","city":"Tehran","latitude":35.69,"longitude":51.42,"asn":58224,"asn_organization":"Telecommunication Infrastructure Company","isp":"TIC"}"#;
        let found = whereabouts_from(body).expect("read");
        assert_eq!(found.country, "IR");
        assert_eq!(found.city.as_deref(), Some("Tehran"));
        assert_eq!(found.asn, Some(58224));
        assert_eq!(found.org.as_deref(), Some("Telecommunication Infrastructure Company"));
    }

    #[test]
    fn a_failure_reported_inside_a_200_is_still_a_failure() {
        assert_eq!(
            whereabouts_from(r#"{"success":false,"message":"Reserved range","ip":"10.0.0.1"}"#),
            None
        );
        assert_eq!(
            whereabouts_from(r#"{"status":"fail","message":"quota","query":"10.0.0.1"}"#),
            None
        );
    }

    #[test]
    fn an_answer_missing_a_location_or_address_is_refused_rather_than_guessed() {
        // A rate-limit page, a captive portal, a missing coordinate: none of them may become a dot
        // on the map in the middle of the Atlantic.
        assert_eq!(whereabouts_from("Too Many Requests"), None);
        assert_eq!(
            whereabouts_from(r#"{"ip":"203.0.113.7","country_code":"DE"}"#),
            None
        );
        assert_eq!(
            whereabouts_from(r#"{"ip":"not an ip","country":"DE","loc":"1,2"}"#),
            None
        );
    }

    /// What sing-box's HTTP proxy answers a request it cannot forward: an empty `502`, then the
    /// connection reset. ureq 2 returned such a connection to its pool the moment it had read the
    /// head, clearing its timeouts with `setsockopt` — which XNU refuses (EINVAL) on a socket that
    /// can neither send nor receive, as a reset one cannot — and turned that into a panic: an abort
    /// in a release build (issue #44). Every exit lookup and every server check can meet one.
    ///
    /// The window is between reading the head and pooling the connection, microseconds wide and on
    /// the client's side, so no answer lands in it every time: a reset before the head is an
    /// ordinary error, one after the pooling harmless. Sweeping the delay before the reset found
    /// it about once in a thousand requests, which reproduced the panic but cannot be relied on
    /// to catch it. What this guards is the rest: through ureq 3 every such answer is an error.
    fn serve_empty_502() -> u16 {
        use std::io::{BufRead, BufReader, Write};
        const HEAD: &[u8] = b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for (n, stream) in listener.incoming().enumerate() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                while reader.read_line(&mut line).map(|n| n > 2).unwrap_or(false) {
                    line.clear();
                }
                let _ = stream.write_all(HEAD);
                std::thread::sleep(std::time::Duration::from_micros((n % 60) as u64));
                // SO_LINGER 0: closing sends a reset rather than a FIN.
                let linger = libc::linger { l_onoff: 1, l_linger: 0 };
                // SAFETY: a live socket and a correctly sized option value.
                unsafe {
                    use std::os::fd::AsRawFd;
                    libc::setsockopt(
                        stream.as_raw_fd(),
                        libc::SOL_SOCKET,
                        libc::SO_LINGER,
                        &linger as *const _ as *const libc::c_void,
                        std::mem::size_of::<libc::linger>() as libc::socklen_t,
                    );
                }
                drop(reader);
                drop(stream);
            }
        });
        port
    }

    #[test]
    fn an_empty_answer_from_the_proxy_is_an_error_not_a_crash() {
        let port = serve_empty_502();
        for _ in 0..500 {
            assert!(super::exit_ip_through(port).is_err());
        }
        // The other paths through a local proxy port meet the same answer.
        assert!(super::exit_addresses(Some(port)).is_err());
        assert!(crate::blocklists::download(crate::blocklists::List::Ads, Some(port)).is_err());
    }
}
