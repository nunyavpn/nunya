use super::*;

#[test]
fn parses_the_names_the_docs_use() {
    assert_eq!(Kind::parse("subprocess"), Some(Kind::Subprocess));
    assert_eq!(Kind::parse("  SubProcess "), Some(Kind::Subprocess));
}

/// Accepted where the platform has the extension, and refused by name elsewhere rather than
/// quietly built into something else.
#[test]
fn the_network_extension_aliases_parse_only_where_there_is_one() {
    let expected = platform::NETWORK_EXTENSION.then_some(Kind::NetworkExtension);
    for name in ["networkextension", "ne", "appex"] {
        assert_eq!(Kind::parse(name), expected, "{name}");
    }
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
