use super::*;

#[test]
fn parses_the_names_the_docs_use() {
    assert_eq!(Kind::parse("subprocess"), Some(Kind::Subprocess));
    assert_eq!(Kind::parse("  SubProcess "), Some(Kind::Subprocess));
}

#[cfg(target_os = "macos")]
#[test]
fn parses_the_network_extension_aliases() {
    assert_eq!(Kind::parse("networkextension"), Some(Kind::NetworkExtension));
    assert_eq!(Kind::parse("ne"), Some(Kind::NetworkExtension));
    assert_eq!(Kind::parse("appex"), Some(Kind::NetworkExtension));
}

#[test]
fn unknown_names_are_rejected_rather_than_guessed() {
    assert_eq!(Kind::parse("wireguard"), None);
    assert_eq!(Kind::parse(""), None);
}

/// Defaulting to NetworkExtension before it can be signed would break every developer who does
/// not have an Apple Developer membership.
#[test]
fn the_default_is_the_one_that_works_unsigned() {
    assert_eq!(Kind::default_for_platform(), Kind::Subprocess);
}
