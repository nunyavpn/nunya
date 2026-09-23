//! Which Cloudflare data center a CDN-fronted config actually enters at, observed rather than
//! looked up.
//!
//! A config fronted by Cloudflare connects to an anycast address: one address announced from
//! every Cloudflare data center at once, answered by whichever one BGP carries this network to.
//! Geo databases give such an address a single place anyway — `104.21.77.84` is "Los Angeles" in
//! all of them — and that place is not where anything is. Measured, the same address was answered
//! from Newark (`EWR`) on one network and Frankfurt (`FRA`) on an Iranian one. So the GeoIP answer
//! for a Cloudflare address is kept as what it is, a database entry, and the data center is asked
//! instead.
//!
//! Asking is one HTTPS request, made the way `curl --resolve host:443:ip` makes it: a TCP
//! connection to the anycast address itself, the config's host name as TLS SNI and HTTP `Host`,
//! and Cloudflare names the data center that answered — in the `CF-Ray` header every response
//! carries (`a3f9c1183a075d86-FRA`), and in `/cdn-cgi/trace`'s `colo=` where the zone serves it.
//! The path asked for is the trace page, so one request gets both; the header is read first,
//! because unlike the trace page it is on every Cloudflare response, a 404 included.
//!
//! Three rules follow from what anycast is, and each is written down where it is kept:
//!
//! - **An address has no data center, only a path does.** Nothing here maps an address to a colo;
//!   an observation is cached under the host, the address *and the network it was seen from*,
//!   and it expires ([`EdgeCache`]).
//! - **Unknown is an answer.** A response without a ray, a failed handshake, a colo code missing
//!   from the table: each is reported as itself ([`Edge`]), never filled in from GeoIP or from
//!   the address. The frontend shows no city rather than a wrong one.
//! - **Ownership is not location.** Whether an address is Cloudflare's comes from the ranges
//!   Cloudflare publishes ([`owner`]), apart from any geo service, and says nothing about where.
//!
//! The ranges and the colo table are Cloudflare's own machine-readable lists, compiled in from
//! `data/cloudflare/` and refreshed by `scripts/update-cloudflare-data.sh`. Not fetched at run
//! time: the networks this client is for are the ones where that fetch would fail.
//!
//! The request is HTTP/1.1, so the name travels as `Host`; an HTTP/2 client would send the same
//! name as `:authority`, and Cloudflare routes on either the same way.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::config::Profile;
use crate::geo::CLOUDFLARE_ASN;

/// Cloudflare's published ranges, one CIDR per line: cloudflare.com/ips-v4 and /ips-v6.
const IPS_V4: &str = include_str!("../data/cloudflare/ips-v4.txt");
const IPS_V6: &str = include_str!("../data/cloudflare/ips-v6.txt");
/// Cloudflare's data centers by code, from speed.cloudflare.com/locations.
const COLOS: &str = include_str!("../data/cloudflare/colos.json");

/// How long an observation stands. Long enough that the status card is not asking on every
/// repaint; short enough that a change of route inside one network is picked up the same hour.
/// A change of network does not wait for this: it is a different key.
pub const OBSERVATION_TTL: Duration = Duration::from_secs(10 * 60);

/// The most of a body read looking for `colo=`. A trace page is a few hundred bytes; a zone that
/// answers the path with a large page of its own has already said it is not serving the trace.
const BODY_LIMIT: u64 = 64 * 1024;

// ---------------------------------------------------------------- whose address

/// One published range, parsed once.
#[derive(Debug, Clone, Copy)]
struct Cidr {
    net: IpAddr,
    len: u8,
}

impl Cidr {
    fn parse(s: &str) -> Option<Cidr> {
        let (net, len) = s.trim().split_once('/')?;
        let (net, len): (IpAddr, u8) = (net.parse().ok()?, len.parse().ok()?);
        let max = if net.is_ipv4() { 32 } else { 128 };
        (len <= max).then_some(Cidr { net, len })
    }

    fn contains(&self, addr: IpAddr) -> bool {
        match (addr, self.net) {
            (IpAddr::V4(a), IpAddr::V4(n)) => {
                let mask = u32::MAX.checked_shl(32 - u32::from(self.len)).unwrap_or(0);
                u32::from(a) & mask == u32::from(n) & mask
            }
            (IpAddr::V6(a), IpAddr::V6(n)) => {
                let mask = u128::MAX.checked_shl(128 - u32::from(self.len)).unwrap_or(0);
                u128::from(a) & mask == u128::from(n) & mask
            }
            _ => false,
        }
    }
}

fn ranges() -> &'static [Cidr] {
    static RANGES: OnceLock<Vec<Cidr>> = OnceLock::new();
    RANGES.get_or_init(|| {
        IPS_V4
            .lines()
            .chain(IPS_V6.lines())
            .filter(|l| !l.trim().is_empty())
            // The update script refuses a file with anything else in it, and a test checks the
            // shipped ones, so a line that does not parse here is not silently in the data.
            .filter_map(Cidr::parse)
            .collect()
    })
}

/// Who announces an address, when it is Cloudflare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Owner {
    pub provider: &'static str,
    pub asn: u32,
}

/// Whether `addr` is inside a range Cloudflare publishes as its own.
///
/// The published list is not the whole of AS13335 — WARP's egress is outside it — which is why
/// `geo::cdn_of` also trusts a geo service's AS number. For an *entry* the list is the right test:
/// a config can only be fronted by an address Cloudflare serves customers' sites from.
pub fn owner(addr: IpAddr) -> Option<Owner> {
    ranges()
        .iter()
        .any(|r| r.contains(addr))
        .then_some(Owner { provider: "Cloudflare", asn: CLOUDFLARE_ASN })
}

// ---------------------------------------------------------------- where a colo is

/// One Cloudflare data center, as Cloudflare lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Colo {
    /// The IATA-style code: the suffix of a `CF-Ray`.
    pub iata: String,
    pub city: String,
    /// Two-letter code, uppercase. Cloudflare's list calls it `cca2`.
    #[serde(alias = "cca2")]
    pub country: String,
    #[serde(default)]
    pub region: Option<String>,
    pub lat: f64,
    pub lon: f64,
}

fn colos() -> &'static HashMap<String, Colo> {
    static COLOS_BY_CODE: OnceLock<HashMap<String, Colo>> = OnceLock::new();
    COLOS_BY_CODE.get_or_init(|| {
        let list: Vec<Colo> = serde_json::from_str(COLOS).unwrap_or_else(|e| {
            // A test parses the shipped file; this is only reachable with a hand-edited one.
            log::error!("the Cloudflare colo table does not parse: {e}");
            Vec::new()
        });
        list.into_iter().map(|c| (c.iata.clone(), c)).collect()
    })
}

/// Where a data center is, when Cloudflare's list knows the code. `None` is not an error: new
/// data centers open between refreshes of the table, and their code is still the answer.
pub fn colo(code: &str) -> Option<&'static Colo> {
    colos().get(&code.to_ascii_uppercase())
}

/// A colo code as Cloudflare writes one: three letters.
fn colo_code(s: &str) -> Option<String> {
    let s = s.trim();
    (s.len() == 3 && s.chars().all(|c| c.is_ascii_alphabetic())).then(|| s.to_ascii_uppercase())
}

/// The data center out of a `CF-Ray` value — `a3f9c1183a075d86-FRA` → `FRA`. The ray id before
/// the dash must be hex, so a header that merely has a dash in it is not taken for one.
pub fn colo_from_ray(ray: &str) -> Option<String> {
    let (id, colo) = ray.trim().rsplit_once('-')?;
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    colo_code(colo)
}

/// The data center out of a `/cdn-cgi/trace` body's `colo=` line.
pub fn colo_from_trace(body: &str) -> Option<String> {
    body.lines().find_map(|l| colo_code(l.trim().strip_prefix("colo=")?))
}

// ---------------------------------------------------------------- asking

/// Which part of Cloudflare's answer named the data center.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Via {
    #[serde(rename = "cf-ray")]
    CfRay,
    #[serde(rename = "trace")]
    Trace,
}

/// What one probe saw.
#[derive(Debug, Clone, PartialEq)]
pub struct Seen {
    pub colo: String,
    pub via: Via,
    /// The TLS handshake with the edge, in milliseconds: one round trip under TLS 1.3, and the
    /// number routing can compare between edges. See `handshake` for why not the TCP connect.
    pub rtt_ms: u32,
}

/// Why a probe did not name a data center.
#[derive(Debug, Clone, PartialEq)]
pub enum ProbeError {
    /// The handshake failed — a certificate that is not for the name, not trusted, or a
    /// middlebox in the way. Its answer, had there been one, could not be believed.
    Tls(String),
    Timeout,
    /// Refused, unreachable, reset: the edge could not be asked at all.
    Failed(String),
    /// Something answered over TLS, with neither a `CF-Ray` nor a trace naming a data center.
    NoColo(String),
}

/// How a probe connects. Only tests change it: another port, a certificate of their own, a
/// deadline short enough to test.
#[derive(Clone)]
pub struct Probe {
    pub timeout: Duration,
    pub port: u16,
    /// Trust roots other than the bundled public ones (`webpki-roots`, as ureq's default).
    pub tls: Option<Arc<rustls::ClientConfig>>,
}

impl Default for Probe {
    fn default() -> Self {
        Probe { timeout: Duration::from_secs(5), port: 443, tls: None }
    }
}

/// Resolves every name to one address: `curl --resolve`. The agent it is given to is used for one
/// request, with redirects off, so no other name can be sent to this address.
struct Pinned(SocketAddr);

impl ureq::Resolver for Pinned {
    fn resolve(&self, _netloc: &str) -> std::io::Result<Vec<SocketAddr>> {
        Ok(vec![self.0])
    }
}

/// Asks the edge at `ip` which data center it is, sending `host` as SNI and `Host`.
///
/// Direct, over whatever route the OS has, and never through a proxy: the question is which edge
/// *this network* reaches, which is also the edge the core reaches when it dials the config.
pub fn probe(host: &str, ip: IpAddr, opts: &Probe) -> Result<Seen, ProbeError> {
    let addr = SocketAddr::new(ip, opts.port);
    let tls = opts.tls.clone().unwrap_or_else(public_roots);

    let rtt_ms = handshake(host, addr, tls.clone(), opts.timeout)?;

    let agent = ureq::AgentBuilder::new()
        .resolver(Pinned(addr))
        .tls_config(tls)
        .timeout(opts.timeout)
        .redirects(0)
        .user_agent(concat!("Nunya/", env!("CARGO_PKG_VERSION")))
        .build();
    let url = if opts.port == 443 {
        format!("https://{host}/cdn-cgi/trace")
    } else {
        format!("https://{host}:{}/cdn-cgi/trace", opts.port)
    };

    let response = match agent.get(&url).call() {
        Ok(r) => r,
        // A 404 or a 403 from Cloudflare still names the data center that sent it.
        Err(ureq::Error::Status(_, r)) => r,
        Err(ureq::Error::Transport(t)) => return Err(transport_error(&t)),
    };

    if let Some(colo) = response.header("cf-ray").and_then(colo_from_ray) {
        return Ok(Seen { colo, via: Via::CfRay, rtt_ms });
    }
    let status = response.status();
    let mut body = String::new();
    use std::io::Read;
    let _ = response.into_reader().take(BODY_LIMIT).read_to_string(&mut body);
    match colo_from_trace(&body) {
        Some(colo) => Ok(Seen { colo, via: Via::Trace, rtt_ms }),
        None => Err(ProbeError::NoColo(format!(
            "answered {status} with no CF-Ray header and no trace"
        ))),
    }
}

/// The public roots ureq trusts by default, made once, so the timed handshake and the request
/// trust exactly the same certificates.
fn public_roots() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .expect("ring supports the default protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

/// How long the edge takes to complete a TLS handshake for `host`, in milliseconds.
///
/// Not the TCP connect, which is what "round trip" first suggests and what this first measured.
/// Anything on this machine that terminates TCP — a TUN-based VPN or a transparent proxy, both
/// common on the networks this client is for — answers the SYN itself, and the connect then
/// takes a quarter of a millisecond to an edge a continent away (measured: 0.26 ms to connect,
/// 660 ms to finish TLS). A handshake cannot be answered locally: it needs the edge's certificate
/// and key. Under TLS 1.3 it is one round trip plus the edge's signature, which is noise.
///
/// A connection of its own, so the time is the handshake's alone; the request that follows makes
/// another. Its failures are the probe's: a certificate refused here is a TLS failure, reported
/// before anything is asked over the connection.
fn handshake(
    host: &str,
    addr: SocketAddr,
    tls: Arc<rustls::ClientConfig>,
    timeout: Duration,
) -> Result<u32, ProbeError> {
    let io = |e: std::io::Error| io_error(&e);
    let mut tcp = TcpStream::connect_timeout(&addr, timeout).map_err(io)?;
    tcp.set_read_timeout(Some(timeout)).map_err(io)?;
    tcp.set_write_timeout(Some(timeout)).map_err(io)?;
    let name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|e| ProbeError::Failed(format!("{host:?} cannot be sent as SNI: {e}")))?;
    let mut conn =
        rustls::ClientConnection::new(tls, name).map_err(|e| ProbeError::Tls(e.to_string()))?;

    let started = Instant::now();
    while conn.is_handshaking() {
        conn.complete_io(&mut tcp).map_err(io)?;
    }
    Ok(u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX))
}

/// A timeout, a TLS failure — rustls reports those inside an `io::Error` — or anything else.
fn io_error(e: &std::io::Error) -> ProbeError {
    if let Some(tls) = e.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>()) {
        return ProbeError::Tls(tls.to_string());
    }
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => ProbeError::Timeout,
        _ => ProbeError::Failed(e.to_string()),
    }
}

/// Sorts a failed request into a timeout, a TLS failure, or anything else, by what caused it.
///
/// By cause, not by ureq's kind: a handshake that times out and one that is refused a
/// certificate are the same `ConnectionFailed` to it. A rustls error sits *inside* an
/// `io::Error`, which `source()` walks straight past, so each `io::Error` is opened by hand.
fn transport_error(t: &ureq::Transport) -> ProbeError {
    let mut next: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(t);
    while let Some(e) = next {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            if matches!(io.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) {
                return ProbeError::Timeout;
            }
            if let Some(tls) = io.get_ref().and_then(|inner| inner.downcast_ref::<rustls::Error>()) {
                return ProbeError::Tls(tls.to_string());
            }
        }
        if let Some(tls) = e.downcast_ref::<rustls::Error>() {
            return ProbeError::Tls(tls.to_string());
        }
        next = e.source();
    }
    ProbeError::Failed(t.to_string())
}

// ---------------------------------------------------------------- the answer

/// The network an observation was made from. Two machines — or one laptop on two networks — can
/// reach one anycast address at two data centers, so this is part of every cache key.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SourceNetwork {
    /// The address the system sends from to reach the internet (`netwatch::route_source`).
    pub local: Option<IpAddr>,
    /// This machine's public address, when known — two networks can hand out the same private one.
    pub public: Option<IpAddr>,
}

/// What asking about one edge came to. Every case is a different thing to tell the user, and none
/// of them is filled in from a guess.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum Edge {
    /// Not an address.
    #[serde(rename_all = "camelCase")]
    InvalidIp { reason: String },
    /// Not one of Cloudflare's; its GeoIP place is as good as any address's.
    NotCloudflare,
    /// Cloudflare's, but the config names no host to send as SNI, and without one Cloudflare
    /// cannot tell which of its customers is being asked for.
    NoHostname,
    /// Cloudflare named the data center.
    #[serde(rename_all = "camelCase")]
    Observed {
        colo: String,
        /// Where that data center is, from Cloudflare's list; `None` for a code it does not have
        /// yet — the code is still the answer, and no other place is put in its stead.
        place: Option<Colo>,
        source: Via,
        rtt_ms: u32,
        /// Unix milliseconds.
        observed_at: u64,
    },
    /// Cloudflare's address, reached over TLS, and nothing in the answer named a data center.
    #[serde(rename_all = "camelCase")]
    NotObservable { reason: String },
    #[serde(rename_all = "camelCase")]
    TlsFailed { reason: String },
    Timeout,
    #[serde(rename_all = "camelCase")]
    ProbeFailed { reason: String },
}

impl Edge {
    /// Worth keeping for `OBSERVATION_TTL`: a fact about the path. Failures are not — they are as
    /// likely to be this minute's network as anything about the edge.
    fn lasts(&self) -> bool {
        matches!(self, Edge::Observed { .. } | Edge::NotObservable { .. })
    }
}

/// One observation, with what it was asked about.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub ip: String,
    /// The name sent as SNI and `Host`.
    pub host: Option<String>,
    /// `"Cloudflare"` when the address is in its published ranges.
    pub provider: Option<&'static str>,
    pub asn: Option<u32>,
    pub edge: Edge,
    pub source_network: SourceNetwork,
}

/// The name a CDN-fronted config is served under: what its TLS layer sends as SNI, else the Host
/// its transport sends, else its server when that is a name. The first that is a DNS name wins;
/// an address cannot be sent as SNI, and Cloudflare would not know whose site it was anyway.
pub fn edge_host(profile: &Profile) -> Option<String> {
    let transport_host = profile.transport.host.split(',').next().unwrap_or("");
    [profile.tls.sni.as_str(), transport_host, profile.server.as_str()]
        .into_iter()
        .find_map(dns_name)
}

fn dns_name(s: &str) -> Option<String> {
    let s = s.trim().trim_end_matches('.').to_ascii_lowercase();
    if s.is_empty() || s.trim_matches(|c| c == '[' || c == ']').parse::<IpAddr>().is_ok() {
        return None;
    }
    rustls::pki_types::DnsName::try_from(s.as_str()).ok()?;
    Some(s)
}

// ---------------------------------------------------------------- remembering

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Key {
    host: String,
    ip: IpAddr,
    source: SourceNetwork,
}

/// Observations by host, address and source network, each for `ttl`.
///
/// Never by address alone. `104.21.77.84 → FRA` is true from one network and false from the next,
/// and a table of it would be a GeoIP database with extra steps.
pub struct EdgeCache {
    ttl: Duration,
    seen: Mutex<HashMap<Key, (Instant, Edge)>>,
}

impl Default for EdgeCache {
    fn default() -> Self {
        EdgeCache::new(OBSERVATION_TTL)
    }
}

impl EdgeCache {
    pub fn new(ttl: Duration) -> Self {
        EdgeCache { ttl, seen: Mutex::new(HashMap::new()) }
    }

    fn get(&self, key: &Key, now: Instant) -> Option<Edge> {
        let seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        let (at, edge) = seen.get(key)?;
        (now.saturating_duration_since(*at) < self.ttl).then(|| edge.clone())
    }

    fn put(&self, key: Key, edge: Edge, now: Instant) {
        let mut seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        // Expired ones go on every write, so a long session on many networks does not grow this.
        seen.retain(|_, (at, _)| now.saturating_duration_since(*at) < self.ttl);
        seen.insert(key, (now, edge));
    }
}

/// Which data center `ip` answers `host` from, seen from `source`: from the cache while an
/// observation stands, else by a [`probe`].
pub fn observe(
    host: Option<&str>,
    ip: &str,
    source: SourceNetwork,
    cache: &EdgeCache,
    opts: &Probe,
) -> Report {
    observe_with(host, ip, source, cache, Instant::now(), |host, ip| probe(host, ip, opts))
}

/// [`observe`] with the probe and the clock handed in, so the rules around them can be tested
/// without a network.
fn observe_with(
    host: Option<&str>,
    ip: &str,
    source: SourceNetwork,
    cache: &EdgeCache,
    now: Instant,
    probe: impl FnOnce(&str, IpAddr) -> Result<Seen, ProbeError>,
) -> Report {
    let host = host.and_then(dns_name);
    let mut report = Report {
        ip: ip.to_string(),
        host: host.clone(),
        provider: None,
        asn: None,
        edge: Edge::NotCloudflare,
        source_network: source.clone(),
    };

    let addr = match ip.trim().trim_matches(|c| c == '[' || c == ']').parse::<IpAddr>() {
        Ok(addr) => addr,
        Err(e) => {
            report.edge = Edge::InvalidIp { reason: format!("{ip:?} is not an address: {e}") };
            return report;
        }
    };
    report.ip = addr.to_string();
    let Some(owner) = owner(addr) else { return report };
    report.provider = Some(owner.provider);
    report.asn = Some(owner.asn);
    let Some(host) = host else {
        report.edge = Edge::NoHostname;
        return report;
    };

    let key = Key { host: host.clone(), ip: addr, source };
    if let Some(edge) = cache.get(&key, now) {
        report.edge = edge;
        return report;
    }

    report.edge = match probe(&host, addr) {
        Ok(seen) => Edge::Observed {
            place: colo(&seen.colo).cloned(),
            colo: seen.colo,
            source: seen.via,
            rtt_ms: seen.rtt_ms,
            observed_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        },
        Err(ProbeError::NoColo(reason)) => Edge::NotObservable { reason },
        Err(ProbeError::Tls(reason)) => Edge::TlsFailed { reason },
        Err(ProbeError::Timeout) => Edge::Timeout,
        Err(ProbeError::Failed(reason)) => Edge::ProbeFailed { reason },
    };
    if report.edge.lasts() {
        cache.put(key, report.edge.clone(), now);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn net(local: &str, public: &str) -> SourceNetwork {
        SourceNetwork { local: Some(ip(local)), public: Some(ip(public)) }
    }

    fn seen(colo: &str) -> Result<Seen, ProbeError> {
        Ok(Seen { colo: colo.into(), via: Via::CfRay, rtt_ms: 12 })
    }

    fn colo_of(report: &Report) -> Option<&str> {
        match &report.edge {
            Edge::Observed { colo, .. } => Some(colo),
            _ => None,
        }
    }

    // ------------------------------------------------------------ ownership

    #[test]
    fn every_shipped_range_parses_and_both_families_are_there() {
        let lines: Vec<&str> =
            IPS_V4.lines().chain(IPS_V6.lines()).filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(ranges().len(), lines.len(), "a line in data/cloudflare/ips-*.txt did not parse");
        assert!(ranges().iter().any(|r| r.net.is_ipv4()));
        assert!(ranges().iter().any(|r| r.net.is_ipv6()));
    }

    #[test]
    fn a_cloudflare_ipv4_address_is_cloudflares_and_its_neighbours_outside_are_not() {
        let cf = Some(Owner { provider: "Cloudflare", asn: 13335 });
        assert_eq!(owner(ip("104.21.77.84")), cf);
        assert_eq!(owner(ip("172.67.134.220")), cf);
        // 104.16.0.0/13 ends where 104.24.0.0/14 begins; 104.28.0.0 is outside both.
        assert_eq!(owner(ip("104.23.255.255")), cf);
        assert_eq!(owner(ip("104.24.0.0")), cf);
        assert_eq!(owner(ip("104.28.0.1")), None);
    }

    #[test]
    fn a_cloudflare_ipv6_address_is_cloudflares_including_inside_a_short_prefix() {
        assert!(owner(ip("2606:4700:3036::6815:4d54")).is_some());
        assert!(owner(ip("2400:cb00::1")).is_some());
        // 2a06:98c0::/29 runs to 2a06:98c7:ffff:…; the next /29 is someone else's.
        assert!(owner(ip("2a06:98c7:ffff::1")).is_some());
        assert!(owner(ip("2a06:98c8::1")).is_none());
    }

    #[test]
    fn an_address_that_is_not_cloudflares_is_not_and_is_not_probed() {
        for other in ["8.8.8.8", "67.220.82.10", "151.101.1.140", "127.0.0.1", "::1", "2001:4860::8888"] {
            assert_eq!(owner(ip(other)), None, "{other}");
            let report = observe_with(Some("edge.test"), other, SourceNetwork::default(), &EdgeCache::default(), Instant::now(), |_, _| {
                panic!("{other} is not Cloudflare's and must not be probed")
            });
            assert_eq!(report.edge, Edge::NotCloudflare);
            assert_eq!((report.provider, report.asn), (None, None));
        }
    }

    #[test]
    fn something_that_is_not_an_address_is_reported_as_such() {
        for bad in ["104.21.77", "network.alinaderiparizi.com", "", "2606:4700::zz"] {
            let report = observe_with(Some("edge.test"), bad, SourceNetwork::default(), &EdgeCache::default(), Instant::now(), |_, _| {
                panic!("nothing to probe")
            });
            assert!(matches!(report.edge, Edge::InvalidIp { .. }), "{bad}: {:?}", report.edge);
        }
    }

    #[test]
    fn a_config_with_no_name_to_send_cannot_be_asked() {
        let report = observe_with(None, "104.21.77.84", SourceNetwork::default(), &EdgeCache::default(), Instant::now(), |_, _| {
            panic!("no SNI to probe with")
        });
        assert_eq!(report.edge, Edge::NoHostname);
        assert_eq!(report.provider, Some("Cloudflare"));
        // An address is not a name either.
        let report = observe_with(Some("104.21.77.84"), "104.21.77.84", SourceNetwork::default(), &EdgeCache::default(), Instant::now(), |_, _| {
            panic!("an address is not an SNI")
        });
        assert_eq!(report.edge, Edge::NoHostname);
    }

    #[test]
    fn the_name_sent_is_the_sni_then_the_host_header_then_the_server() {
        let mut p: Profile = serde_json::from_value(serde_json::json!({
            "name": "x", "server": "104.21.77.84", "port": 443, "uuid": "",
            "tls": { "enabled": true, "sni": "Network.AlinaderiParizi.com." },
            "transport": { "kind": "ws", "host": "other.example.net" }
        }))
        .unwrap();
        assert_eq!(edge_host(&p).as_deref(), Some("network.alinaderiparizi.com"));
        p.tls.sni.clear();
        assert_eq!(edge_host(&p).as_deref(), Some("other.example.net"));
        p.transport.host.clear();
        assert_eq!(edge_host(&p), None, "the server is an address, which is no name");
        p.server = "edge.example.net".into();
        assert_eq!(edge_host(&p).as_deref(), Some("edge.example.net"));
    }

    // ------------------------------------------------------------ reading the answer

    #[test]
    fn the_colo_is_the_suffix_of_a_cf_ray() {
        assert_eq!(colo_from_ray("a3f9c1183a075d86-FRA").as_deref(), Some("FRA"));
        assert_eq!(colo_from_ray(" a3f9c6052fe693b7-ewr ").as_deref(), Some("EWR"));
        // No id, an id that is not a ray id, a suffix that is not a code.
        assert_eq!(colo_from_ray("-FRA"), None);
        assert_eq!(colo_from_ray("not-FRA"), None);
        assert_eq!(colo_from_ray("a3f9c1183a075d86-FRANKFURT"), None);
        assert_eq!(colo_from_ray("a3f9c1183a075d86"), None);
        assert_eq!(colo_from_ray(""), None);
    }

    #[test]
    fn the_colo_is_read_from_a_trace_body() {
        let body = "fl=12f123\nh=network.alinaderiparizi.com\nip=178.252.132.98\nts=1.2\n\
                    visit_scheme=https\nuag=curl\ncolo=FRA\nsliver=none\nhttp=http/2\nloc=IR\n";
        assert_eq!(colo_from_trace(body).as_deref(), Some("FRA"));
        assert_eq!(colo_from_trace("ip=1.2.3.4\nloc=IR\n"), None);
        assert_eq!(colo_from_trace("<html>colo=FRA is not a line</html>"), None);
    }

    #[test]
    fn the_colo_table_is_cloudflares_list_and_places_the_codes_seen() {
        assert!(colos().len() > 200, "data/cloudflare/colos.json did not parse");
        for (code, city, country) in [
            ("EWR", "Newark", "US"),
            ("FRA", "Frankfurt", "DE"),
            ("LHR", "London", "GB"),
            ("AMS", "Amsterdam", "NL"),
            ("LAX", "Los Angeles", "US"),
            ("SJC", "San Jose", "US"),
            ("SIN", "Singapore", "SG"),
        ] {
            let c = colo(code).unwrap_or_else(|| panic!("{code} missing"));
            assert!(c.city.starts_with(city), "{code}: {}", c.city);
            assert_eq!(c.country, country);
        }
        assert_eq!(colo("fra").map(|c| c.iata.as_str()), Some("FRA"));
    }

    // ------------------------------------------------------------ never a guess

    #[test]
    fn a_colo_missing_from_the_table_is_reported_as_its_code_with_no_place() {
        let report = observe_with(Some("edge.test"), "104.21.77.84", SourceNetwork::default(), &EdgeCache::default(), Instant::now(), |_, _| seen("QQQ"));
        match report.edge {
            Edge::Observed { colo, place, .. } => {
                assert_eq!(colo, "QQQ");
                assert_eq!(place, None, "no place may be put in for a code the table lacks");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_answer_without_a_colo_is_unobservable_and_says_nothing_of_where() {
        let report = observe_with(Some("edge.test"), "104.21.77.84", SourceNetwork::default(), &EdgeCache::default(), Instant::now(), |_, _| {
            Err(ProbeError::NoColo("answered 200 with no CF-Ray header and no trace".into()))
        });
        assert!(matches!(report.edge, Edge::NotObservable { .. }));
        // The only places a report can carry are inside `Observed`, and they come from the colo
        // table: there is no field a GeoIP city could be put into.
        let json = serde_json::to_value(&report).unwrap();
        assert!(json.pointer("/edge/place").is_none());
        assert!(json.get("city").is_none() && json.get("country").is_none());
    }

    #[test]
    fn an_anycast_address_is_placed_where_it_answered_not_where_geoip_puts_it() {
        // GeoIP says Los Angeles for this address; from this network it answers in Frankfurt.
        let report = observe_with(Some("network.alinaderiparizi.com"), "104.21.77.84", net("192.168.1.20", "178.252.132.98"), &EdgeCache::default(), Instant::now(), |_, _| seen("FRA"));
        match &report.edge {
            Edge::Observed { place: Some(place), .. } => {
                assert_eq!(place.country, "DE");
                assert_ne!(place.city, "Los Angeles");
            }
            other => panic!("{other:?}"),
        }
    }

    // ------------------------------------------------------------ remembering

    #[test]
    fn one_address_seen_from_two_networks_is_two_observations() {
        let cache = EdgeCache::default();
        let now = Instant::now();
        let brazil = net("192.168.0.10", "187.14.56.70");
        let iran = net("192.168.0.10", "178.252.132.98");
        let host = Some("network.alinaderiparizi.com");

        let a = observe_with(host, "104.21.77.84", brazil.clone(), &cache, now, |_, _| seen("EWR"));
        let b = observe_with(host, "104.21.77.84", iran.clone(), &cache, now, |_, _| seen("FRA"));
        assert_eq!(colo_of(&a), Some("EWR"));
        assert_eq!(colo_of(&b), Some("FRA"));

        // Each is remembered under its own network, and neither overwrote the other.
        let again = |source| observe_with(host, "104.21.77.84", source, &cache, now, |_, _| panic!("should be cached"));
        assert_eq!(colo_of(&again(brazil)), Some("EWR"));
        assert_eq!(colo_of(&again(iran)), Some("FRA"));
        // A network nothing was seen from is asked afresh, not given either answer.
        let asked = Cell::new(false);
        let fresh = observe_with(host, "104.21.77.84", net("10.0.0.2", "5.160.0.1"), &cache, now, |_, _| {
            asked.set(true);
            seen("IST")
        });
        assert!(asked.get());
        assert_eq!(colo_of(&fresh), Some("IST"));
    }

    #[test]
    fn an_observation_is_kept_per_host_as_well_as_per_address() {
        let cache = EdgeCache::default();
        let now = Instant::now();
        observe_with(Some("a.example.net"), "104.21.77.84", SourceNetwork::default(), &cache, now, |_, _| seen("FRA"));
        let asked = Cell::new(false);
        observe_with(Some("b.example.net"), "104.21.77.84", SourceNetwork::default(), &cache, now, |_, _| {
            asked.set(true);
            seen("FRA")
        });
        assert!(asked.get(), "another host on the same address is its own question");
    }

    #[test]
    fn an_observation_expires() {
        let cache = EdgeCache::new(Duration::from_secs(60));
        let t0 = Instant::now();
        let host = Some("edge.test");
        observe_with(host, "104.21.77.84", SourceNetwork::default(), &cache, t0, |_, _| seen("FRA"));

        let within = observe_with(host, "104.21.77.84", SourceNetwork::default(), &cache, t0 + Duration::from_secs(59), |_, _| panic!("still fresh"));
        assert_eq!(colo_of(&within), Some("FRA"));

        let asked = Cell::new(false);
        let after = observe_with(host, "104.21.77.84", SourceNetwork::default(), &cache, t0 + Duration::from_secs(60), |_, _| {
            asked.set(true);
            seen("AMS")
        });
        assert!(asked.get(), "an expired observation must be asked again");
        assert_eq!(colo_of(&after), Some("AMS"));
    }

    #[test]
    fn a_failure_is_not_remembered() {
        let cache = EdgeCache::default();
        let now = Instant::now();
        observe_with(Some("edge.test"), "104.21.77.84", SourceNetwork::default(), &cache, now, |_, _| Err(ProbeError::Timeout));
        let asked = Cell::new(false);
        observe_with(Some("edge.test"), "104.21.77.84", SourceNetwork::default(), &cache, now, |_, _| {
            asked.set(true);
            seen("FRA")
        });
        assert!(asked.get(), "a timeout is this minute's network, not a fact to keep");
    }

    #[test]
    fn a_report_reads_as_the_frontend_expects() {
        let report = observe_with(Some("network.alinaderiparizi.com"), "104.21.77.84", net("192.168.1.20", "178.252.132.98"), &EdgeCache::default(), Instant::now(), |_, _| seen("FRA"));
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["provider"], "Cloudflare");
        assert_eq!(json["asn"], 13335);
        assert_eq!(json["edge"]["status"], "observed");
        assert_eq!(json["edge"]["colo"], "FRA");
        assert_eq!(json["edge"]["source"], "cf-ray");
        assert_eq!(json["edge"]["place"]["country"], "DE");
        assert_eq!(json["sourceNetwork"]["public"], "178.252.132.98");
        assert_eq!(serde_json::to_value(Edge::TlsFailed { reason: "x".into() }).unwrap()["status"], "tlsFailed");
        assert_eq!(serde_json::to_value(Edge::NoHostname).unwrap()["status"], "noHostname");
    }

    // ------------------------------------------------------------ the probe itself, over TLS
    //
    // A local TLS server with a certificate for `edge.test` (RFC 2606: it resolves nowhere), from
    // a throwaway CA in tests/fixtures/edge-tls. The probe reaches it only because the address is
    // pinned — which is the point being tested.

    const CA: &[u8] = include_bytes!("../tests/fixtures/edge-tls/ca.der");
    const LEAF: &[u8] = include_bytes!("../tests/fixtures/edge-tls/leaf.der");
    const LEAF_KEY: &[u8] = include_bytes!("../tests/fixtures/edge-tls/leaf.key.der");

    /// What the server saw of the request.
    #[derive(Debug, Default)]
    struct Heard {
        sni: Option<String>,
        request_line: String,
        host: Option<String>,
    }

    #[derive(Debug)]
    struct RecordSni {
        key: Arc<rustls::sign::CertifiedKey>,
        sni: mpsc::Sender<Option<String>>,
    }

    impl rustls::server::ResolvesServerCert for RecordSni {
        fn resolve(&self, hello: rustls::server::ClientHello<'_>) -> Option<Arc<rustls::sign::CertifiedKey>> {
            let _ = self.sni.send(hello.server_name().map(str::to_string));
            Some(self.key.clone())
        }
    }

    fn provider() -> Arc<rustls::crypto::CryptoProvider> {
        Arc::new(rustls::crypto::ring::default_provider())
    }

    /// A client that trusts only the test CA.
    fn trusting_test_ca() -> Probe {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(rustls::pki_types::CertificateDer::from(CA.to_vec())).unwrap();
        let config = rustls::ClientConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Probe { timeout: Duration::from_secs(5), port: 0, tls: Some(Arc::new(config)) }
    }

    /// Serves one TLS connection on 127.0.0.1 with `response`, and reports what it heard.
    fn serve_once(response: &'static str) -> (u16, mpsc::Receiver<Heard>) {
        serve_after(Duration::ZERO, response)
    }

    /// `serve_once`, answering each handshake only after `delay` — an edge that far away.
    fn serve_after(delay: Duration, response: &'static str) -> (u16, mpsc::Receiver<Heard>) {
        let key = rustls::pki_types::PrivateKeyDer::try_from(LEAF_KEY.to_vec()).unwrap();
        let signer = rustls::crypto::ring::sign::any_supported_type(&key).unwrap();
        let certified = Arc::new(rustls::sign::CertifiedKey::new(
            vec![rustls::pki_types::CertificateDer::from(LEAF.to_vec())],
            signer,
        ));
        let (sni_tx, sni_rx) = mpsc::channel();
        let config = rustls::ServerConfig::builder_with_provider(provider())
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(RecordSni { key: certified, sni: sni_tx }));
        let config = Arc::new(config);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            // The probe's first connection only times a handshake; the request is on the next.
            // When that handshake fails there is no next, and this thread waits out the test.
            let Ok((mut first, _)) = listener.accept() else { return };
            std::thread::sleep(delay);
            let mut timed = rustls::ServerConnection::new(config.clone()).unwrap();
            while timed.is_handshaking() {
                if timed.complete_io(&mut first).is_err() {
                    break;
                }
            }
            drop(first);
            let Ok((tcp, _)) = listener.accept() else { return };
            let conn = rustls::ServerConnection::new(config).unwrap();
            let mut tls = rustls::StreamOwned::new(conn, tcp);
            let mut heard = Heard::default();
            let mut reader = BufReader::new(&mut tls);
            let mut line = String::new();
            while reader.read_line(&mut line).map(|n| n > 0).unwrap_or(false) {
                let l = line.trim_end().to_string();
                line.clear();
                if l.is_empty() {
                    break;
                }
                if heard.request_line.is_empty() {
                    heard.request_line = l;
                } else if let Some((name, value)) = l.split_once(':') {
                    if name.eq_ignore_ascii_case("host") {
                        heard.host = Some(value.trim().to_string());
                    }
                }
            }
            // One name per handshake; both must have been the same.
            let names: Vec<_> = sni_rx.try_iter().collect();
            assert!(names.windows(2).all(|w| w[0] == w[1]), "{names:?}");
            heard.sni = names.into_iter().last().flatten();
            let _ = tls.write_all(response.as_bytes());
            let _ = tls.flush();
            tls.conn.send_close_notify();
            let _ = tls.conn.complete_io(&mut tls.sock);
            let _ = tx.send(heard);
        });
        (port, rx)
    }

    fn probe_local(host: &str, port: u16, mut opts: Probe) -> Result<Seen, ProbeError> {
        opts.port = port;
        probe(host, ip("127.0.0.1"), &opts)
    }

    #[test]
    fn the_probe_connects_to_the_address_and_sends_the_name_as_sni_and_host() {
        let (port, heard) = serve_once(
            "HTTP/1.1 200 OK\r\ncf-ray: a3f9c1183a075d86-FRA\r\ncontent-length: 20\r\n\
             connection: close\r\n\r\ncolo=FRA\nloc=IR\nx=1\n",
        );
        let seen = probe_local("edge.test", port, trusting_test_ca()).unwrap();
        assert_eq!(seen.colo, "FRA");
        assert_eq!(seen.via, Via::CfRay);

        let heard = heard.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(heard.sni.as_deref(), Some("edge.test"), "SNI");
        assert_eq!(heard.host, Some(format!("edge.test:{port}")), "Host");
        assert_eq!(heard.request_line, "GET /cdn-cgi/trace HTTP/1.1");
    }

    #[test]
    fn the_round_trip_is_the_tls_handshake_not_a_connect_something_local_could_answer() {
        // TCP is accepted at once, as a local TUN accepts it; only the handshake takes the time a
        // distant edge would.
        let (port, _) = serve_after(
            Duration::from_millis(250),
            "HTTP/1.1 200 OK\r\ncf-ray: a3f9c1183a075d86-FRA\r\ncontent-length: 0\r\n\r\n",
        );
        let seen = probe_local("edge.test", port, trusting_test_ca()).unwrap();
        assert!(seen.rtt_ms >= 250, "rtt {} ms", seen.rtt_ms);
    }

    #[test]
    fn an_error_status_from_cloudflare_still_names_its_data_center() {
        let (port, _) = serve_once(
            "HTTP/1.1 404 Not Found\r\ncf-ray: 8a1b2c3d4e5f6a7b-EWR\r\ncontent-length: 0\r\n\
             connection: close\r\n\r\n",
        );
        let seen = probe_local("edge.test", port, trusting_test_ca()).unwrap();
        assert_eq!((seen.colo.as_str(), seen.via), ("EWR", Via::CfRay));
    }

    #[test]
    fn without_a_cf_ray_the_trace_body_is_read() {
        let (port, _) = serve_once(
            "HTTP/1.1 200 OK\r\ncontent-length: 19\r\nconnection: close\r\n\r\nip=1.2.3.4\ncolo=AMS",
        );
        let seen = probe_local("edge.test", port, trusting_test_ca()).unwrap();
        assert_eq!((seen.colo.as_str(), seen.via), ("AMS", Via::Trace));
    }

    #[test]
    fn an_answer_with_neither_names_no_data_center() {
        let (port, _) = serve_once(
            "HTTP/1.1 200 OK\r\nserver: nginx\r\ncontent-length: 5\r\nconnection: close\r\n\r\nhello",
        );
        match probe_local("edge.test", port, trusting_test_ca()) {
            Err(ProbeError::NoColo(reason)) => assert!(reason.contains("200"), "{reason}"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_certificate_for_another_name_is_a_tls_failure() {
        let (port, _) = serve_once("HTTP/1.1 200 OK\r\ncf-ray: a3f9c1183a075d86-FRA\r\n\r\n");
        match probe_local("other.test", port, trusting_test_ca()) {
            Err(ProbeError::Tls(reason)) => assert!(!reason.is_empty()),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_certificate_from_an_untrusted_issuer_is_a_tls_failure() {
        // The bundled public roots, which have never heard of the test CA.
        let (port, _) = serve_once("HTTP/1.1 200 OK\r\ncf-ray: a3f9c1183a075d86-FRA\r\n\r\n");
        let opts = Probe { tls: None, ..trusting_test_ca() };
        assert!(matches!(probe_local("edge.test", port, opts), Err(ProbeError::Tls(_))));
    }

    #[test]
    fn an_edge_that_accepts_and_never_answers_times_out() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hold = std::thread::spawn(move || {
            // Accept the connection and keep it open, silent: the handshake never completes.
            let held = listener.accept();
            std::thread::sleep(Duration::from_secs(2));
            drop(held);
        });
        let opts = Probe { timeout: Duration::from_millis(300), ..trusting_test_ca() };
        assert_eq!(probe_local("edge.test", port, opts), Err(ProbeError::Timeout));
        let _ = hold.join();
    }

    #[test]
    fn an_edge_that_refuses_the_connection_is_a_failed_probe() {
        let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        assert!(matches!(probe_local("edge.test", port, trusting_test_ca()), Err(ProbeError::Failed(_))));
    }

    /// The measurement from the issue, against a real Cloudflare edge from whatever network runs
    /// it: `curl --resolve network.alinaderiparizi.com:443:104.21.77.84 …/cdn-cgi/trace`.
    #[test]
    #[ignore = "needs the internet"]
    fn live_edge_of_a_cloudflare_address() {
        let report = observe(
            Some("network.alinaderiparizi.com"),
            "104.21.77.84",
            SourceNetwork::default(),
            &EdgeCache::default(),
            &Probe::default(),
        );
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
        assert!(matches!(report.edge, Edge::Observed { .. }), "{:?}", report.edge);
    }
}
