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

const CA: &[u8] = include_bytes!("../../tests/fixtures/edge-tls/ca.der");
const LEAF: &[u8] = include_bytes!("../../tests/fixtures/edge-tls/leaf.der");
const LEAF_KEY: &[u8] = include_bytes!("../../tests/fixtures/edge-tls/leaf.key.der");

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
        // One connection: the timed handshake, then the request over it. A handshake that
        // fails ends it, and the reads below find nothing.
        let Ok((tcp, _)) = listener.accept() else { return };
        std::thread::sleep(delay);
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
