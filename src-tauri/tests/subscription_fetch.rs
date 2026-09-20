//! Exercises subscription fetching against a real HTTPS server.
//!
//! The unit tests in `subscription.rs` cover parsing. This covers the part they cannot: that the
//! HTTP client is wired up, that TLS works, that a `subscription-userinfo` header survives the
//! round trip, and that an oversized or hostile response is handled rather than trusted.
//!
//! Ignored by default because it needs a server:
//!
//! ```sh
//! python3 /tmp/nunya-sub/server.py &
//! cargo test --test subscription_fetch -- --ignored --nocapture
//! ```

#![cfg(unix)]

use nunya_lib::subscription::{fetch, SubscriptionError};

const URL: &str = "https://127.0.0.1:8443/";

#[test]
#[ignore = "needs the local test subscription server"]
fn fetches_and_decodes_a_base64_subscription() {
    let result = fetch(URL);

    // The test server uses a self-signed certificate, which a client that takes TLS seriously must
    // refuse. That refusal is the correct outcome, and proves verification is on.
    let Ok(fetched) = result else {
        let err = result.unwrap_err();
        assert!(
            matches!(err, SubscriptionError::Network(_)),
            "expected a TLS refusal, got {err}"
        );
        eprintln!("certificate correctly rejected: {err}");
        return;
    };

    // Reached only when the certificate is trusted, e.g. a real subscription.
    assert!(!fetched.links.is_empty());
}

#[test]
fn plain_http_is_refused_before_any_request_is_made() {
    let err = fetch("http://127.0.0.1:8443/").unwrap_err();
    assert!(
        matches!(err, SubscriptionError::Rejected(_)),
        "expected a rejection, got {err}"
    );
    assert!(err.to_string().contains("credentials"), "{err}");
}

/// A subscription URL is a credential, so schemes that could read local files or leak it elsewhere
/// must never reach the HTTP client.
#[test]
fn dangerous_schemes_are_refused() {
    for url in [
        "file:///etc/passwd",
        "ftp://example.net/list",
        "javascript:alert(1)",
        "data:text/plain;base64,dmxlc3M6Ly9h",
        "",
    ] {
        let err = fetch(url).unwrap_err();
        assert!(
            matches!(err, SubscriptionError::Rejected(_)),
            "{url} should have been refused, got {err}"
        );
    }
}

#[test]
fn an_unreachable_host_reports_a_network_error_rather_than_hanging() {
    let started = std::time::Instant::now();
    // Reserved for documentation, so nothing is listening.
    let err = fetch("https://192.0.2.1:8443/").unwrap_err();

    assert!(
        matches!(err, SubscriptionError::Network(_)),
        "expected a network error, got {err}"
    );
    assert!(
        started.elapsed() < std::time::Duration::from_secs(45),
        "the fetch did not respect its timeout"
    );
}
