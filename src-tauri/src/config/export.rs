//! A server as a whole client config, for another app: the Share sheet's JSON formats.
//!
//! A share link is what most clients scan, but some take only a config — v2rayN and v2rayNG's
//! custom configs (Xray), the sing-box apps — and a link loses what it has no parameter for. Both
//! are written by the same code that writes the configs this app runs (`config::build` and
//! `runtime::xray_outbound`), so an export cannot describe a different server from the one Nunya
//! connects to. What is ours alone is left out: the stats API, the TUN's interface name, the block
//! lists' paths on this disk and the bypass list, which is this machine's business.

use super::{runtime, BuildRequest, Mode, Profile, TransportKind};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    Xray,
    SingBox,
}

pub fn export(profile: &Profile, format: Format, dns: &str) -> Result<String, String> {
    let config = match format {
        Format::Xray => xray(profile, dns)?,
        Format::SingBox => sing_box(profile, dns)?,
    };
    serde_json::to_string_pretty(&config).map_err(|e| e.to_string())
}

/// A TUN config, since that is what the sing-box apps run; everything else is `config::build`'s.
fn sing_box(profile: &Profile, dns: &str) -> Result<Value, String> {
    if profile.transport.kind == TransportKind::Xhttp {
        return Err("sing-box has no XHTTP transport; share this server as Xray JSON or a link".into());
    }
    let mut config = super::build(&BuildRequest {
        profile: profile.clone(),
        mode: Mode::Vpn,
        tun: Default::default(),
        proxy: Default::default(),
        bypass: Vec::new(),
        dns: dns.to_string(),
        log_level: "warn".into(),
        block: Default::default(),
        block_lists: Vec::new(),
    });
    let root = config.as_object_mut().expect("the config is an object");
    root.remove("experimental");
    if let Some(tun) = root["inbounds"][0].as_object_mut() {
        tun.remove("interface_name");
    }
    Ok(config)
}

/// Local SOCKS on 10808 and HTTP on 10809, the ports v2rayN and v2rayNG use, so their own VPN
/// mode finds the config's listener where it looks for one.
fn xray(profile: &Profile, dns: &str) -> Result<Value, String> {
    let mut config = json!({
        "remarks": profile.name,
        "log": { "loglevel": "warning" },
        "inbounds": [
            { "tag": "socks", "listen": "127.0.0.1", "port": 10808, "protocol": "socks",
              "settings": { "udp": true },
              "sniffing": { "enabled": true, "destOverride": ["http", "tls", "quic"], "routeOnly": true } },
            { "tag": "http", "listen": "127.0.0.1", "port": 10809, "protocol": "http" }
        ],
        "outbounds": [
            runtime::xray_outbound(profile, super::tags::PROXY)?,
            { "tag": super::tags::DIRECT, "protocol": "freedom" }
        ],
        "routing": { "domainStrategy": "IPIfNonMatch", "rules": [
            // Written out rather than `geoip:private`, which fails wherever no geoip.dat is installed.
            { "type": "field", "ip": PRIVATE, "outboundTag": super::tags::DIRECT }
        ] }
    });
    // Xray reads DoH and plain addresses only; anything else is left to the client's own DNS
    // rather than written in a form it would refuse.
    if let Some(server) = xray_dns(dns) {
        config["dns"] = json!({ "servers": [server] });
    }
    Ok(config)
}

/// What sing-box's `ip_is_private` covers, so both exports keep the same traffic local.
const PRIVATE: &[&str] = &[
    "0.0.0.0/8", "10.0.0.0/8", "100.64.0.0/10", "127.0.0.0/8", "169.254.0.0/16", "172.16.0.0/12",
    "192.168.0.0/16", "224.0.0.0/4", "::1/128", "fc00::/7", "fe80::/10", "ff00::/8",
];

fn xray_dns(setting: &str) -> Option<&str> {
    if setting.starts_with("https://") {
        return Some(setting);
    }
    let bare = setting.strip_prefix("udp://").unwrap_or(setting);
    (!bare.is_empty() && !bare.contains("://")).then_some(bare)
}

#[cfg(test)]
mod tests;
