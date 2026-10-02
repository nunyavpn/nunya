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
    mode: String,
    extra: String,
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
            ("mode", self.mode.clone()),
            ("extra", self.extra.clone()),
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
        "xhttp" | "splithttp" => {
            let settings = at("xhttpSettings").or_else(|| at("splithttpSettings"));
            node.path = text(settings, "path");
            node.host = text(settings, "host");
            node.mode = text(settings, "mode");
            if let Some(settings) = settings.and_then(Value::as_object) {
                // Xray's extra replaces the other advanced fields; keep exactly that precedence.
                let extra = settings.get("extra").cloned().unwrap_or_else(|| {
                    Value::Object(settings.iter()
                        .filter(|(k, _)| !["host", "path", "mode"].contains(&k.as_str()))
                        .map(|(k, v)| (k.clone(), v.clone())).collect())
                });
                node.extra = extra.to_string();
            }
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
        if matches!(node.network.as_str(), "xhttp" | "splithttp") {
            node.cipher = text(user, "encryption");
        }
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

    // No proxy from the environment: a subscription URL is a credential, and it goes only where
    // the system routes it — through the tunnel when one is up.
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .proxy(None)
        .timeout_global(Some(TIMEOUT))
        .user_agent(USER_AGENT)
        .build()
        .into();

    let response = match agent.get(&address).call() {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(status)) => return Err(SubscriptionError::Status { status }),
        Err(e) => return Err(SubscriptionError::Network(e.to_string())),
    };

    let header = |name: &str| response.headers().get(name).and_then(|v| v.to_str().ok());
    let quota = header("subscription-userinfo").map(parse_userinfo);
    let title = header("profile-title")
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());

    // Capped read rather than into_string(), which would happily consume an unbounded body.
    let mut body = String::new();
    response
        .into_body()
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
mod tests;
