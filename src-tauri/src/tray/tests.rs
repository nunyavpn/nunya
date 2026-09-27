use super::Lines;

#[test]
fn a_tunnel_that_carries_nothing_says_not_working_and_still_offers_disconnect() {
    let lines = Lines::new("on", "failed", Some("DE-1 Frankfurt"), None, true);
    assert_eq!(lines.status, "Status: Not working");
    assert_eq!(lines.toggle, "Disconnect");
    assert!(lines.toggle_enabled);
}

#[test]
fn an_attempt_that_failed_says_not_working_and_offers_connect_again() {
    let lines = Lines::new("off", "failed", Some("DE-1 Frankfurt"), None, true);
    assert_eq!(lines.status, "Status: Not working");
    assert_eq!(lines.toggle, "Connect");
    assert!(lines.toggle_enabled);
}

#[test]
fn connect_is_offered_only_when_the_window_would_accept_it() {
    assert!(Lines::new("off", "off", Some("a"), None, true).toggle_enabled);
    assert!(!Lines::new("off", "off", Some("a"), None, false).toggle_enabled);
    // Not a second attempt on top of the first, nor a second stop.
    assert!(!Lines::new("connecting", "connecting", Some("a"), None, true).toggle_enabled);
    assert!(!Lines::new("disconnecting", "connecting", Some("a"), None, true).toggle_enabled);
}

#[test]
fn proxy_mode_names_the_listener_rather_than_the_machine() {
    let lines = Lines::new("on", "on", Some("a"), Some("proxy on 127.0.0.1:2080"), true);
    assert_eq!(lines.status, "Status: Connected · proxy on 127.0.0.1:2080");
}

#[test]
fn no_selection_is_said_rather_than_left_blank() {
    let lines = Lines::new("off", "off", None, None, false);
    assert_eq!(lines.server, "No server selected");
    assert_eq!(lines.status, "Status: Disconnected");
}
