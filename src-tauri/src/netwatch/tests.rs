use super::*;

fn ip(s: &str) -> Option<IpAddr> {
    Some(s.parse().unwrap())
}

#[test]
fn the_first_look_is_not_a_change() {
    let mut watch = Watch::default();
    assert!(!watch.saw(ip("192.168.1.20")));
}

#[test]
fn staying_on_one_network_is_not_a_change() {
    let mut watch = Watch::default();
    watch.saw(ip("192.168.1.20"));
    assert!(!watch.saw(ip("192.168.1.20")));
}

#[test]
fn moving_from_one_network_straight_to_another_is_a_change() {
    let mut watch = Watch::default();
    watch.saw(ip("192.168.1.20"));
    assert!(watch.saw(ip("172.20.10.3")));
}

#[test]
fn going_offline_and_coming_back_are_both_changes() {
    let mut watch = Watch::default();
    watch.saw(ip("192.168.1.20"));
    assert!(watch.saw(None));
    assert!(watch.saw(ip("192.168.1.20")));
}
