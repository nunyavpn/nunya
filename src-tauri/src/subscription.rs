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
fn extract_links(body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && line.contains("://"))
        .map(str::to_string)
        .collect()
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
}
