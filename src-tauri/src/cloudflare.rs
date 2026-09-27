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
use std::io::{Read, Write};
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
    /// Trust roots other than the bundled public ones (`webpki-roots`).
    pub tls: Option<Arc<rustls::ClientConfig>>,
}

impl Default for Probe {
    fn default() -> Self {
        Probe { timeout: Duration::from_secs(5), port: 443, tls: None }
    }
}

/// Asks the edge at `ip` which data center it is, sending `host` as SNI and `Host`.
///
/// Direct, over whatever route the OS has, and never through a proxy: the question is which edge
/// *this network* reaches, which is also the edge the core reaches when it dials the config.
///
/// `curl --resolve`, by hand: the request goes over the connection whose handshake was timed. An
/// HTTP client would need that address pinned through its resolver, and it would add nothing for
/// one GET whose answer is a header or a `colo=` line.
pub fn probe(host: &str, ip: IpAddr, opts: &Probe) -> Result<Seen, ProbeError> {
    let addr = SocketAddr::new(ip, opts.port);
    let tls = opts.tls.clone().unwrap_or_else(public_roots);

    let (mut stream, rtt_ms) = handshake(host, addr, tls, opts.timeout)?;

    let authority = if opts.port == 443 { host.to_string() } else { format!("{host}:{}", opts.port) };
    let request = format!(
        "GET /cdn-cgi/trace HTTP/1.1\r\nHost: {authority}\r\nUser-Agent: Nunya/{}\r\n\
         Accept: */*\r\nConnection: close\r\n\r\n",
        env!("CARGO_PKG_VERSION")
    );
    stream.write_all(request.as_bytes()).map_err(|e| io_error(&e))?;

    let mut raw = Vec::new();
    let read = (&mut stream).take(BODY_LIMIT).read_to_end(&mut raw);
    // An edge that closes without a TLS close_notify is an error to rustls, after the answer has
    // arrived; what was read still counts, and only an empty read is a failure.
    if raw.is_empty() {
        return Err(match read {
            Err(e) => io_error(&e),
            Ok(_) => ProbeError::Failed("the edge closed without answering".into()),
        });
    }

    let answer = String::from_utf8_lossy(&raw);
    let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((&answer, ""));
    let mut lines = head.lines();
    // A 404 or a 403 from Cloudflare still names the data center that sent it.
    let status = lines.next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("nothing");
    let ray = lines
        .filter_map(|l| l.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("cf-ray"))
        .and_then(|(_, value)| colo_from_ray(value));

    if let Some(colo) = ray {
        return Ok(Seen { colo, via: Via::CfRay, rtt_ms });
    }
    match colo_from_trace(body) {
        Some(colo) => Ok(Seen { colo, via: Via::Trace, rtt_ms }),
        None => Err(ProbeError::NoColo(format!(
            "answered {status} with no CF-Ray header and no trace"
        ))),
    }
}

/// The bundled public roots (`webpki-roots`), made once.
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
/// The request then goes over the same connection; the time is taken before it, so it is the
/// handshake's alone. Its failures are the probe's: a certificate refused here is a TLS failure,
/// reported before anything is asked over the connection.
fn handshake(
    host: &str,
    addr: SocketAddr,
    tls: Arc<rustls::ClientConfig>,
    timeout: Duration,
) -> Result<(rustls::StreamOwned<rustls::ClientConnection, TcpStream>, u32), ProbeError> {
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
    let rtt_ms = u32::try_from(started.elapsed().as_millis()).unwrap_or(u32::MAX);
    Ok((rustls::StreamOwned::new(conn, tcp), rtt_ms))
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
mod tests;
