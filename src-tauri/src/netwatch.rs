//! Notices when this machine moves to another network, so the frontend can find where it is now.
//!
//! "Where you are" is the first dot on the map and the start of every route drawn from it, and it
//! is looked up at launch (`locate_me`). A laptop that goes from home Wi-Fi to a phone's hotspot is
//! somewhere else afterwards, and the map went on drawing routes from home.
//!
//! The signal is the address this machine would send from to reach the internet: the one the
//! system picks for a UDP socket "connected" to a public address. Connecting a UDP socket sends
//! nothing — it only consults the routing table — so a look costs three syscalls, no traffic and no
//! permission. Another network means another address, or none and then one. The webview's `online`
//! event is not enough: it misses a move from one network straight to another, which is the common
//! case.
//!
//! In VPN mode the route leads into the TUN, so connecting and disconnecting change the answer as
//! well. That costs nothing: the frontend looks the location up only with the tunnel down, and
//! after every disconnect anyway.

use std::net::{IpAddr, UdpSocket};
use std::time::Duration;

use tauri::{AppHandle, Emitter};

/// How often to look. A move is noticed within this, and a look is nearly free.
const EVERY: Duration = Duration::from_secs(4);

/// Any public address will do; nothing is sent to it.
const PROBE: &str = "1.1.1.1:53";

/// The address this machine would send from to reach the internet, if it can reach it at all.
fn route_source() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(PROBE).ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}

/// What has been seen so far, and whether the latest look is a change.
#[derive(Default)]
struct Watch {
    /// `None` before the first look; `Some(None)` when there was no route.
    last: Option<Option<IpAddr>>,
}

impl Watch {
    /// The first look is where things stand, not a change: launch already looks the location up.
    fn saw(&mut self, now: Option<IpAddr>) -> bool {
        let changed = matches!(self.last, Some(before) if before != now);
        self.last = Some(now);
        changed
    }
}

/// Looks every `EVERY` for as long as the app runs, and emits `network-changed` on a change.
pub fn spawn(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut watch = Watch::default();
        loop {
            // Blocking in name only: a socket, a connect and a getsockname, with no packet and no
            // name to resolve.
            if watch.saw(route_source()) {
                // The address itself is not logged: a log is something users paste into issues.
                log::info!("the network changed");
                let _ = app.emit("network-changed", ());
            }
            tokio::time::sleep(EVERY).await;
        }
    });
}

#[cfg(test)]
mod tests {
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
}
