//! Updating the app: which release is newer, and replacing this copy with it.
//!
//! Tauri's updater does the replacing, because it verifies what it downloaded: every package is
//! signed in the release workflow, and the public key is compiled into the app (`plugins.updater`
//! in `tauri.conf.json`). A VPN client that installed whatever a download handed it would be one
//! tampered file away from running someone else's code with the user's traffic.
//!
//! What the updater cannot do is choose *which* release. Its endpoint is one URL, and GitHub has a
//! fixed URL only for the newest *stable* release, while every merge here publishes a beta. So the
//! releases are listed first, the newest this copy may take is picked (`newest`: betas only when
//! the user asked for them), and the updater is pointed at that release's `latest.json`, the
//! manifest the release workflow publishes beside the packages. The updater then refuses anything
//! not newer than this copy, so turning betas off never installs an older stable.
//!
//! Both requests go through the local listener when one is up in proxy mode, as the block lists
//! do: on a network that blocks GitHub, the tunnel is how the update arrives.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Newest first, so the few releases worth considering are on the first page.
const RELEASES: &str = "https://api.github.com/repos/nunyavpn/nunya/releases?per_page=30";
/// What the release workflow names the updater's manifest.
const MANIFEST: &str = "latest.json";
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// The parts of a GitHub release this reads.
#[derive(Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

/// An update found and not yet installed: what the frontend shows.
#[derive(Debug, Clone, Serialize)]
pub struct Available {
    pub version: String,
    pub prerelease: bool,
    pub notes: Option<String>,
}

/// The newest release this copy may update to, with the address of its manifest.
///
/// Drafts never; betas (GitHub pre-releases) only with `beta`; and only releases that carry a
/// manifest, since one without (every release before updates existed) has nothing to install.
/// Newest by version, not by date: a stable fix to an older line can be published after a newer
/// release.
pub fn newest(releases: &[Release], beta: bool) -> Option<(&Release, &str)> {
    releases
        .iter()
        .filter(|r| !r.draft && (beta || !r.prerelease))
        .filter_map(|r| {
            let version = semver::Version::parse(r.tag_name.strip_prefix('v')?).ok()?;
            let manifest = r.assets.iter().find(|a| a.name == MANIFEST)?;
            Some((version, r, manifest.browser_download_url.as_str()))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, release, url)| (release, url))
}

/// Lists the published releases, through the local listener when `proxy_port` names one.
pub fn releases(proxy_port: Option<u16>) -> Result<Vec<Release>, String> {
    let proxy = proxy_port
        .map(|port| ureq::Proxy::new(&format!("http://127.0.0.1:{port}")))
        .transpose()
        .map_err(|e| format!("bad proxy address: {e}"))?;
    let agent: ureq::Agent = ureq::Agent::config_builder()
        // Explicit either way: ureq would otherwise take a proxy from the environment.
        .proxy(proxy)
        .timeout_global(Some(TIMEOUT))
        // GitHub's API refuses a request without one.
        .user_agent(concat!("Nunya/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let body = agent
        .get(RELEASES)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("could not list releases: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("could not read the release list: {e}"))?;
    serde_json::from_str(&body).map_err(|e| format!("could not read the release list: {e}"))
}

/// An update found by the last check, and its package once downloaded. The package is kept in
/// memory until it is installed: it is tens of megabytes, once, and a file on disk would be one
/// more thing to verify again before use.
pub struct Staged {
    pub update: tauri_plugin_updater::Update,
    pub bytes: Option<Vec<u8>>,
}

#[cfg(test)]
mod tests;
