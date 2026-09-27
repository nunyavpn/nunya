use super::*;

#[test]
fn the_donation_page_is_allowed() {
    assert!(allowed("https://buymeacoffee.com/nunya").is_ok());
    assert!(allowed("https://www.buymeacoffee.com/nunya?ref=app#top").is_ok());
    // Host names are case-insensitive, and a link typed by hand is not always lowercase.
    assert!(allowed("https://BuyMeACoffee.com/nunya").is_ok());
}

#[test]
fn plain_http_is_refused() {
    assert!(allowed("http://buymeacoffee.com/nunya").is_err());
}

#[test]
fn any_other_host_is_refused() {
    assert!(allowed("https://example.net/").is_err());
    assert!(allowed("https://pay.buymeacoffee.com/nunya").is_err());
}

/// The three ways a URL can look like an allowed host and not be one.
#[test]
fn lookalike_hosts_are_refused() {
    assert!(allowed("https://buymeacoffee.com.example.net/nunya").is_err());
    assert!(allowed("https://buymeacoffee.com@example.net/").is_err());
    assert!(allowed("https://buymeacoffee.com:443@example.net/").is_err());
}

#[test]
fn other_schemes_and_hidden_characters_are_refused() {
    assert!(allowed("javascript:alert(1)").is_err());
    assert!(allowed("file:///etc/passwd").is_err());
    assert!(allowed("https://buymeacoffee.com/a b").is_err());
    assert!(allowed("https://buymeacoffee.com/\nhttps://example.net").is_err());
    assert!(allowed("").is_err());
}
