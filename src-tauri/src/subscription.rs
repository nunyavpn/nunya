//! Fetching and parsing subscriptions.
//!
//! This lives in Rust rather than the webview for two reasons. A subscription URL is a credential —
//! anyone holding it can read every server you have — so it should never sit in a page's network
//! log or be reachable from injected script. And when a tunnel is up the request has to go through
//! it, which it does automatically here because the TUN carries the whole process.
//!
//! The response format is not standardised. In practice a subscription is a list of share links,
//! either as plain text or base64-encoded, and the interesting metadata arrives in a header. A panel
//! asked for a particular app — BPB's `?app=xray`, `?app=sing-box`, `?app=clash` — serves that app's
//! whole configuration instead, and `extract_links` reads each of the three back into share links.
//!
//! The address itself arrives in more than one form too: a panel's "import to sing-box" button
//! copies a `sing-box://import-remote-profile?url=…` link wrapping the real one; see `resolve`.

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

/// Links that hand a subscription address to one particular app.
///
/// Panels offer them as "import to sing-box" and "import to Clash" buttons, and that button is what
/// users copy — so refusing the wrapper would turn away a working subscription over its envelope.
/// The wrapper is kept as the group's address and unwrapped on every fetch, which keeps this the one
/// parser for it rather than one here and another in the frontend.
const IMPORT_LINKS: &[&str] = &[
    "sing-box://import-remote-profile",
    "clash://install-config",
    "clashmeta://install-config",
];

/// The address to fetch: the URL itself, or the one an import link carries in `url=`.
///
/// sing-box's scheme says the carried address is percent-encoded, and BPB writes it raw —
/// `?url=https://host/sub/normal?app=sing-box#name` — so both are read. A raw address runs to the
/// end of the link, because any `&` in it belongs to its own query. The fragment is a display name
/// in every form, never part of the address, and is left off the request.
fn resolve(url: &str) -> Result<String, SubscriptionError> {
    let trimmed = url.trim();
    let carried = IMPORT_LINKS.iter().find_map(|prefix| {
        trimmed
            .get(..prefix.len())
            .filter(|head| head.eq_ignore_ascii_case(prefix))
            .map(|_| &trimmed[prefix.len()..])
    });

    let address = match carried {
        None => trimmed.to_string(),
        Some(rest) => {
            let query = rest.split('#').next().unwrap_or_default();
            let query = query.trim_start_matches('/').trim_start_matches('?');
            let value = query
                .strip_prefix("url=")
                .or_else(|| query.find("&url=").map(|at| &query[at + 5..]))
                .ok_or_else(|| {
                    SubscriptionError::Rejected(
                        "This import link carries no subscription address (url=…).".into(),
                    )
                })?;
            let first = value.split('&').next().unwrap_or_default();
            if first.contains("://") {
                value.to_string()
            } else {
                percent_decode(first).ok_or_else(|| {
                    SubscriptionError::Rejected(
                        "The address inside this import link is not validly encoded.".into(),
                    )
                })?
            }
        }
    };

    check_url(&address)?;
    Ok(address.split('#').next().unwrap_or_default().trim().to_string())
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
/// Two kinds of body are in circulation and nothing in the headers distinguishes them, so this goes
/// by content. The common one is a list of share links, one per line. The other is a client's whole
/// JSON configuration — what a panel serves when the link names an app — of which the servers are
/// the only part worth keeping; see `config_links`.
///
/// A body that parses as JSON never falls back to the line scan. JSON is never a list of share
/// links, and scanning it anyway finds the `://` inside a DNS address and reports a "server" made
/// of configuration fragments.
fn extract_links(body: &str) -> Vec<String> {
    let trimmed = body.trim_start();

    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
            return match &parsed {
                Value::Array(configs) => configs.iter().flat_map(config_links).collect(),
                _ => config_links(&parsed),
            };
        }
    }

    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && line.contains("://"))
        .map(str::to_string)
        .collect()
}

/// Rewrites one client configuration as share links.
///
/// Which client it was written for is read from its shape, because nothing else says: Clash lists
/// its servers under `proxies`; sing-box and Xray both use `outbounds`, but sing-box names each
/// one's protocol `type` and Xray `protocol`. sing-box has also moved WireGuard out to `endpoints`.
///
/// Protocols this build cannot run are emitted anyway, under their own scheme. The frontend then
/// rejects them by name, exactly as it does for a hand-pasted link. Dropping them here would hand
/// the user a list quietly shorter than the one their provider published, with nothing to say why
/// half of it went missing.
fn config_links(config: &Value) -> Vec<String> {
    if let Some(proxies) = config.get("proxies").and_then(Value::as_array) {
        return proxies.iter().filter_map(clash_link).collect();
    }

    let list = |key: &str| {
        config
            .get(key)
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
    };
    let (outbounds, endpoints) = (list("outbounds"), list("endpoints"));
    if outbounds.iter().chain(endpoints).any(|o| o.get("type").is_some()) {
        return outbounds
            .iter()
            .chain(endpoints)
            .filter_map(singbox_link)
            .collect();
    }

    xray_config_link(config).into_iter().collect()
}

// ------------------------------------------------------------------------------------ the writer

/// One server as every configuration format describes it: what the readers fill in and the one
/// link writer reads.
///
/// One writer is the point. Xray, sing-box and Clash spell the same server three ways, and links
/// written from each by its own code would drift apart — one subscription importing differently
/// depending on which of the panel's buttons the user happened to copy. Empty means absent.
#[derive(Default)]
struct Node {
    /// The share-link scheme, which is not always the configuration's word: `ss`, not
    /// `shadowsocks`. See `scheme_of`.
    scheme: String,
    /// The UUID for VLESS and VMess, the password for everything password-based.
    credential: String,
    address: String,
    port: i64,
    network: String,
    header_type: String,
    security: String,
    /// The WebSocket path carries early data as `?ed=N`, the way every panel's links write it.
    path: String,
    host: String,
    service_name: String,
    sni: String,
    fingerprint: String,
    public_key: String,
    short_id: String,
    alpn: Vec<String>,
    insecure: bool,
    /// VLESS only.
    flow: String,
    /// VMess only.
    cipher: String,
    alter_id: Option<i64>,
    name: String,
}

impl Node {
    /// The `scheme://credential@host:port?query#name` link the frontend parses.
    fn link(&self) -> String {
        let mut params: Vec<(&str, String)> = vec![
            ("type", self.network.clone()),
            ("security", self.security.clone()),
            ("headerType", self.header_type.clone()),
            ("path", self.path.clone()),
            ("host", self.host.clone()),
            ("serviceName", self.service_name.clone()),
            ("sni", self.sni.clone()),
            ("fp", self.fingerprint.clone()),
            ("pbk", self.public_key.clone()),
            ("sid", self.short_id.clone()),
            ("alpn", self.alpn.join(",")),
        ];
        if self.insecure {
            params.push(("allowInsecure", "1".to_string()));
        }
        params.push(("flow", self.flow.clone()));
        params.push(("encryption", self.cipher.clone()));
        if let Some(alter_id) = self.alter_id {
            params.push(("alterId", alter_id.to_string()));
        }

        let link = format!(
            "{}://{}@{}:{}",
            self.scheme,
            percent_encode(&self.credential),
            bracketed(&self.address),
            self.port
        );
        named(with_query(link, &params), &self.name)
    }
}

/// A WireGuard server, which has keys and interface addresses where the others have a user and a
/// transport, and so shares nothing with `Node` but the address.
///
/// The link syntax is the one v2rayN and Hiddify already emit — private key as userinfo, peer key
/// and interface addresses as query parameters — so a link pasted from another client parses here
/// too, and this is not a private format invented for one panel.
///
/// `reserved` is Cloudflare WARP's client identifier. Getting it wrong is silent — the server drops
/// the handshake rather than refusing it — which is why it is carried rather than dropped as an
/// optimisation. It is kept as text: numbers comma-separated, or the base64 some exports use, both
/// of which the frontend reads.
#[derive(Default)]
struct WireGuard {
    secret: String,
    /// Bracketed already when it is an IPv6 literal.
    host: String,
    port: i64,
    public_key: String,
    addresses: Vec<String>,
    reserved: String,
    mtu: Option<i64>,
    keepalive: Option<i64>,
    name: String,
}

impl WireGuard {
    fn link(&self) -> String {
        let mut params: Vec<(&str, String)> = vec![
            ("publickey", self.public_key.clone()),
            ("address", self.addresses.join(",")),
            ("reserved", self.reserved.clone()),
        ];
        if let Some(mtu) = self.mtu {
            params.push(("mtu", mtu.to_string()));
        }
        if let Some(keepalive) = self.keepalive {
            params.push(("keepalive", keepalive.to_string()));
        }

        let link = format!(
            "wireguard://{}@{}:{}",
            percent_encode(&self.secret),
            self.host,
            self.port
        );
        named(with_query(link, &params), &self.name)
    }
}

/// A link that is only a scheme and an endpoint: enough for the frontend to refuse the entry by
/// name. Used for chains (`chain://`) and for protocols whose shape is not known here.
fn marker(scheme: &str, endpoint: &str, name: &str) -> String {
    named(format!("{scheme}://{endpoint}"), name)
}

/// The share-link scheme for a configuration's protocol name, where the two differ.
fn scheme_of(protocol: &str) -> &str {
    match protocol {
        "shadowsocks" => "ss",
        other => other,
    }
}

fn with_query(mut link: String, params: &[(&str, String)]) -> String {
    let query: Vec<String> = params
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| format!("{key}={}", percent_encode(value)))
        .collect();
    if !query.is_empty() {
        link.push('?');
        link.push_str(&query.join("&"));
    }
    link
}

fn named(mut link: String, name: &str) -> String {
    if !name.is_empty() {
        link.push('#');
        link.push_str(&percent_encode(name));
    }
    link
}

/// An IPv6 literal needs brackets, or the port cannot be told apart from the address.
fn bracketed(address: &str) -> String {
    if address.contains(':') && !address.starts_with('[') {
        format!("[{address}]")
    } else {
        address.to_string()
    }
}

/// A string field, or empty.
fn text(from: Option<&Value>, field: &str) -> String {
    from.and_then(|v| v.get(field))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The first of two readings that has anything in it — for fields formats spell two ways.
fn either(first: String, second: String) -> String {
    if first.is_empty() {
        second
    } else {
        first
    }
}

/// A number that may have been written as a number or as a string — ports especially.
fn number(from: Option<&Value>, field: &str) -> Option<i64> {
    let value = from?.get(field)?;
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.trim().parse().ok()))
}

/// A list of strings that may also have been written as one string.
fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
        Some(Value::String(one)) if !one.is_empty() => vec![one.clone()],
        _ => Vec::new(),
    }
}

/// WARP's client id: three numbers, or text already in link form.
fn reserved_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::Array(bytes)) => bytes
            .iter()
            .filter_map(Value::as_i64)
            .map(|b| b.to_string())
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::String(text)) => text.clone(),
        _ => String::new(),
    }
}

/// The WebSocket path with early data written back into it as `?ed=N`, the form links carry.
///
/// sing-box and Clash hold early data as two fields beside the path. The header name is the one
/// Xray always uses unless it says otherwise, and `eh` is how a link says otherwise. With no header
/// at all the early data would go in the path itself, which a link cannot express — so none is
/// written, and the connection does without the optimisation rather than failing.
fn early_data_path(path: String, max_early_data: Option<i64>, header: &str) -> String {
    match max_early_data {
        Some(ed) if ed > 0 && header == "Sec-WebSocket-Protocol" => format!("{path}?ed={ed}"),
        Some(ed) if ed > 0 && !header.is_empty() => {
            format!("{path}?ed={ed}&eh={}", percent_encode(header))
        }
        _ => path,
    }
}

// -------------------------------------------------------------------------------------- xray

/// Rewrites one Xray configuration as a share link.
///
/// BPB serves `?app=xray` as an array of these, one server each, wrapped in an entire client
/// config of inbounds, routing and DNS. The server is the outbound tagged `proxy`, which is the
/// convention every panel emitting this format follows. One without such an outbound is a load
/// balancer — BPB's "Best Ping" entry carries `proxy-1` through `proxy-8` behind a `leastPing`
/// selector — and is skipped rather than flattened, because this client has no balancer and the
/// servers behind one are already listed individually elsewhere in the same array. Flattening
/// would import each of them twice.
fn xray_config_link(config: &Value) -> Option<String> {
    let outbounds = config.get("outbounds").and_then(Value::as_array)?;
    let remarks = text(Some(config), "remarks");

    match carrier(outbounds) {
        Carrier::One(proxy) => xray_link(proxy, &remarks),
        // Named rather than quietly reduced to its last hop. BPB's "WoW" entries are
        // WARP-over-WARP, and importing one as a single hop would give the user something
        // labelled WoW that is not.
        Carrier::Chain(exit) => Some(marker("chain", &exit, &remarks)),
        Carrier::Balancer => None,
    }
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
    let tag_of = |o: &Value| text(Some(o), "tag");

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

/// Turns one Xray outbound into a share link.
fn xray_link(outbound: &Value, remarks: &str) -> Option<String> {
    let protocol = outbound.get("protocol").and_then(Value::as_str)?;

    // Where the address and the credential live depends on the protocol: VLESS and VMess use
    // `vnext[].users[]`, and everything password-based uses `servers[]`.
    let (peer, credential) = match protocol {
        "vless" | "vmess" => {
            let peer = outbound.pointer("/settings/vnext/0")?;
            (peer, peer.pointer("/users/0/id").and_then(Value::as_str)?)
        }
        "trojan" | "shadowsocks" | "socks" | "http" => {
            let peer = outbound.pointer("/settings/servers/0")?;
            (
                peer,
                peer.get("password")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
        }
        // WireGuard keeps its peer under `peers` and its identity in keys rather than a user
        // record, so it shares nothing with the two shapes above and is read on its own.
        "wireguard" => return xray_wireguard(outbound, remarks),
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
            return Some(marker(scheme_of(protocol), endpoint, remarks));
        }
    };

    let stream = outbound.get("streamSettings");
    let network = text(stream, "network");
    let security = text(stream, "security");
    let at = |key: &str| stream.and_then(|s| s.get(key));

    let mut node = Node {
        scheme: scheme_of(protocol).to_string(),
        credential: credential.to_string(),
        address: peer.get("address").and_then(Value::as_str)?.to_string(),
        port: number(Some(peer), "port")?,
        network: if network.is_empty() { "tcp".into() } else { network },
        security: if security.is_empty() { "none".into() } else { security },
        name: remarks.to_string(),
        ..Node::default()
    };

    // The share-link field names are not always Xray's JSON ones, so each transport is spelled out
    // rather than mapped generically. Xray already writes early data into the WebSocket path.
    match node.network.as_str() {
        "ws" => {
            let ws = at("wsSettings");
            node.path = text(ws, "path");
            node.host = text(ws, "host");
            if node.host.is_empty() {
                node.host = text(ws.and_then(|w| w.get("headers")), "Host");
            }
        }
        "httpupgrade" => {
            let hu = at("httpupgradeSettings");
            node.path = text(hu, "path");
            node.host = text(hu, "host");
        }
        "grpc" => node.service_name = text(at("grpcSettings"), "serviceName"),
        "http" | "h2" | "h3" => {
            let http = at("httpSettings");
            node.path = text(http, "path");
            // HTTP/2 is the one transport whose host is a list rather than a string — the same
            // asymmetry config.rs has to handle on the way out.
            node.host = strings(http.and_then(|h| h.get("host")))
                .into_iter()
                .next()
                .unwrap_or_default();
        }
        _ => {}
    }

    // Reality keeps its settings under its own key rather than in `tlsSettings`.
    let tls = at(if node.security == "reality" {
        "realitySettings"
    } else {
        "tlsSettings"
    });
    node.sni = text(tls, "serverName");
    node.fingerprint = text(tls, "fingerprint");
    node.public_key = text(tls, "publicKey");
    node.short_id = text(tls, "shortId");
    node.alpn = strings(tls.and_then(|t| t.get("alpn")));
    node.insecure = tls.and_then(|t| t.get("allowInsecure")).and_then(Value::as_bool) == Some(true);

    // VLESS carries its flow on the user rather than the stream, and VMess its cipher and alterId.
    let user = outbound.pointer("/settings/vnext/0/users/0");
    if protocol == "vless" {
        node.flow = text(user, "flow");
    }
    if protocol == "vmess" {
        node.cipher = text(user, "security");
        node.alter_id = user.and_then(|u| u.get("alterId")).and_then(Value::as_i64);
    }

    Some(node.link())
}

/// A WireGuard outbound in Xray's shape: the peer as one `host:port` string, which has to survive
/// an IPv6 literal, and `reserved` as three numbers.
fn xray_wireguard(outbound: &Value, remarks: &str) -> Option<String> {
    let settings = outbound.get("settings")?;
    let peer = settings.pointer("/peers/0")?;
    let (host, port) = peer.get("endpoint").and_then(Value::as_str)?.rsplit_once(':')?;

    Some(
        WireGuard {
            secret: text(Some(settings), "secretKey"),
            host: host.to_string(),
            port: port.parse().ok()?,
            public_key: text(Some(peer), "publicKey"),
            addresses: strings(settings.get("address")),
            reserved: reserved_text(settings.get("reserved")),
            mtu: number(Some(settings), "mtu"),
            keepalive: number(Some(peer), "keepAlive"),
            name: remarks.to_string(),
        }
        .link(),
    )
}

// ---------------------------------------------------------------------------------- sing-box

/// sing-box outbound types that are not servers: groups of other outbounds, and the local ones.
///
/// A group is skipped rather than expanded because every member is listed as its own outbound in
/// the same configuration — BPB's "Best Ping" `urltest` names the eight servers beside it.
const SINGBOX_NOT_SERVERS: &[&str] = &["selector", "urltest", "direct", "block", "dns"];

/// Turns one sing-box outbound, or endpoint, into a share link.
///
/// sing-box's `network` field is not the transport — it restricts the outbound to TCP or UDP — so
/// the transport is read from `transport.type` alone, and its absence means plain TCP.
fn singbox_link(outbound: &Value) -> Option<String> {
    let o = Some(outbound);
    let kind = text(o, "type");
    if SINGBOX_NOT_SERVERS.contains(&kind.as_str()) {
        return None;
    }
    let name = text(o, "tag");
    let server = text(o, "server");
    let port = number(o, "server_port");

    // `detour` dials this outbound through another, so traffic crosses both: a chain, refused by
    // name for the same reason as Xray's `dialerProxy`. The far hop is this outbound itself.
    if !text(o, "detour").is_empty() {
        let peer = outbound.pointer("/peers/0");
        let (host, port) = if server.is_empty() {
            (text(peer, "address"), number(peer, "port"))
        } else {
            (server, port)
        };
        let endpoint = match port {
            Some(port) => format!("{}:{port}", bracketed(&host)),
            None => host,
        };
        return Some(marker("chain", &endpoint, &name));
    }

    let credential = match kind.as_str() {
        "vless" | "vmess" => text(o, "uuid"),
        "trojan" | "shadowsocks" => text(o, "password"),
        "wireguard" => return singbox_wireguard(outbound, &name),
        _ => return Some(marker(scheme_of(&kind), &server, &name)),
    };
    if server.is_empty() {
        return None;
    }

    let mut node = Node {
        scheme: scheme_of(&kind).to_string(),
        credential,
        address: server,
        port: port?,
        security: "none".into(),
        name,
        ..Node::default()
    };

    let tls = outbound.get("tls");
    let reality = tls.and_then(|t| t.get("reality"));
    let enabled = |v: Option<&Value>| {
        v.and_then(|v| v.get("enabled")).and_then(Value::as_bool) == Some(true)
    };
    if enabled(tls) {
        node.security = if enabled(reality) { "reality" } else { "tls" }.into();
        node.sni = text(tls, "server_name");
        node.insecure = tls.and_then(|t| t.get("insecure")).and_then(Value::as_bool) == Some(true);
        node.alpn = strings(tls.and_then(|t| t.get("alpn")));
        let utls = tls.and_then(|t| t.get("utls"));
        if enabled(utls) {
            node.fingerprint = text(utls, "fingerprint");
        }
        node.public_key = text(reality, "public_key");
        node.short_id = text(reality, "short_id");
    }

    let transport = outbound.get("transport");
    let network = text(transport, "type");
    node.network = if network.is_empty() { "tcp".into() } else { network };
    match node.network.as_str() {
        "ws" => {
            // A header may be a list in sing-box; the first entry is the one sent.
            node.host = strings(transport.and_then(|t| t.pointer("/headers/Host")))
                .into_iter()
                .next()
                .unwrap_or_default();
            node.path = early_data_path(
                text(transport, "path"),
                number(transport, "max_early_data"),
                &text(transport, "early_data_header_name"),
            );
        }
        "http" | "httpupgrade" => {
            node.path = text(transport, "path");
            node.host = strings(transport.and_then(|t| t.get("host")))
                .into_iter()
                .next()
                .unwrap_or_default();
        }
        "grpc" => node.service_name = text(transport, "service_name"),
        _ => {}
    }

    match kind.as_str() {
        "vless" => node.flow = text(o, "flow"),
        "vmess" => {
            node.cipher = text(o, "security");
            node.alter_id = number(o, "alter_id");
        }
        _ => {}
    }

    Some(node.link())
}

/// WireGuard in either of sing-box's shapes: the endpoint it moved to in 1.11 (`address`, and the
/// peer under `peers`), or the older outbound (`local_address`, the peer inline).
fn singbox_wireguard(outbound: &Value, name: &str) -> Option<String> {
    let o = Some(outbound);
    let peer = outbound.pointer("/peers/0");
    let host = either(either(text(peer, "address"), text(peer, "server")), text(o, "server"));
    if host.is_empty() {
        return None;
    }
    let mut addresses = strings(outbound.get("address"));
    if addresses.is_empty() {
        addresses = strings(outbound.get("local_address"));
    }

    Some(
        WireGuard {
            secret: text(o, "private_key"),
            host: bracketed(&host),
            port: number(peer, "port")
                .or_else(|| number(peer, "server_port"))
                .or_else(|| number(o, "server_port"))?,
            public_key: either(text(peer, "public_key"), text(o, "peer_public_key")),
            addresses,
            reserved: either(
                reserved_text(peer.and_then(|p| p.get("reserved"))),
                reserved_text(outbound.get("reserved")),
            ),
            mtu: number(o, "mtu"),
            keepalive: number(peer, "persistent_keepalive_interval"),
            name: name.to_string(),
        }
        .link(),
    )
}

// ------------------------------------------------------------------------------------- clash

/// Clash (mihomo) proxy types that are not servers.
const CLASH_NOT_SERVERS: &[&str] = &["direct", "reject", "dns"];

/// Turns one entry of a Clash configuration's `proxies` into a share link.
///
/// Clash is usually YAML, which this client does not read (see `is_clash_yaml`), but BPB serves
/// `?app=clash` as JSON — YAML's own superset — so its configuration arrives here as ordinary JSON.
/// `proxy-groups` is ignored: its members are all in `proxies`.
fn clash_link(proxy: &Value) -> Option<String> {
    let p = Some(proxy);
    let kind = text(p, "type").to_ascii_lowercase();
    if CLASH_NOT_SERVERS.contains(&kind.as_str()) {
        return None;
    }
    let name = text(p, "name");
    let server = text(p, "server");
    let port = number(p, "port");

    // Clash's spelling of a chain: this proxy dials through the one named.
    if !text(p, "dialer-proxy").is_empty() {
        let endpoint = match port {
            Some(port) => format!("{}:{port}", bracketed(&server)),
            None => server,
        };
        return Some(marker("chain", &endpoint, &name));
    }

    let credential = match kind.as_str() {
        "vless" | "vmess" => text(p, "uuid"),
        "trojan" | "ss" => text(p, "password"),
        "wireguard" => return clash_wireguard(proxy, &name),
        _ => return Some(marker(scheme_of(&kind), &server, &name)),
    };
    if server.is_empty() {
        return None;
    }

    // Trojan is TLS by definition in Clash, so it often says nothing about it.
    let reality = proxy.get("reality-opts").filter(|r| r.is_object());
    let tls = kind == "trojan" || proxy.get("tls").and_then(Value::as_bool) == Some(true);
    let mut node = Node {
        scheme: scheme_of(&kind).to_string(),
        credential,
        address: server,
        port: port?,
        security: match (reality, tls) {
            (Some(_), _) => "reality",
            (None, true) => "tls",
            (None, false) => "none",
        }
        .into(),
        name,
        ..Node::default()
    };

    if node.security != "none" {
        // VLESS and VMess call it `servername`, Trojan `sni`.
        node.sni = either(text(p, "servername"), text(p, "sni"));
        node.fingerprint = text(p, "client-fingerprint");
        node.insecure = proxy.get("skip-cert-verify").and_then(Value::as_bool) == Some(true);
        node.alpn = strings(proxy.get("alpn"));
        node.public_key = text(reality, "public-key");
        node.short_id = text(reality, "short-id");
    }

    let network = text(p, "network");
    node.network = if network.is_empty() { "tcp".into() } else { network };
    let first = |value: Option<&Value>| strings(value).into_iter().next().unwrap_or_default();
    match node.network.as_str() {
        "ws" => {
            let ws = proxy.get("ws-opts");
            // Older Clash configs spell these as top-level `ws-path` and `ws-headers`.
            let path = either(text(ws, "path"), text(p, "ws-path"));
            node.host = first(ws.and_then(|w| w.pointer("/headers/Host")));
            if node.host.is_empty() {
                node.host = first(proxy.pointer("/ws-headers/Host"));
            }
            // mihomo's switch for HTTPUpgrade, which it carries as a kind of WebSocket.
            if ws.and_then(|w| w.get("v2ray-http-upgrade")).and_then(Value::as_bool) == Some(true) {
                node.network = "httpupgrade".into();
                node.path = path;
            } else {
                node.path = early_data_path(
                    path,
                    number(ws, "max-early-data"),
                    &text(ws, "early-data-header-name"),
                );
            }
        }
        "grpc" => node.service_name = text(proxy.get("grpc-opts"), "grpc-service-name"),
        // Clash's `h2` is the HTTP transport; its `http` is TCP with an HTTP header in front.
        "h2" => {
            let h2 = proxy.get("h2-opts");
            node.network = "http".into();
            node.path = text(h2, "path");
            node.host = first(h2.and_then(|h| h.get("host")));
        }
        "http" => {
            let http = proxy.get("http-opts");
            node.network = "tcp".into();
            node.header_type = "http".into();
            node.path = first(http.and_then(|h| h.get("path")));
            node.host = first(http.and_then(|h| h.pointer("/headers/Host")));
        }
        _ => {}
    }

    match kind.as_str() {
        "vless" => node.flow = text(p, "flow"),
        "vmess" => {
            node.cipher = text(p, "cipher");
            node.alter_id = number(p, "alterId");
        }
        _ => {}
    }

    Some(node.link())
}

/// WireGuard as Clash writes it: bare interface addresses under `ip` and `ipv6`, which a link needs
/// as prefixes, and the peer either inline or under `peers`.
fn clash_wireguard(proxy: &Value, name: &str) -> Option<String> {
    let p = Some(proxy);
    let peer = proxy.pointer("/peers/0");
    let host = either(text(peer, "server"), text(p, "server"));
    if host.is_empty() {
        return None;
    }
    let addresses = ["ip", "ipv6"]
        .iter()
        .map(|key| text(p, key))
        .filter(|address| !address.is_empty())
        .map(|address| match (address.contains('/'), address.contains(':')) {
            (true, _) => address,
            (false, true) => format!("{address}/128"),
            (false, false) => format!("{address}/32"),
        })
        .collect();

    Some(
        WireGuard {
            secret: text(p, "private-key"),
            host: bracketed(&host),
            port: number(peer, "port").or_else(|| number(p, "port"))?,
            public_key: either(text(peer, "public-key"), text(p, "public-key")),
            addresses,
            reserved: either(
                reserved_text(peer.and_then(|x| x.get("reserved"))),
                reserved_text(proxy.get("reserved")),
            ),
            mtu: number(p, "mtu"),
            keepalive: number(p, "persistent-keepalive"),
            name: name.to_string(),
        }
        .link(),
    )
}

/// Whether a body that is not JSON is a Clash configuration in YAML.
///
/// Refused by name rather than read. Reading YAML means carrying a parser for an untrusted body,
/// for a format the panels this client is built around serve as JSON anyway — and letting it fall
/// through to the line scan is worse than either: that finds the `://` in its DNS servers and
/// health-check URLs and reports each one as a server.
fn is_clash_yaml(body: &str) -> bool {
    body.lines().any(|line| line.starts_with("proxies:"))
}

// ----------------------------------------------------------------------------------- escaping

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

/// The inverse of `percent_encode`: `None` for a malformed escape or bytes that are not UTF-8.
fn percent_decode(value: &str) -> Option<String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

// ----------------------------------------------------------------------------------- fetching

/// Fetches a subscription and returns its links and allowance.
///
/// Blocking: call it from `spawn_blocking`.
pub fn fetch(url: &str) -> Result<Fetched, SubscriptionError> {
    let address = resolve(url)?;

    let agent = ureq::AgentBuilder::new()
        .timeout(TIMEOUT)
        .user_agent(USER_AGENT)
        .build();

    let response = match agent.get(&address).call() {
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
    let decoded = decode_body(body);
    if is_clash_yaml(&decoded) {
        return Err(SubscriptionError::Rejected(
            "This subscription is a Clash configuration in YAML, which this build cannot read. \
             Use the provider's link for another app — sing-box, Xray or v2rayNG — instead."
                .into(),
        ));
    }
    let links = extract_links(&decoded);
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

    // ---------------------------------------------------------- the links a BPB panel offers

    /// One BPB subscription, three ways, as its panel hands them out: an import link for sing-box
    /// wrapping the address raw, and the address itself asked for Clash and for Xray. Each carries
    /// the panel's display name in its fragment.
    const SING_BOX_LINK: &str = "sing-box://import-remote-profile?url=https://edge.example.net/Xq3vT9pLmN2wR8sK/sub/normal?app=sing-box#%F0%9F%92%A6%20BPB%20Normal";
    const CLASH_LINK: &str =
        "https://edge.example.net/Xq3vT9pLmN2wR8sK/sub/normal?app=clash#%F0%9F%92%A6%20BPB%20Normal";
    const XRAY_LINK: &str =
        "https://edge.example.net/Xq3vT9pLmN2wR8sK/sub/normal?app=xray#%F0%9F%92%A6%20BPB%20Normal";

    /// What BPB serves to `?app=sing-box`: one configuration holding every server as an outbound,
    /// beside a selector, a `urltest` "Best Ping" group over the same servers, and `direct`. The
    /// same two servers as `bpb_array`, trimmed of the inbounds, routing and most of the DNS.
    fn bpb_sing_box() -> &'static str {
        r#"{
          "dns": { "servers": [{ "type": "https", "server": "8.8.8.8", "detour": "✅ Selector", "tag": "dns-remote" }] },
          "outbounds": [
            {
              "tag": "💦 1. VLESS - Domain : 443",
              "type": "vless",
              "server": "edge.example.net",
              "server_port": 443,
              "tcp_fast_open": false,
              "uuid": "455c35ab-42b9-43d7-b63c-ea915c2c72ab",
              "packet_encoding": "",
              "network": "tcp",
              "tls": {
                "enabled": true,
                "server_name": "edge.example.net",
                "record_fragment": false,
                "insecure": false,
                "alpn": ["http/1.1"],
                "utls": { "enabled": true, "fingerprint": "chrome" }
              },
              "transport": {
                "type": "ws",
                "path": "/vl/8NjpyzwBr4ARYZEeGqW",
                "max_early_data": 2560,
                "early_data_header_name": "Sec-WebSocket-Protocol",
                "headers": { "Host": "edge.example.net" }
              },
              "domain_resolver": "dns-direct"
            },
            {
              "tag": "💦 1. Trojan - Domain : 443",
              "type": "trojan",
              "server": "edge.example.net",
              "server_port": 443,
              "password": "E1wy;,GYGPPhYEYXn",
              "network": "tcp",
              "tls": {
                "enabled": true,
                "server_name": "edge.example.net",
                "insecure": false,
                "utls": { "enabled": true, "fingerprint": "chrome" }
              },
              "transport": {
                "type": "ws",
                "path": "/tr/FHxj7oXIlRMTvceFHCN",
                "max_early_data": 2560,
                "early_data_header_name": "Sec-WebSocket-Protocol",
                "headers": { "Host": "edge.example.net" }
              }
            },
            { "type": "selector", "tag": "✅ Selector",
              "outbounds": ["💦 Best Ping 🚀", "💦 1. VLESS - Domain : 443", "💦 1. Trojan - Domain : 443"] },
            { "type": "direct", "tag": "direct" },
            { "type": "urltest", "tag": "💦 Best Ping 🚀",
              "outbounds": ["💦 1. VLESS - Domain : 443", "💦 1. Trojan - Domain : 443"],
              "url": "https://www.gstatic.com/generate_204" }
          ]
        }"#
    }

    /// What BPB serves to `?app=clash`: a Clash configuration, but as JSON rather than YAML, with
    /// the servers under `proxies` and the groups over them under `proxy-groups`. The same two
    /// servers again.
    fn bpb_clash() -> &'static str {
        r#"{
          "mixed-port": 7890,
          "dns": { "nameserver": ["https://8.8.8.8/dns-query"] },
          "proxies": [
            {
              "name": "💦 1. VLESS - Domain : 443",
              "type": "vless",
              "server": "edge.example.net",
              "port": 443,
              "ip-version": "ipv4",
              "tfo": false,
              "udp": false,
              "uuid": "455c35ab-42b9-43d7-b63c-ea915c2c72ab",
              "packet-encoding": "",
              "encryption": "",
              "tls": true,
              "servername": "edge.example.net",
              "client-fingerprint": "chrome",
              "skip-cert-verify": false,
              "alpn": ["http/1.1"],
              "network": "ws",
              "ws-opts": {
                "path": "/vl/8NjpyzwBr4ARYZEeGqW",
                "max-early-data": 2560,
                "early-data-header-name": "Sec-WebSocket-Protocol",
                "headers": { "Host": "edge.example.net" }
              }
            },
            {
              "name": "💦 1. Trojan - Domain : 443",
              "type": "trojan",
              "server": "edge.example.net",
              "port": 443,
              "password": "E1wy;,GYGPPhYEYXn",
              "tls": true,
              "sni": "edge.example.net",
              "client-fingerprint": "chrome",
              "skip-cert-verify": false,
              "network": "ws",
              "ws-opts": {
                "path": "/tr/FHxj7oXIlRMTvceFHCN",
                "max-early-data": 2560,
                "early-data-header-name": "Sec-WebSocket-Protocol",
                "headers": { "Host": "edge.example.net" }
              }
            }
          ],
          "proxy-groups": [
            { "name": "✅ Selector", "type": "select",
              "proxies": ["💦 Best Ping 🚀", "💦 1. VLESS - Domain : 443", "💦 1. Trojan - Domain : 443"] },
            { "name": "💦 Best Ping 🚀", "type": "url-test", "url": "https://www.gstatic.com/generate_204",
              "proxies": ["💦 1. VLESS - Domain : 443", "💦 1. Trojan - Domain : 443"] }
          ]
        }"#
    }

    #[test]
    fn a_sing_box_import_link_is_fetched_at_the_address_it_carries() {
        assert_eq!(
            resolve(SING_BOX_LINK).unwrap(),
            "https://edge.example.net/Xq3vT9pLmN2wR8sK/sub/normal?app=sing-box"
        );
    }

    /// The fragment is the panel's name for the subscription, not part of its address.
    #[test]
    fn the_clash_and_xray_links_are_fetched_as_written_less_their_name() {
        assert_eq!(
            resolve(CLASH_LINK).unwrap(),
            "https://edge.example.net/Xq3vT9pLmN2wR8sK/sub/normal?app=clash"
        );
        assert_eq!(
            resolve(XRAY_LINK).unwrap(),
            "https://edge.example.net/Xq3vT9pLmN2wR8sK/sub/normal?app=xray"
        );
    }

    /// Each link asks for a different app's format, and each format comes back as the same two
    /// servers — the whole path, from what the user pasted to the links the frontend parses.
    #[test]
    fn the_three_links_a_bpb_panel_offers_all_import() {
        for (link, app, body) in [
            (SING_BOX_LINK, "sing-box", bpb_sing_box()),
            (CLASH_LINK, "clash", bpb_clash()),
            (XRAY_LINK, "xray", bpb_array()),
        ] {
            assert!(resolve(link).unwrap().ends_with(&format!("?app={app}")), "{link}");
            let fetched = assemble(None, None, body).unwrap();
            assert_eq!(fetched.links.len(), 2, "{app}: {:?}", fetched.links);
            assert!(fetched.links[0].starts_with("vless://"), "{app}");
            assert!(fetched.links[1].starts_with("trojan://"), "{app}");
        }
    }

    /// Xray, sing-box and Clash spell one server three ways. Which button the user copied must not
    /// change what gets imported, so all three have to come out as the very same links.
    #[test]
    fn every_format_of_one_subscription_imports_the_same_servers() {
        let xray = extract_links(bpb_array());
        assert_eq!(extract_links(bpb_sing_box()), xray);
        assert_eq!(extract_links(bpb_clash()), xray);
    }

    /// sing-box and Clash keep early data in two fields beside the path; the link form carries it
    /// inside the path as `?ed=N`, where the frontend lifts it back out.
    #[test]
    fn early_data_is_written_back_into_the_path_the_way_links_carry_it() {
        for body in [bpb_sing_box(), bpb_clash()] {
            let links = extract_links(body);
            assert!(links[0].contains("path=%2Fvl%2F8NjpyzwBr4ARYZEeGqW%3Fed%3D2560"), "{}", links[0]);
        }
    }

    #[test]
    fn early_data_under_another_header_names_it_in_the_path() {
        let body = r#"{"outbounds":[{"type":"vless","tag":"x","server":"a.example.net","server_port":443,"uuid":"u",
          "transport":{"type":"ws","path":"/p","max_early_data":2048,"early_data_header_name":"X-Early"}}]}"#;
        assert!(extract_links(body)[0].contains("path=%2Fp%3Fed%3D2048%26eh%3DX-Early"));
    }

    /// The groups are not servers, and every member of one is listed on its own already.
    #[test]
    fn sing_box_selectors_and_url_tests_are_not_imported_as_servers() {
        let links = extract_links(bpb_sing_box());
        assert!(!links.iter().any(|l| l.contains("Best") || l.contains("Selector")), "{links:?}");
    }

    /// Clash's groups hold a health-check URL, which must not be read as a server either.
    #[test]
    fn clash_proxy_groups_are_not_imported_as_servers() {
        let links = extract_links(bpb_clash());
        assert!(!links.iter().any(|l| l.contains("gstatic") || l.contains("Best")), "{links:?}");
    }

    /// Trojan is always TLS in Clash, so a configuration is free to leave `tls` out.
    #[test]
    fn a_clash_trojan_is_tls_even_when_it_does_not_say_so() {
        let body = r#"{"proxies":[{"name":"t","type":"trojan","server":"a.example.net","port":443,
          "password":"p","sni":"a.example.net"}]}"#;
        let link = &extract_links(body)[0];
        assert!(link.contains("security=tls"), "{link}");
        assert!(link.contains("sni=a.example.net"), "{link}");
    }

    #[test]
    fn an_import_link_may_carry_its_address_percent_encoded() {
        // The encoded form keeps the carried address's own `&` inside it.
        let link = "sing-box://import-remote-profile?url=https%3A%2F%2Fedge.example.net%2Fsub%3Fapp%3Dsing-box%26lang%3Den#Normal";
        assert_eq!(resolve(link).unwrap(), "https://edge.example.net/sub?app=sing-box&lang=en");
    }

    /// Clash's import link puts the name in its own parameter, which is not part of the address.
    #[test]
    fn a_clash_import_link_is_unwrapped_and_its_name_left_behind() {
        let link = "clash://install-config?url=https%3A%2F%2Fedge.example.net%2Fsub%2Fnormal%3Fapp%3Dclash&name=BPB%20Normal";
        assert_eq!(resolve(link).unwrap(), "https://edge.example.net/sub/normal?app=clash");
    }

    /// The wrapper changes nothing about the rule: the address inside is a credential, and plain
    /// HTTP would hand it to the network.
    #[test]
    fn an_import_link_wrapping_plain_http_is_refused() {
        let err = resolve("sing-box://import-remote-profile?url=http://edge.example.net/sub#x").unwrap_err();
        assert!(err.to_string().contains("plain HTTP"), "{err}");
    }

    #[test]
    fn an_import_link_carrying_no_address_is_refused() {
        let err = resolve("sing-box://import-remote-profile?name=x").unwrap_err();
        assert!(matches!(err, SubscriptionError::Rejected(_)), "{err}");
    }

    #[test]
    fn a_sing_box_detour_is_a_chain_and_refused_by_name() {
        let body = r#"{"endpoints":[
          { "type": "wireguard", "tag": "💦 1 - WoW", "detour": "💦 1 - Warp",
            "private_key": "s", "address": ["172.16.0.2/32"],
            "peers": [{ "address": "162.159.192.1", "port": 2408, "public_key": "k" }] }
        ]}"#;
        let links = extract_links(body);
        assert_eq!(links.len(), 1);
        assert!(links[0].starts_with("chain://162.159.192.1:2408#"), "got {}", links[0]);
    }

    #[test]
    fn a_clash_dialer_proxy_is_a_chain_and_refused_by_name() {
        let body = r#"{"proxies":[{"name":"WoW","type":"wireguard","server":"162.159.192.1","port":2408,
          "dialer-proxy":"Warp","private-key":"s","public-key":"k","ip":"172.16.0.2"}]}"#;
        assert!(extract_links(body)[0].starts_with("chain://162.159.192.1:2408#"));
    }

    /// sing-box moved WireGuard from `outbounds` to `endpoints`, with the peer under `peers`. The
    /// WARP client id has to come through, or the tunnel comes up and carries nothing.
    #[test]
    fn a_sing_box_wireguard_endpoint_keeps_its_keys_and_reserved_bytes() {
        let body = r#"{"endpoints":[{
          "type": "wireguard", "tag": "💦 1 - Warp", "mtu": 1280,
          "address": ["172.16.0.2/32", "2606:4700:110::1/128"],
          "private_key": "f7m/C8NHWPWIkGAbxTBAMhYHlzu3Ya7lSCbOSqfGu68=",
          "peers": [{ "address": "engage.cloudflareclient.com", "port": 2408,
                      "public_key": "bmXOC+F1FxEMF9dyiK2H5/1SUtzH0JuVo51h2wPfgyo=",
                      "reserved": [216, 253, 3], "persistent_keepalive_interval": 5,
                      "allowed_ips": ["0.0.0.0/0", "::/0"] }]
        }]}"#;
        let link = &extract_links(body)[0];
        assert!(
            link.starts_with("wireguard://f7m%2FC8NHWPWIkGAbxTBAMhYHlzu3Ya7lSCbOSqfGu68%3D@engage.cloudflareclient.com:2408?"),
            "got {link}"
        );
        assert!(link.contains("publickey=bmXOC%2BF1FxEMF9dyiK2H5%2F1SUtzH0JuVo51h2wPfgyo%3D"));
        assert!(link.contains("address=172.16.0.2%2F32%2C2606%3A4700%3A110%3A%3A1%2F128"));
        assert!(link.contains("reserved=216%2C253%2C3"));
        assert!(link.contains("mtu=1280"));
        assert!(link.contains("keepalive=5"));
    }

    /// Clash gives the interface bare addresses; the link needs them as prefixes.
    #[test]
    fn a_clash_wireguard_proxy_gets_prefixes_on_its_bare_addresses() {
        let body = r#"{"proxies":[{"name":"Warp","type":"wireguard","server":"engage.cloudflareclient.com",
          "port":2408,"ip":"172.16.0.2","ipv6":"2606:4700:110::1","private-key":"s","public-key":"k",
          "reserved":[216,253,3],"mtu":1280}]}"#;
        let link = &extract_links(body)[0];
        assert!(link.starts_with("wireguard://s@engage.cloudflareclient.com:2408?"), "{link}");
        assert!(link.contains("address=172.16.0.2%2F32%2C2606%3A4700%3A110%3A%3A1%2F128"), "{link}");
        assert!(link.contains("reserved=216%2C253%2C3"), "{link}");
    }

    #[test]
    fn clash_reality_keys_are_read_from_reality_opts() {
        let body = r#"{"proxies":[{"name":"R","type":"vless","server":"a.example.net","port":443,"uuid":"u",
          "flow":"xtls-rprx-vision","tls":true,"servername":"www.example.net","client-fingerprint":"chrome",
          "reality-opts":{"public-key":"PBK","short-id":"ab"},"network":"tcp"}]}"#;
        let link = &extract_links(body)[0];
        assert!(link.contains("security=reality"), "{link}");
        assert!(link.contains("pbk=PBK"));
        assert!(link.contains("sid=ab"));
        assert!(link.contains("sni=www.example.net"));
        assert!(link.contains("flow=xtls-rprx-vision"));
    }

    #[test]
    fn sing_box_reality_keys_are_read_from_the_tls_block() {
        let body = r#"{"outbounds":[{"type":"vless","tag":"R","server":"a.example.net","server_port":443,
          "uuid":"u","flow":"xtls-rprx-vision",
          "tls":{"enabled":true,"server_name":"www.example.net","utls":{"enabled":true,"fingerprint":"chrome"},
                 "reality":{"enabled":true,"public_key":"PBK","short_id":"ab"}}}]}"#;
        assert_eq!(
            extract_links(body)[0],
            "vless://u@a.example.net:443?type=tcp&security=reality&sni=www.example.net&fp=chrome&pbk=PBK&sid=ab&flow=xtls-rprx-vision#R"
        );
    }

    /// VMess over gRPC: the cipher and alterId live on the outbound, the service name on the
    /// transport, and each has a different name in the link.
    #[test]
    fn sing_box_vmess_over_grpc_keeps_its_cipher_and_service_name() {
        let body = r#"{"outbounds":[{"type":"vmess","tag":"V","server":"a.example.net","server_port":443,
          "uuid":"u","security":"auto","alter_id":0,
          "tls":{"enabled":true,"server_name":"a.example.net"},
          "transport":{"type":"grpc","service_name":"svc"}}]}"#;
        assert_eq!(
            extract_links(body)[0],
            "vmess://u@a.example.net:443?type=grpc&security=tls&serviceName=svc&sni=a.example.net&encryption=auto&alterId=0#V"
        );
    }

    /// `shadowsocks` is sing-box's word; the frontend knows the scheme as `ss` and names it in its
    /// refusal. Under the long spelling it could only say "not a protocol this build knows".
    #[test]
    fn shadowsocks_is_written_under_the_scheme_the_frontend_can_name() {
        let body = r#"{"outbounds":[{"type":"shadowsocks","tag":"S","server":"a.example.net","server_port":8388,
          "method":"aes-128-gcm","password":"p"}]}"#;
        assert!(extract_links(body)[0].starts_with("ss://"));
    }

    /// Clash's usual form is YAML, which this build does not read. Scanned as lines it would
    /// report its DNS servers and health-check URLs as servers, so it is refused by name instead.
    #[test]
    fn a_clash_yaml_configuration_is_refused_by_name_rather_than_scanned() {
        let body = "mixed-port: 7890\n\
                    dns:\n  nameserver:\n    - https://8.8.8.8/dns-query\n\
                    proxies:\n  - {name: A, type: vless, server: a.example.net, port: 443, uuid: u}\n\
                    proxy-groups:\n  - {name: Auto, type: url-test, url: https://www.gstatic.com/generate_204, proxies: [A]}\n";
        let err = assemble(None, None, body).unwrap_err();
        assert!(matches!(err, SubscriptionError::Rejected(_)), "{err}");
        assert!(err.to_string().contains("Clash"), "{err}");
    }
}
