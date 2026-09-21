//! Fetching and parsing subscriptions.
//!
//! This lives in Rust rather than the webview for two reasons. A subscription URL is a credential —
//! anyone holding it can read every server you have — so it should never sit in a page's network
//! log or be reachable from injected script. And when a tunnel is up the request has to go through
//! it, which it does automatically here because the TUN carries the whole process.
//!
//! The response format is not standardised. In practice a subscription is a list of share links,
//! either as plain text or base64-encoded, and the interesting metadata arrives in a header.

use std::io::Read;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A subscription that returns more than this is misbehaving, and reading it all would let a
/// hostile endpoint exhaust memory.
const MAX_BODY: usize = 8 * 1024 * 1024;

const TIMEOUT: Duration = Duration::from_secs(30);

/// Sent so operators can tell clients apart in their logs. Deliberately not a browser string:
/// pretending to be Chrome would be a lie that helps nobody.
const USER_AGENT: &str = concat!("Nunya/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, thiserror::Error)]
pub enum SubscriptionError {
    #[error("{0}")]
    Rejected(String),
    #[error("could not reach the subscription: {0}")]
    Network(String),
    #[error("the subscription returned {status}")]
    Status { status: u16 },
    #[error("the subscription returned nothing that looks like a server list")]
    Empty,
}

/// Traffic allowance, from the `subscription-userinfo` response header.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Quota {
    pub used_bytes: i64,
    pub total_bytes: i64,
    /// Unix milliseconds, or `None` when the subscription does not say.
    pub resets_at: Option<i64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fetched {
    /// Share links, exactly as the subscription wrote them. Parsing into profiles happens in the
    /// frontend, which already does it for hand-pasted links and reports rejections per line.
    pub links: Vec<String>,
    pub quota: Option<Quota>,
    /// What the subscription calls itself, if it said.
    pub title: Option<String>,
}

/// Refuses anything that would leak the server list in transit.
///
/// A subscription response contains UUIDs, passwords and Reality keys — everything needed to
/// impersonate the user. Over plain HTTP all of it is readable by the network, which for this
/// client's users is frequently the adversary.
fn check_url(url: &str) -> Result<(), SubscriptionError> {
    let trimmed = url.trim();

    if trimmed.starts_with("http://") {
        return Err(SubscriptionError::Rejected(
            "This subscription uses plain HTTP, which would expose your server credentials to \
             anyone on the network. Ask your provider for an https:// link."
                .into(),
        ));
    }
    if !trimmed.starts_with("https://") {
        return Err(SubscriptionError::Rejected(
            "A subscription address must start with https://".into(),
        ));
    }
    Ok(())
}

/// Parses `subscription-userinfo: upload=1; download=2; total=3; expire=1700000000`.
fn parse_userinfo(header: &str) -> Quota {
    let mut quota = Quota::default();
    let mut upload = 0i64;
    let mut download = 0i64;

    for part in header.split(';') {
        let Some((key, value)) = part.split_once('=') else {
            continue;
        };
        let Ok(number) = value.trim().parse::<i64>() else {
            continue;
        };
        match key.trim() {
            "upload" => upload = number,
            "download" => download = number,
            "total" => quota.total_bytes = number,
            // Seconds since the epoch; the UI works in milliseconds.
            "expire" if number > 0 => quota.resets_at = Some(number * 1000),
            _ => {}
        }
    }

    // Panels report the two directions separately but bill their sum against the allowance.
    quota.used_bytes = upload.saturating_add(download);
    quota
}

/// Decodes a body that may or may not be base64.
///
/// Most panels base64 the whole list; some return it plain. Rather than guess from headers, which
/// are unreliable, this tries to decode and keeps whichever result actually contains share links.
fn decode_body(body: &str) -> String {
    let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    match base64_decode(&compact) {
        Some(bytes) => match String::from_utf8(bytes) {
            Ok(text) if text.contains("://") => text,
            _ => body.to_string(),
        },
        None => body.to_string(),
    }
}

/// Standard and URL-safe base64, tolerating missing padding.
///
/// Hand-rolled to avoid a dependency for forty lines: this runs on data from an untrusted endpoint,
/// and it is easier to be confident about code that cannot panic than about a crate's edge cases.
fn base64_decode(input: &str) -> Option<Vec<u8>> {
    if input.is_empty() {
        return None;
    }

    let value_of = |c: u8| -> Option<u8> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        })
    };

    let mut out = Vec::with_capacity(input.len() / 4 * 3);
    let mut buffer: u32 = 0;
    let mut bits = 0u32;

    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let value = value_of(byte)?;
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }

    (!out.is_empty()).then_some(out)
}

/// Pulls share links out of a decoded body.
///
/// Two formats are in circulation and nothing in the headers distinguishes them, so this goes by
/// content. The common one is a list of share links, one per line. The other is a JSON Xray
/// configuration — or an array of them — which is what a BPB panel serves to `?app=xray`: the same
/// servers, each wrapped in an entire client config of inbounds, routing and DNS, of which exactly
/// one outbound is the part worth keeping.
///
/// A body that parses as JSON never falls back to the line scan. JSON is never a list of share
/// links, and scanning it anyway finds the `://` inside a DNS address and reports a "server" made
/// of configuration fragments.
fn extract_links(body: &str) -> Vec<String> {
    let trimmed = body.trim_start();

    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        if let Some(links) = xray_config_links(trimmed) {
            return links;
        }
    }

    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && line.contains("://"))
        .map(str::to_string)
        .collect()
}

/// Rewrites one or more Xray configurations as share links.
///
/// `None` means the body was not JSON at all, which is the caller's signal to scan it by line.
///
/// Each configuration contributes the outbound tagged `proxy`, which is the convention every panel
/// emitting this format follows. One without such an outbound is a load balancer — BPB's
/// "Best Ping" entry carries `proxy-1` through `proxy-8` behind a `leastPing` selector — and is
/// skipped rather than flattened, because this client has no balancer and the servers behind one
/// are already listed individually elsewhere in the same array. Flattening would import each of
/// them twice.
///
/// Protocols this build cannot run are emitted anyway, under their own scheme. The frontend then
/// rejects them by name, exactly as it does for a hand-pasted link. Dropping them here would hand
/// the user a list quietly shorter than the one their provider published, with nothing to say why
/// half of it went missing.
fn xray_config_links(body: &str) -> Option<Vec<String>> {
    let parsed: Value = serde_json::from_str(body).ok()?;

    let configs: Vec<&Value> = match &parsed {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![&parsed],
        _ => return None,
    };

    let mut links = Vec::new();

    for config in configs {
        let Some(outbounds) = config.get("outbounds").and_then(Value::as_array) else {
            continue;
        };

        let remarks = config
            .get("remarks")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match carrier(outbounds) {
            Carrier::One(proxy) => {
                if let Some(link) = outbound_link(proxy, remarks) {
                    links.push(link);
                }
            }
            // Named rather than quietly reduced to its last hop. BPB's "WoW" entries are
            // WARP-over-WARP, and importing one as a single hop would give the user something
            // labelled WoW that is not.
            Carrier::Chain(exit) => {
                let mut link = format!("chain://{exit}");
                if !remarks.is_empty() {
                    link.push('#');
                    link.push_str(&percent_encode(remarks));
                }
                links.push(link);
            }
            Carrier::Balancer => {}
        }
    }

    Some(links)
}

/// What a configuration's outbounds add up to.
enum Carrier<'a> {
    /// One outbound carries the traffic.
    One(&'a Value),
    /// Outbounds dial through each other, with this endpoint as the far hop.
    Chain(String),
    /// Several interchangeable proxies behind a selector.
    Balancer,
}

/// Decides which outbound — if any — is the one this configuration is really offering.
///
/// Panels emit three shapes under one schema, and telling them apart by tag alone is not enough:
///
/// - the ordinary case, a single outbound tagged `proxy`;
/// - a chain, where one outbound names another in `sockopt.dialerProxy` so traffic traverses both.
///   This client runs one hop, so a chain is refused by name rather than silently flattened;
/// - a balancer, several interchangeable `proxy-N` outbounds behind a `leastPing` selector. Those
///   servers appear individually elsewhere in the same array, so importing the balancer too would
///   duplicate every one of them.
///
/// The balancer case turns on there being *more than one* candidate, not on the tag's spelling.
/// BPB's WARP subscription has a "Best Ping" entry holding exactly one `proxy-1`, which is a real
/// configuration and not a duplicate of anything — skipping it on the tag alone lost it.
fn carrier(outbounds: &[Value]) -> Carrier<'_> {
    let tag_of = |o: &Value| {
        o.get("tag")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };

    if let Some(outer) = outbounds.iter().find(|o| {
        o.pointer("/streamSettings/sockopt/dialerProxy")
            .and_then(Value::as_str)
            .is_some_and(|t| !t.is_empty())
    }) {
        let exit = outer
            .pointer("/settings/peers/0/endpoint")
            .or_else(|| outer.pointer("/settings/vnext/0/address"))
            .or_else(|| outer.pointer("/settings/servers/0/address"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        return Carrier::Chain(exit);
    }

    let candidates: Vec<&Value> = outbounds
        .iter()
        .filter(|o| {
            let tag = tag_of(o);
            tag == "proxy" || tag.starts_with("proxy-")
        })
        .collect();

    if let Some(exact) = candidates.iter().find(|o| tag_of(o) == "proxy") {
        return Carrier::One(exact);
    }
    match candidates.len() {
        1 => Carrier::One(candidates[0]),
        _ => Carrier::Balancer,
    }
}

/// A port that may have been written as a number or as a string.
fn port_of(peer: &Value) -> Option<i64> {
    let port = peer.get("port")?;
    port.as_i64()
        .or_else(|| port.as_str().and_then(|s| s.parse().ok()))
}

/// Turns one outbound into a share link.
///
/// The scheme is whatever the outbound calls its protocol, including protocols this build does not
/// run — deciding that is the frontend's job, and it explains a refusal better than a silent drop.
fn outbound_link(outbound: &Value, remarks: &str) -> Option<String> {
    let protocol = outbound.get("protocol").and_then(Value::as_str)?;

    // Where the address and the credential live depends on the protocol: VLESS and VMess use
    // `vnext[].users[]`, and everything password-based uses `servers[]`.
    let (address, port, credential) = match protocol {
        "vless" | "vmess" => {
            let peer = outbound.pointer("/settings/vnext/0")?;
            (
                peer.get("address").and_then(Value::as_str)?,
                port_of(peer)?,
                peer.pointer("/users/0/id").and_then(Value::as_str)?,
            )
        }
        // Everything password-based in Xray's schema keeps its peer under `servers`.
        "trojan" | "shadowsocks" | "socks" | "http" => {
            let peer = outbound.pointer("/settings/servers/0")?;
            (
                peer.get("address").and_then(Value::as_str)?,
                port_of(peer)?,
                peer.get("password")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
        }
        // WireGuard keeps its peer under `peers` and its identity in keys rather than a user
        // record, so it shares nothing with the two shapes above and is encoded on its own.
        "wireguard" => return wireguard_link(outbound, remarks),
        // A protocol whose shape this does not know. Returning `None` would drop the entry, and a
        // subscription made only of such entries would come back as "nothing that looks like a
        // server list" — which reads as a broken link rather than as an unsupported protocol. A
        // marker link under the protocol's own scheme is enough for the frontend to name it.
        _ => {
            let endpoint = outbound
                .pointer("/settings/servers/0/address")
                .or_else(|| outbound.pointer("/settings/peers/0/endpoint"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let mut link = format!("{protocol}://{endpoint}");
            if !remarks.is_empty() {
                link.push('#');
                link.push_str(&percent_encode(remarks));
            }
            return Some(link);
        }
    };

    let stream = outbound.get("streamSettings");
    let network = stream
        .and_then(|s| s.get("network"))
        .and_then(Value::as_str)
        .unwrap_or("tcp");
    let security = stream
        .and_then(|s| s.get("security"))
        .and_then(Value::as_str)
        .unwrap_or("none");

    let mut params: Vec<(&str, String)> = vec![
        ("type", network.to_string()),
        ("security", security.to_string()),
    ];

    let settings_at = |key: &str| stream.and_then(|s| s.get(key));
    let push_str = |params: &mut Vec<(&str, String)>, key: &'static str, from: Option<&Value>, field: &str| {
        if let Some(value) = from.and_then(|v| v.get(field)).and_then(Value::as_str) {
            params.push((key, value.to_string()));
        }
    };

    // The share-link field names are not always Xray's JSON ones, so each transport is spelled out
    // rather than mapped generically.
    match network {
        "ws" => {
            let ws = settings_at("wsSettings");
            push_str(&mut params, "path", ws, "path");
            push_str(&mut params, "host", ws, "host");
        }
        "httpupgrade" => {
            let hu = settings_at("httpupgradeSettings");
            push_str(&mut params, "path", hu, "path");
            push_str(&mut params, "host", hu, "host");
        }
        "grpc" => {
            push_str(&mut params, "serviceName", settings_at("grpcSettings"), "serviceName");
        }
        "http" | "h2" | "h3" => {
            let http = settings_at("httpSettings");
            push_str(&mut params, "path", http, "path");
            // HTTP/2 is the one transport whose host is a list rather than a string — the same
            // asymmetry config.rs has to handle on the way out.
            if let Some(first) = http
                .and_then(|v| v.get("host"))
                .and_then(Value::as_array)
                .and_then(|hosts| hosts.first())
                .and_then(Value::as_str)
            {
                params.push(("host", first.to_string()));
            } else {
                push_str(&mut params, "host", http, "host");
            }
        }
        _ => {}
    }

    // Reality keeps its settings under its own key rather than in `tlsSettings`.
    let tls = settings_at(if security == "reality" {
        "realitySettings"
    } else {
        "tlsSettings"
    });
    push_str(&mut params, "sni", tls, "serverName");
    push_str(&mut params, "fp", tls, "fingerprint");
    push_str(&mut params, "pbk", tls, "publicKey");
    push_str(&mut params, "sid", tls, "shortId");

    if let Some(alpn) = tls.and_then(|v| v.get("alpn")).and_then(Value::as_array) {
        let names: Vec<&str> = alpn.iter().filter_map(Value::as_str).collect();
        if !names.is_empty() {
            params.push(("alpn", names.join(",")));
        }
    }
    if tls.and_then(|v| v.get("allowInsecure")).and_then(Value::as_bool) == Some(true) {
        params.push(("allowInsecure", "1".to_string()));
    }

    // VLESS carries its flow on the user rather than the stream, and VMess its cipher and alterId.
    let user = outbound.pointer("/settings/vnext/0/users/0");
    if protocol == "vless" {
        push_str(&mut params, "flow", user, "flow");
    }
    if protocol == "vmess" {
        push_str(&mut params, "encryption", user, "security");
        if let Some(alter_id) = user.and_then(|u| u.get("alterId")).and_then(Value::as_i64) {
            params.push(("alterId", alter_id.to_string()));
        }
    }

    // An IPv6 literal needs brackets, or the port cannot be told apart from the address.
    let host = if address.contains(':') && !address.starts_with('[') {
        format!("[{address}]")
    } else {
        address.to_string()
    };

    let query: Vec<String> = params
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| format!("{key}={}", percent_encode(value)))
        .collect();

    let mut link = format!("{protocol}://{}@{host}:{port}", percent_encode(credential));
    if !query.is_empty() {
        link.push('?');
        link.push_str(&query.join("&"));
    }
    if !remarks.is_empty() {
        link.push('#');
        link.push_str(&percent_encode(remarks));
    }

    Some(link)
}

/// A WireGuard outbound as a `wireguard://` link.
///
/// The syntax is the one v2rayN and Hiddify already emit — private key as userinfo, peer key and
/// interface addresses as query parameters — so a link pasted from another client parses here too,
/// and this is not a private format invented for one panel.
///
/// `reserved` is Cloudflare WARP's client identifier. Xray writes it as three numbers; the link
/// form writes them comma-separated. Getting it wrong is silent — the server drops the handshake
/// rather than refusing it — which is why it is carried rather than dropped as an optimisation.
fn wireguard_link(outbound: &Value, remarks: &str) -> Option<String> {
    let settings = outbound.get("settings")?;
    let peer = settings.pointer("/peers/0")?;

    // Xray writes the peer as a single `host:port` string, which has to survive an IPv6 literal.
    let endpoint = peer.get("endpoint").and_then(Value::as_str)?;
    let (host, port) = endpoint.rsplit_once(':')?;

    let mut params: Vec<(&str, String)> = Vec::new();
    if let Some(key) = peer.get("publicKey").and_then(Value::as_str) {
        params.push(("publickey", key.to_string()));
    }
    if let Some(addresses) = settings.get("address").and_then(Value::as_array) {
        let list: Vec<&str> = addresses.iter().filter_map(Value::as_str).collect();
        if !list.is_empty() {
            params.push(("address", list.join(",")));
        }
    }
    if let Some(reserved) = settings.get("reserved").and_then(Value::as_array) {
        let bytes: Vec<String> = reserved
            .iter()
            .filter_map(Value::as_i64)
            .map(|b| b.to_string())
            .collect();
        if !bytes.is_empty() {
            params.push(("reserved", bytes.join(",")));
        }
    }
    if let Some(mtu) = settings.get("mtu").and_then(Value::as_i64) {
        params.push(("mtu", mtu.to_string()));
    }
    if let Some(keepalive) = peer.get("keepAlive").and_then(Value::as_i64) {
        params.push(("keepalive", keepalive.to_string()));
    }

    let secret = settings
        .get("secretKey")
        .and_then(Value::as_str)
        .unwrap_or_default();

    let query: Vec<String> = params
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| format!("{key}={}", percent_encode(value)))
        .collect();

    let mut link = format!("wireguard://{}@{host}:{port}", percent_encode(secret));
    if !query.is_empty() {
        link.push('?');
        link.push_str(&query.join("&"));
    }
    if !remarks.is_empty() {
        link.push('#');
        link.push_str(&percent_encode(remarks));
    }

    Some(link)
}

/// Percent-encodes everything outside RFC 3986's unreserved set.
///
/// Hand-rolled for the same reason the base64 decoder above is: it is a dozen lines against a
/// dependency. The escaping is not optional — a WebSocket path is routinely `/vl/abc?ed=2560`,
/// whose `?` would otherwise end the query it is written into, and remarks arrive full of emoji.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Fetches a subscription and returns its links and allowance.
///
/// Blocking: call it from `spawn_blocking`.
pub fn fetch(url: &str) -> Result<Fetched, SubscriptionError> {
    check_url(url)?;

    let agent = ureq::AgentBuilder::new()
        .timeout(TIMEOUT)
        .user_agent(USER_AGENT)
        .build();

    let response = match agent.get(url.trim()).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(status, _)) => return Err(SubscriptionError::Status { status }),
        Err(e) => return Err(SubscriptionError::Network(e.to_string())),
    };

    let quota = response
        .header("subscription-userinfo")
        .map(parse_userinfo);
    let title = response
        .header("profile-title")
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());

    // Capped read rather than into_string(), which would happily consume an unbounded body.
    let mut body = String::new();
    response
        .into_reader()
        .take(MAX_BODY as u64)
        .read_to_string(&mut body)
        .map_err(|e| SubscriptionError::Network(e.to_string()))?;

    assemble(quota, title, &body)
}

/// Turns a response into a `Fetched`.
///
/// Split from `fetch` so the whole parse path can be tested without a trusted certificate: the
/// network half needs a real server, this half needs only bytes.
fn assemble(
    quota: Option<Quota>,
    title: Option<String>,
    body: &str,
) -> Result<Fetched, SubscriptionError> {
    let links = extract_links(&decode_body(body));
    if links.is_empty() {
        return Err(SubscriptionError::Empty);
    }

    Ok(Fetched {
        links,
        quota,
        title,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Public lists are plain text, unencoded, and enormous — one on GitHub carries over twenty
    /// thousand links in 4.7 MB. Every line has to come through, and quickly.
    #[test]
    fn a_public_list_of_tens_of_thousands_of_plain_links_is_read_whole() {
        let body: String = (0..25_000)
            .map(|i| format!("vless://00000000-0000-4000-8000-{i:012}@h{i}.example.net:443?type=ws&security=tls#By list 🧬 {i}\n"))
            .collect();
        assert!(body.len() < MAX_BODY);
        let started = std::time::Instant::now();
        let fetched = assemble(None, None, &body).unwrap();
        assert_eq!(fetched.links.len(), 25_000);
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[test]
    fn plain_http_is_refused_because_it_leaks_credentials() {
        let err = check_url("http://example.net/sub").unwrap_err();
        assert!(err.to_string().contains("plain HTTP"), "{err}");
    }

    #[test]
    fn non_http_schemes_are_refused() {
        assert!(check_url("file:///etc/passwd").is_err());
        assert!(check_url("ftp://example.net").is_err());
        assert!(check_url("").is_err());
    }

    #[test]
    fn https_is_accepted() {
        check_url("https://example.net/sub").unwrap();
        check_url("  https://example.net/sub  ").unwrap();
    }

    #[test]
    fn userinfo_sums_both_directions_against_the_allowance() {
        let q = parse_userinfo("upload=100; download=900; total=5000; expire=1700000000");
        assert_eq!(q.used_bytes, 1000);
        assert_eq!(q.total_bytes, 5000);
        assert_eq!(q.resets_at, Some(1_700_000_000_000));
    }

    #[test]
    fn userinfo_tolerates_missing_and_junk_fields() {
        let q = parse_userinfo("download=42; total=; nonsense; expire=0");
        assert_eq!(q.used_bytes, 42);
        assert_eq!(q.total_bytes, 0);
        // An expire of 0 means "never", not "1970".
        assert_eq!(q.resets_at, None);
    }

    #[test]
    fn decodes_standard_base64() {
        // "vless://a\nvmess://b"
        let encoded = "dmxlc3M6Ly9hCnZtZXNzOi8vYg==";
        assert_eq!(decode_body(encoded), "vless://a\nvmess://b");
    }

    #[test]
    fn decodes_url_safe_base64_without_padding() {
        let encoded = "dmxlc3M6Ly9hCnZtZXNzOi8vYg";
        assert_eq!(decode_body(encoded), "vless://a\nvmess://b");
    }

    #[test]
    fn leaves_a_plain_list_alone() {
        let plain = "vless://a\nvmess://b";
        assert_eq!(decode_body(plain), plain);
    }

    /// A body that decodes as valid base64 but yields no links is not a subscription; keeping the
    /// original text gives the line-level parser a chance to report something useful.
    #[test]
    fn keeps_the_original_when_decoding_produces_no_links() {
        assert_eq!(decode_body("abcd"), "abcd");
    }

    #[test]
    fn extracts_links_and_skips_comments_and_blanks() {
        let body = "# my servers\n\nvless://a\n   \ntrojan://b\nnot a link\n";
        assert_eq!(extract_links(body), vec!["vless://a", "trojan://b"]);
    }

    /// The exact shape a panel returns: a base64 body, an allowance header and a title.
    #[test]
    fn assembles_a_real_subscription_response() {
        let links = [
            "vless://8f3c9d2e-4a17-4b8e-9c21-7de5f0a63b14@de4.example.net:443?security=reality#DE-4",
            "vless://8f3c9d2e-4a17-4b8e-9c21-7de5f0a63b15@nl1.example.net:443?security=tls#NL-1",
            "snell://nope@jp.example.net:8388",
            "# a comment line",
        ]
        .join("\n");

        // base64 of the above, as a panel would send it
        let body = encode_for_test(links.as_bytes());
        let quota = parse_userinfo(
            "upload=10737418240; download=79725330432; total=214748364800; expire=1790000000",
        );

        let fetched = assemble(Some(quota), Some("Example Nodes".into()), &body).unwrap();

        // The comment is dropped; the unsupported protocol is kept for the frontend to reject by
        // name, rather than disappearing silently here.
        assert_eq!(fetched.links.len(), 3);
        assert!(fetched.links[0].starts_with("vless://"));
        assert!(fetched.links[2].starts_with("snell://"));

        let q = fetched.quota.unwrap();
        assert_eq!(q.used_bytes, 90_462_748_672);
        assert_eq!(q.total_bytes, 214_748_364_800);
        assert_eq!(q.resets_at, Some(1_790_000_000_000));
        assert_eq!(fetched.title.as_deref(), Some("Example Nodes"));
    }

    #[test]
    fn a_body_with_no_links_is_an_error_rather_than_an_empty_list() {
        let err = assemble(None, None, "<html>login required</html>").unwrap_err();
        assert!(matches!(err, SubscriptionError::Empty));
    }

    /// Minimal encoder, so the test above can build its own fixture.
    fn encode_for_test(input: &[u8]) -> String {
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut out = String::new();
        for chunk in input.chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            for i in 0..4 {
                if i <= chunk.len() {
                    out.push(ALPHABET[((n >> (18 - i * 6)) & 0x3F) as usize] as char);
                } else {
                    out.push('=');
                }
            }
        }
        out
    }

    #[test]
    fn base64_rejects_invalid_alphabet() {
        assert!(base64_decode("!!!!").is_none());
        assert!(base64_decode("").is_none());
    }

    /// The shape a BPB panel serves to `?app=xray`: an array of whole client configurations, each
    /// carrying one `proxy` outbound. Trimmed of the inbounds, routing and DNS that come with it
    /// and are not read here.
    fn bpb_array() -> &'static str {
        r#"[
          {
            "remarks": "💦 1. VLESS - Domain : 443",
            "outbounds": [
              {
                "protocol": "vless",
                "settings": {
                  "vnext": [{
                    "address": "edge.example.net",
                    "port": 443,
                    "users": [{ "id": "455c35ab-42b9-43d7-b63c-ea915c2c72ab", "encryption": "none" }]
                  }]
                },
                "streamSettings": {
                  "network": "ws",
                  "wsSettings": { "host": "edge.example.net", "path": "/vl/8NjpyzwBr4ARYZEeGqW?ed=2560" },
                  "security": "tls",
                  "tlsSettings": { "serverName": "edge.example.net", "fingerprint": "chrome", "alpn": ["http/1.1"] }
                },
                "tag": "proxy"
              },
              { "protocol": "freedom", "tag": "direct" }
            ]
          },
          {
            "remarks": "💦 1. Trojan - Domain : 443",
            "outbounds": [
              {
                "protocol": "trojan",
                "settings": { "servers": [{ "address": "edge.example.net", "port": 443, "password": "E1wy;,GYGPPhYEYXn" }] },
                "streamSettings": {
                  "network": "ws",
                  "wsSettings": { "host": "edge.example.net", "path": "/tr/FHxj7oXIlRMTvceFHCN?ed=2560" },
                  "security": "tls",
                  "tlsSettings": { "serverName": "edge.example.net", "fingerprint": "chrome" }
                },
                "tag": "proxy"
              }
            ]
          },
          {
            "remarks": "💦 Best Ping",
            "outbounds": [
              { "protocol": "vless", "tag": "proxy-1" },
              { "protocol": "trojan", "tag": "proxy-2" }
            ],
            "routing": { "balancers": [{ "tag": "all-proxies", "selector": ["proxy"] }] }
          }
        ]"#
    }

    #[test]
    fn an_xray_configuration_array_becomes_share_links() {
        let links = extract_links(bpb_array());
        assert_eq!(links.len(), 2, "one per configuration with a proxy outbound");

        let vless = &links[0];
        assert!(vless.starts_with("vless://455c35ab-42b9-43d7-b63c-ea915c2c72ab@edge.example.net:443?"));
        assert!(vless.contains("type=ws"));
        assert!(vless.contains("security=tls"));
        assert!(vless.contains("host=edge.example.net"));
        assert!(vless.contains("sni=edge.example.net"));
        assert!(vless.contains("fp=chrome"));
        assert!(vless.contains("alpn=http%2F1.1"));
    }

    /// The path carries a query string of its own, which has to survive being written into one.
    #[test]
    fn a_websocket_path_is_escaped_rather_than_ending_the_query() {
        let links = extract_links(bpb_array());
        assert!(links[0].contains("path=%2Fvl%2F8NjpyzwBr4ARYZEeGqW%3Fed%3D2560"));
    }

    /// Emitted, not dropped. The frontend answers "Trojan is not supported yet"; a subscription
    /// that silently lost half its entries could not.
    #[test]
    fn a_protocol_this_build_cannot_run_is_still_emitted_under_its_own_scheme() {
        let links = extract_links(bpb_array());
        let trojan = &links[1];
        assert!(trojan.starts_with("trojan://"));
        // The password is userinfo, so its punctuation has to be escaped.
        assert!(trojan.contains("E1wy%3B%2CGYGPPhYEYXn@edge.example.net:443"));
    }

    /// A chain traverses two hops. Reducing it to its last one would hand back something the
    /// entry's own name contradicts, so it is refused instead.
    #[test]
    fn a_chained_configuration_is_refused_rather_than_flattened() {
        let body = r#"[{
          "remarks": "💦 1 - WoW",
          "outbounds": [
            { "protocol": "wireguard", "tag": "chain",
              "streamSettings": { "sockopt": { "dialerProxy": "proxy" } },
              "settings": { "peers": [{ "endpoint": "162.159.192.1:2408" }] } },
            { "protocol": "wireguard", "tag": "proxy",
              "settings": { "peers": [{ "endpoint": "engage.cloudflareclient.com:2408" }] } }
          ]
        }]"#;
        let links = extract_links(body);
        assert_eq!(links.len(), 1);
        assert!(links[0].starts_with("chain://162.159.192.1:2408#"), "got {}", links[0]);
    }

    /// A "Best Ping" entry holding exactly one proxy is a real configuration, not a duplicate of
    /// anything, so the balancer rule must turn on the number of candidates rather than on the
    /// tag's spelling. Skipping it on the tag alone lost a config the provider published.
    #[test]
    fn a_selector_over_a_single_proxy_is_still_that_proxy() {
        let body = r#"[{
          "remarks": "Best Ping",
          "outbounds": [{ "protocol": "wireguard", "tag": "proxy-1",
            "settings": { "peers": [{ "endpoint": "engage.cloudflareclient.com:2408", "publicKey": "k" }],
                          "address": ["172.16.0.2/32"], "secretKey": "s" } }],
          "routing": { "balancers": [{ "tag": "all", "selector": ["proxy"] }] }
        }]"#;
        let links = extract_links(body);
        assert_eq!(links.len(), 1, "got {links:?}");
        assert!(links[0].starts_with("wireguard://s@engage.cloudflareclient.com:2408?"));
    }

    /// A balancer has no `proxy` outbound, and the servers behind it are listed individually in
    /// the same array — importing them again through the balancer would duplicate every one.
    #[test]
    fn a_balancer_configuration_contributes_nothing() {
        let links = extract_links(bpb_array());
        assert!(
            !links.iter().any(|l| l.contains("Best")),
            "the balancer entry should be skipped, got {links:?}"
        );
    }

    #[test]
    fn remarks_become_the_name_and_survive_their_emoji() {
        let links = extract_links(bpb_array());
        assert!(links[0].ends_with("#%F0%9F%92%A6%201.%20VLESS%20-%20Domain%20%3A%20443"));
    }

    /// The other thing a panel may serve at a subscription path: its own settings object. It has
    /// no outbounds, and the `://` inside its DNS addresses must not be read as a server.
    #[test]
    fn json_that_holds_no_outbounds_yields_nothing_rather_than_fragments() {
        let body = r#"{"remoteDNS":"https://8.8.8.8/dns-query","localDNS":"8.8.8.8"}"#;
        assert!(extract_links(body).is_empty());
    }

    #[test]
    fn a_plain_list_of_share_links_is_untouched() {
        let body = "vless://uuid@example.net:443?type=ws#One\n# a comment\n\nvmess://blob\n";
        assert_eq!(
            extract_links(body),
            vec!["vless://uuid@example.net:443?type=ws#One", "vmess://blob"]
        );
    }

    #[test]
    fn an_ipv6_address_keeps_its_brackets_so_the_port_is_readable() {
        let body = r#"{"outbounds":[{"protocol":"vless","tag":"proxy","settings":{"vnext":[{"address":"2001:db8::1","port":443,"users":[{"id":"u"}]}]}}]}"#;
        assert_eq!(extract_links(body), vec!["vless://u@[2001:db8::1]:443?type=tcp&security=none"]);
    }

    /// A WARP subscription is entirely WireGuard, which keeps its peer under `peers` and its
    /// identity in keys rather than a user record — so it shares no field with the other
    /// protocols and once fell through the address lookup and vanished entirely.
    #[test]
    fn a_wireguard_outbound_becomes_a_link_carrying_its_keys() {
        let body = r#"[{
          "remarks": "💦 1 - Warp",
          "outbounds": [{
            "protocol": "wireguard",
            "tag": "proxy",
            "settings": {
              "address": ["172.16.0.2/32", "2606:4700:110::1/128"],
              "mtu": 1280,
              "peers": [{
                "endpoint": "engage.cloudflareclient.com:2408",
                "publicKey": "bmXOC+F1FxEMF9dyiK2H5/1SUtzH0JuVo51h2wPfgyo=",
                "keepAlive": 5
              }],
              "reserved": [216, 253, 3],
              "secretKey": "f7m/C8NHWPWIkGAbxTBAMhYHlzu3Ya7lSCbOSqfGu68="
            }
          }]
        }]"#;
        let links = extract_links(body);
        assert_eq!(links.len(), 1);
        let link = &links[0];

        // The private key is userinfo, so its `/` and `+` have to be escaped.
        assert!(
            link.starts_with("wireguard://f7m%2FC8NHWPWIkGAbxTBAMhYHlzu3Ya7lSCbOSqfGu68%3D@engage.cloudflareclient.com:2408?"),
            "got {link}"
        );
        assert!(link.contains("publickey=bmXOC%2BF1FxEMF9dyiK2H5%2F1SUtzH0JuVo51h2wPfgyo%3D"));
        assert!(link.contains("address=172.16.0.2%2F32%2C2606%3A4700%3A110%3A%3A1%2F128"));
        // WARP's client id. A wrong value is dropped by the server without an error, so losing
        // it here would produce a tunnel that comes up and carries nothing.
        assert!(link.contains("reserved=216%2C253%2C3"));
        assert!(link.contains("mtu=1280"));
        assert!(link.contains("keepalive=5"));
    }

    /// An endpoint whose shape is unknown still has to reach the frontend to be refused by name,
    /// or a subscription made only of them looks like a broken link.
    #[test]
    fn a_protocol_with_an_unfamiliar_shape_is_named_rather_than_dropped() {
        let body = r#"[{
          "remarks": "Something new",
          "outbounds": [{ "protocol": "hysteria2", "tag": "proxy",
            "settings": { "servers": [{ "address": "h2.example.net", "port": 443 }] } }]
        }]"#;
        let links = extract_links(body);
        assert_eq!(links.len(), 1);
        assert!(links[0].starts_with("hysteria2://h2.example.net#"), "got {}", links[0]);
    }

    #[test]
    fn reality_keys_are_read_from_their_own_settings_object() {
        let body = r#"{"outbounds":[{"protocol":"vless","tag":"proxy",
          "settings":{"vnext":[{"address":"a.example.net","port":443,"users":[{"id":"u","flow":"xtls-rprx-vision"}]}]},
          "streamSettings":{"network":"tcp","security":"reality",
            "realitySettings":{"serverName":"www.example.net","publicKey":"PBK","shortId":"ab","fingerprint":"chrome"}}}]}"#;
        let link = &extract_links(body)[0];
        assert!(link.contains("security=reality"));
        assert!(link.contains("pbk=PBK"));
        assert!(link.contains("sid=ab"));
        assert!(link.contains("flow=xtls-rprx-vision"));
    }
}
