//! Couples sing-box routing with the Xray engine already embedded in nunya-core.
//!
//! XHTTP outbounds sit behind authenticated loopback SOCKS listeners. sing-box still owns TUN,
//! DNS, bypass rules and counters; the core owns both engines' lifetime and binds Xray's egress
//! to the physical interface. Every start, check and probe goes through the same preparation.

use super::{Profile, Protocol, TransportKind};
use crate::platform::Engine;
use crate::rpc::{gen, method};
use serde::Serialize;
use serde_json::{json, Value};
use std::net::TcpListener;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Runtime {
    pub core: Value,
    pub xray: Option<Value>,
}

impl Runtime {
    pub fn load_request(&self) -> gen::LoadConfigReq {
        gen::LoadConfigReq {
            core_config: Some(self.core.to_string()),
            need_extra_process: Some(false),
            need_xray: Some(self.xray.is_some()),
            xray_config: self.xray.as_ref().map(Value::to_string),
            // Resolve the server outside the running TUN, through sing-box's direct DNS transport.
            xray_outbound_dns_strategy: Some("UseIP".into()),
            ..Default::default()
        }
    }

    pub fn test_request(&self) -> gen::TestReq {
        gen::TestReq {
            config: Some(self.core.to_string()),
            need_xray: Some(self.xray.is_some()),
            xray_config: self.xray.as_ref().map(Value::to_string),
            xray_outbound_dns_strategy: Some("UseIP".into()),
            ..Default::default()
        }
    }

    pub async fn check(&self, core: &Engine) -> Result<(), String> {
        // CheckConfig returns after checking Xray when need_xray is true, so validate both halves.
        for xray in [false, true] {
            if xray && self.xray.is_none() {
                continue;
            }
            let resp: gen::ErrorResp = core
                .call(
                    method::CHECK_CONFIG,
                    &gen::LoadConfigReq {
                        need_xray: Some(xray),
                        ..self.load_request()
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            if let Some(error) = resp.error.filter(|e| !e.is_empty()) {
                return Err(error);
            }
        }
        Ok(())
    }

    pub fn pretty(&self) -> Result<String, String> {
        // Preserve the existing diagnostics shape when only sing-box is needed.
        if self.xray.is_none() {
            serde_json::to_string_pretty(&self.core)
        } else {
            serde_json::to_string_pretty(self)
        }
        .map_err(|e| e.to_string())
    }
}

pub fn build(req: &super::BuildRequest) -> Result<Runtime, String> {
    prepare(
        super::build(req),
        std::slice::from_ref(&req.profile),
        &[super::tags::PROXY.into()],
    )
}

pub fn build_test(profiles: &[Profile]) -> Result<(Runtime, Vec<String>), String> {
    let (core, tags) = super::build_test(profiles);
    Ok((prepare(core, profiles, &tags)?, tags))
}

pub fn build_probe(profiles: &[Profile], ports: &[u16]) -> Result<Runtime, String> {
    let tags = (0..profiles.len()).map(super::test_tag).collect::<Vec<_>>();
    prepare(super::build_probe(profiles, ports), profiles, &tags)
}

fn prepare(mut core: Value, profiles: &[Profile], tags: &[String]) -> Result<Runtime, String> {
    let mut inbounds = Vec::new();
    let mut outbounds = Vec::new();
    let mut rules = Vec::new();
    let mut held = Vec::new();
    for (profile, tag) in profiles.iter().zip(tags) {
        if profile.protocol == Protocol::Wireguard || profile.transport.kind != TransportKind::Xhttp
        {
            continue;
        }
        let outbound = xhttp_outbound(profile, tag)?;
        let listener =
            TcpListener::bind("127.0.0.1:0").map_err(|e| format!("XHTTP bridge: {e}"))?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        held.push(listener);
        let mut secret = [0u8; 32];
        rustls::crypto::ring::default_provider()
            .secure_random
            .fill(&mut secret)
            .map_err(|_| "could not generate XHTTP bridge credentials")?;
        let password: String = secret.iter().map(|b| format!("{b:02x}")).collect();
        inbounds.push(json!({
            "tag": tag, "listen": "127.0.0.1", "port": port, "protocol": "socks",
            "settings": { "auth": "password", "accounts": [{ "user": "nunya", "pass": password }],
                "udp": true, "ip": "127.0.0.1" }
        }));
        outbounds.push(outbound);
        rules.push(json!({ "type": "field", "inboundTag": [tag], "outboundTag": tag }));
        let target = core["outbounds"]
            .as_array_mut()
            .and_then(|items| items.iter_mut().find(|o| o["tag"].as_str() == Some(tag)))
            .ok_or_else(|| format!("missing XHTTP outbound {tag}"))?;
        *target = json!({ "type": "socks", "tag": tag, "server": "127.0.0.1", "server_port": port,
            "version": "5", "username": "nunya", "password": password });
    }
    // ponytail: ports are released before the core binds them; a collision fails startup. Use
    // inherited listeners if contention becomes measurable. Random auth restricts access to the TCP bridge.
    let xray = (!outbounds.is_empty()).then(|| {
        json!({
            "log": { "loglevel": "warning" }, "inbounds": inbounds, "outbounds": outbounds,
            "routing": { "rules": rules }
        })
    });
    Ok(Runtime { core, xray })
}

fn xhttp_outbound(p: &Profile, tag: &str) -> Result<Value, String> {
    let t = &p.transport;
    let mode = if t.mode.is_empty() { "auto" } else { &t.mode };
    if !["auto", "packet-up", "stream-up", "stream-one"].contains(&mode) {
        return Err(format!("XHTTP mode {mode:?} is not supported"));
    }
    // Match share.ts. Xray silently ignores unknown keys; never make a misspelling look supported.
    const EXTRA: &[&str] = &[
        "host",
        "path",
        "mode",
        "headers",
        "xPaddingBytes",
        "noGRPCHeader",
        "noSSEHeader",
        "scMaxEachPostBytes",
        "scMinPostsIntervalMs",
        "scMaxBufferedPosts",
        "scStreamUpServerSecs",
        "xmux",
    ];
    for key in t.extra.keys() {
        if !EXTRA.contains(&key.as_str()) {
            return Err(format!("XHTTP extra option {key:?} is not supported"));
        }
    }
    if let Some(xmux) = t.extra.get("xmux") {
        let xmux = xmux.as_object().ok_or("XHTTP xmux must be a JSON object")?;
        for key in xmux.keys() {
            if ![
                "maxConcurrency",
                "maxConnections",
                "cMaxReuseTimes",
                "hMaxRequestTimes",
                "hMaxReusableSecs",
                "hKeepAlivePeriod",
            ]
            .contains(&key.as_str())
            {
                return Err(format!("XHTTP xmux option {key:?} is not supported"));
            }
        }
    }
    if p.protocol == Protocol::Vless && !p.flow.is_empty() {
        return Err("XHTTP does not support VLESS flow; remove the flow setting".into());
    }
    let settings = match p.protocol {
        Protocol::Vless => json!({ "vnext": [{ "address": p.server, "port": p.port,
            "users": [{ "id": p.uuid, "encryption": "none" }] }] }),
        Protocol::Vmess => {
            if p.alter_id != 0 {
                return Err("XHTTP requires VMess AEAD (alterId 0)".into());
            }
            json!({ "vnext": [{ "address": p.server, "port": p.port, "users": [{ "id": p.uuid,
                "security": if p.security.is_empty() { "auto" } else { &p.security } }] }] })
        }
        Protocol::Trojan => {
            json!({ "servers": [{ "address": p.server, "port": p.port, "password": p.password }] })
        }
        Protocol::Wireguard => return Err("WireGuard cannot use XHTTP".into()),
    };
    let mut stream = json!({ "network": "xhttp", "security": "none", "xhttpSettings": {
        "host": t.host, "path": if t.path.is_empty() { "/" } else { &t.path }, "mode": mode, "extra": t.extra
    }});
    if p.tls.enabled {
        let tls = &p.tls;
        if let Some(reality) = &tls.reality {
            stream["security"] = json!("reality");
            stream["realitySettings"] = json!({ "serverName": tls.sni, "publicKey": reality.public_key,
                "shortId": reality.short_id, "fingerprint": if tls.fingerprint.is_empty() { "chrome" } else { &tls.fingerprint } });
        } else {
            stream["security"] = json!("tls");
            stream["tlsSettings"] = json!({ "serverName": tls.sni, "allowInsecure": tls.insecure });
            if !tls.alpn.is_empty() {
                stream["tlsSettings"]["alpn"] = json!(tls.alpn);
            }
            if !tls.fingerprint.is_empty() {
                stream["tlsSettings"]["fingerprint"] = json!(tls.fingerprint);
            }
        }
    }
    Ok(
        json!({ "tag": tag, "protocol": p.protocol, "settings": settings, "streamSettings": stream }),
    )
}

#[cfg(test)]
mod tests;
