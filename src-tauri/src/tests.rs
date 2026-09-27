use super::TEST_URLS;

/// The chain exists because one endpoint is never enough, and the order is load-bearing.
///
/// A Cloudflare Workers proxy cannot reach Cloudflare, so leading with `cp.cloudflare.com`
/// reports every Workers-based server — most of a typical free subscription — as unreachable
/// while it is working perfectly. That was the original bug; this is the guard against
/// "simplifying" the list back to one entry or reordering it.
#[test]
fn the_fallback_chain_does_not_lead_with_cloudflare() {
    assert!(
        TEST_URLS.len() > 1,
        "a single endpoint cannot measure both Workers proxies and WARP"
    );
    assert!(
        !TEST_URLS[0].contains("cloudflare"),
        "the first endpoint is the one most servers are measured against, and a Cloudflare \
             Workers proxy can never reach Cloudflare"
    );
}
