//! The ad blocker's and the anti-tracker's lists: where they come from, how they are checked, and
//! where they are kept.
//!
//! The core reads rule sets itself, and it could download them itself too (`remote` rule sets).
//! It is not asked to, because of what it does when a download fails: a remote rule set with
//! nothing cached fails the core's start ("initial rule-set" in sing-box's `rule_set_remote.go`),
//! so a GitHub outage — or a network where GitHub is blocked, which is most of the networks this
//! client is for — would turn into a Connect button that does not work. Here the app downloads
//! each list, has the core check it, and keeps it on disk. A switch whose list has not arrived yet
//! is left out of the config (`on_disk`), and the tunnel comes up without it.
//!
//! **Ads** is `geosite-category-ads-all` from SagerNet/sing-geosite: the rule sets sing-box's own
//! authors publish, built from v2fly's domain-list-community, already in the core's binary format.
//!
//! **Trackers** is HaGeZi's Native Tracker lists: the telemetry built into operating systems,
//! devices and apps. Geosite has no tracker category — `category-public-tracker` is BitTorrent
//! trackers. HaGeZi publishes plain domain lists, so they are merged into one source-format rule
//! set whose single `domain_suffix` rule blocks each name and everything under it, as the lists
//! intend.
//!
//! **Nothing is trusted because it downloaded.** A rate-limit page or a truncated body written
//! over a good list would fail the next connect. So every download is staged beside the list,
//! handed to the core's own `CheckConfig`, and renamed over the old one only if the core read it.
//! Measured against this core: a missing file and an HTML page both fail the check; a binary
//! `.srs` and source versions 1 to 4 pass.
//!
//! **A running core picks up a replaced list by itself.** It watches a local rule set's directory
//! and reloads on a rename into place (sagernet/fswatch), so refreshing a list needs no reconnect.
//! Adding one to a config that did not name it does.

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::config::{self, BlockList, BlockOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum List {
    Ads,
    Trackers,
}

impl List {
    pub const ALL: [List; 2] = [List::Ads, List::Trackers];

    /// How the settings and the UI name it, and so how an error names it.
    pub fn name(self) -> &'static str {
        match self {
            List::Ads => "ad",
            List::Trackers => "tracker",
        }
    }

    fn tag(self) -> &'static str {
        match self {
            List::Ads => "block-ads",
            List::Trackers => "block-trackers",
        }
    }

    fn format(self) -> &'static str {
        match self {
            List::Ads => "binary",
            List::Trackers => "source",
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            List::Ads => "ads.srs",
            List::Trackers => "trackers.json",
        }
    }

    fn switched_on(self, block: &BlockOptions) -> bool {
        match self {
            List::Ads => block.ads,
            List::Trackers => block.trackers,
        }
    }
}

const ADS_URL: &str =
    "https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-category-ads-all.srs";

/// Every Native Tracker list HaGeZi publishes, not only the ones for the machine this runs on: in
/// proxy mode with "allow LAN" a phone or a TV can be using the listener too.
const TRACKER_URLS: [&str; 11] = [
    "https://raw.githubusercontent.com/hagezi/dns-blocklists/main/wildcard/native.amazon-onlydomains.txt",
    "https://raw.githubusercontent.com/hagezi/dns-blocklists/main/wildcard/native.apple-onlydomains.txt",
    "https://raw.githubusercontent.com/hagezi/dns-blocklists/main/wildcard/native.huawei-onlydomains.txt",
    "https://raw.githubusercontent.com/hagezi/dns-blocklists/main/wildcard/native.lgwebos-onlydomains.txt",
    "https://raw.githubusercontent.com/hagezi/dns-blocklists/main/wildcard/native.oppo-realme-onlydomains.txt",
    "https://raw.githubusercontent.com/hagezi/dns-blocklists/main/wildcard/native.roku-onlydomains.txt",
    "https://raw.githubusercontent.com/hagezi/dns-blocklists/main/wildcard/native.samsung-onlydomains.txt",
    // Not `native.tiktok.extended`, which blocks TikTok's own features, not only its tracking.
    "https://raw.githubusercontent.com/hagezi/dns-blocklists/main/wildcard/native.tiktok-onlydomains.txt",
    "https://raw.githubusercontent.com/hagezi/dns-blocklists/main/wildcard/native.vivo-onlydomains.txt",
    "https://raw.githubusercontent.com/hagezi/dns-blocklists/main/wildcard/native.winoffice-onlydomains.txt",
    "https://raw.githubusercontent.com/hagezi/dns-blocklists/main/wildcard/native.xiaomi-onlydomains.txt",
];

/// Where the lists live, inside the app's owner-only data directory.
const DIR: &str = "blocklists";

const TIMEOUT: Duration = Duration::from_secs(30);

/// No list asked for here is anywhere near this; a body that is did not come from where it was
/// asked for.
const MAX_BODY: u64 = 16 * 1024 * 1024;

pub fn path(dir: &Path, list: List) -> PathBuf {
    dir.join(DIR).join(list.file_name())
}

/// The lists switched on in `block` that are on disk, for the config to name.
pub fn on_disk(dir: &Path, block: &BlockOptions) -> Vec<BlockList> {
    List::ALL
        .into_iter()
        .filter(|list| list.switched_on(block))
        .map(|list| (list, path(dir, list)))
        .filter(|(_, path)| path.is_file())
        .map(|(list, path)| BlockList {
            tag: list.tag(),
            format: list.format(),
            path: path.to_string_lossy().into_owned(),
        })
        .collect()
}

/// Whether a list is on disk, and since when.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub list: List,
    /// Milliseconds since the epoch, as the frontend's `Date.now()`; `None` when not downloaded.
    pub updated_at: Option<u64>,
}

pub fn status(dir: &Path, list: List) -> Status {
    let updated_at = fs::metadata(path(dir, list))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64);
    Status { list, updated_at }
}

/// Downloads a list and returns what to write, through the proxy listener on `proxy_port` when
/// one is given. In VPN mode none is needed: the TUN carries a direct request.
///
/// Blocking: call it from `spawn_blocking`.
pub fn download(list: List, proxy_port: Option<u16>) -> Result<Vec<u8>, String> {
    let mut builder = ureq::AgentBuilder::new()
        .timeout(TIMEOUT)
        .user_agent(concat!("Nunya/", env!("CARGO_PKG_VERSION")));
    if let Some(port) = proxy_port {
        let proxy = ureq::Proxy::new(format!("http://127.0.0.1:{port}"))
            .map_err(|e| format!("bad proxy address: {e}"))?;
        builder = builder.proxy(proxy);
    }
    let agent = builder.build();

    match list {
        List::Ads => {
            let body = get(&agent, ADS_URL)?;
            // The format's magic number; anything else is an error page, and saying so here is
            // clearer than the core's "invalid rule-set file".
            if !body.starts_with(b"SRS") {
                return Err(format!("{ADS_URL} did not answer with a rule set"));
            }
            Ok(body)
        }
        List::Trackers => {
            let mut all = BTreeSet::new();
            // One missing list is an error, not a shorter list: that would be an anti-tracker
            // quietly blocking less than it says. The list already on disk stays in use.
            for url in TRACKER_URLS {
                let body = get(&agent, url)?;
                let text = String::from_utf8_lossy(&body);
                let names = domains(&text);
                if names.is_empty() {
                    return Err(format!("{url} did not answer with a domain list"));
                }
                all.extend(names);
            }
            Ok(tracker_rule_set(&all))
        }
    }
}

fn get(agent: &ureq::Agent, url: &str) -> Result<Vec<u8>, String> {
    let response = match agent.get(url).call() {
        Ok(r) => r,
        Err(ureq::Error::Status(status, _)) => return Err(format!("{url} answered {status}")),
        Err(e) => return Err(format!("{url}: {e}")),
    };
    let mut body = Vec::new();
    response
        .into_reader()
        .take(MAX_BODY)
        .read_to_end(&mut body)
        .map_err(|e| format!("{url}: {e}"))?;
    Ok(body)
}

/// The names in a HaGeZi domain list: one per line, `#` for comments.
///
/// Anything that is not a plain host name is dropped rather than passed to the core, where one
/// malformed entry would fail the whole list.
fn domains(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.trim_start_matches("*.").to_ascii_lowercase())
        .filter(|name| is_host(name))
        .collect()
}

fn is_host(name: &str) -> bool {
    name.len() <= 253
        && name.contains('.')
        && name.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

/// A source-format rule set: one rule matching each name and every name under it.
fn tracker_rule_set(names: &BTreeSet<String>) -> Vec<u8> {
    json!({ "version": 2, "rules": [{ "domain_suffix": names }] })
        .to_string()
        .into_bytes()
}

/// Writes a download beside the list it would replace, for the core to check first.
///
/// Same directory, so the rename that installs it stays on one filesystem and is atomic, and the
/// core — which watches that directory — never sees a half-written list.
pub fn stage(dir: &Path, list: List, bytes: &[u8]) -> Result<PathBuf, String> {
    let folder = dir.join(DIR);
    fs::create_dir_all(&folder).map_err(|e| format!("could not create {}: {e}", folder.display()))?;
    let staged = folder.join(format!("{}.{}.new", list.file_name(), std::process::id()));
    fs::write(&staged, bytes).map_err(|e| format!("could not write {}: {e}", staged.display()))?;
    Ok(staged)
}

/// A config that does nothing but make the core read one file as a rule set, so `CheckConfig`
/// says whether the core can use it.
pub fn check_config(list: List, staged: &Path) -> Value {
    let file = BlockList {
        tag: list.tag(),
        format: list.format(),
        path: staged.to_string_lossy().into_owned(),
    };
    json!({
        "route": {
            "rule_set": [config::block_rule_set(&file)],
            "rules": [{ "rule_set": [list.tag()], "action": "reject" }],
        }
    })
}

/// Puts a checked download in place of the list.
pub fn install(dir: &Path, list: List, staged: &Path) -> Result<(), String> {
    fs::rename(staged, path(dir, list)).map_err(|e| {
        let _ = fs::remove_file(staged);
        format!("could not save the {} list: {e}", list.name())
    })
}

pub fn discard(staged: &Path) {
    let _ = fs::remove_file(staged);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hagezi_list_yields_its_names_and_nothing_else() {
        let text = "# Title: HaGeZi's Apple Tracker\n#\ncstat.g.aaplimg.com\n\n  Metrics.Example.com \n*.wild.example.net\nnot a name\n<html>\nlocalhost\n";
        assert_eq!(
            domains(text),
            ["cstat.g.aaplimg.com", "metrics.example.com", "wild.example.net"]
        );
    }

    #[test]
    fn an_error_page_is_not_a_domain_list() {
        assert!(domains("<!DOCTYPE html><html><body>429 Too Many Requests</body></html>").is_empty());
    }

    #[test]
    fn the_tracker_rule_set_blocks_each_name_and_everything_under_it() {
        let names: BTreeSet<String> = ["b.example.com", "a.example.com"].map(String::from).into();
        let set: Value = serde_json::from_slice(&tracker_rule_set(&names)).unwrap();
        assert_eq!(set["rules"][0]["domain_suffix"], json!(["a.example.com", "b.example.com"]));
    }

    #[test]
    fn a_switch_whose_list_has_not_arrived_is_left_out() {
        let dir = std::env::temp_dir().join(format!("nunya-blocklists-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(DIR)).unwrap();
        fs::write(path(&dir, List::Trackers), "{}").unwrap();

        let both = BlockOptions { ads: true, trackers: true };
        let lists = on_disk(&dir, &both);
        assert_eq!(lists.len(), 1, "the ad list is not on disk");
        assert_eq!(lists[0].tag, "block-trackers");

        let none = BlockOptions::default();
        assert!(on_disk(&dir, &none).is_empty(), "a list on disk is not used while switched off");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn every_tracker_list_is_asked_for_once() {
        let unique: BTreeSet<&str> = TRACKER_URLS.into_iter().collect();
        assert_eq!(unique.len(), TRACKER_URLS.len());
    }
}
