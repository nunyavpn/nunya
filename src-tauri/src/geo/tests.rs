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
