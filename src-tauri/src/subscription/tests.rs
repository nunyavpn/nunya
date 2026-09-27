use super::*;

/// Public lists are plain text, unencoded, and enormous — one on GitHub carries over twenty
/// thousand links in 4.7 MB. Every line has to come through, and quickly.
#[test]
fn a_public_list_of_tens_of_thousands_of_plain_links_is_read_whole() {
    let body: String = (0..25_000)
        .map(|i| format!("vless://00000000-0000-4000-8000-{i:012}@h{i}.example.net:443?type=ws&security=tls#By list 🧬 {i}\n"))
        .collect();
    assert!(body.len() < MAX_BODY);
    let started = std::time::Instant::now();
    let fetched = assemble(None, None, &body).unwrap();
    assert_eq!(fetched.links.len(), 25_000);
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
}

#[test]
fn plain_http_is_refused_because_it_leaks_credentials() {
    let err = check_url("http://example.net/sub").unwrap_err();
    assert!(err.to_string().contains("plain HTTP"), "{err}");
}

#[test]
fn non_http_schemes_are_refused() {
    assert!(check_url("file:///etc/passwd").is_err());
    assert!(check_url("ftp://example.net").is_err());
    assert!(check_url("").is_err());
}

#[test]
fn https_is_accepted() {
    check_url("https://example.net/sub").unwrap();
    check_url("  https://example.net/sub  ").unwrap();
}

#[test]
fn userinfo_sums_both_directions_against_the_allowance() {
    let q = parse_userinfo("upload=100; download=900; total=5000; expire=1700000000");
    assert_eq!(q.used_bytes, 1000);
    assert_eq!(q.total_bytes, 5000);
    assert_eq!(q.resets_at, Some(1_700_000_000_000));
}

#[test]
fn userinfo_tolerates_missing_and_junk_fields() {
    let q = parse_userinfo("download=42; total=; nonsense; expire=0");
    assert_eq!(q.used_bytes, 42);
    assert_eq!(q.total_bytes, 0);
    // An expire of 0 means "never", not "1970".
    assert_eq!(q.resets_at, None);
}

#[test]
fn decodes_standard_base64() {
    // "vless://a\nvmess://b"
    let encoded = "dmxlc3M6Ly9hCnZtZXNzOi8vYg==";
    assert_eq!(decode_body(encoded), "vless://a\nvmess://b");
}

#[test]
fn decodes_url_safe_base64_without_padding() {
    let encoded = "dmxlc3M6Ly9hCnZtZXNzOi8vYg";
    assert_eq!(decode_body(encoded), "vless://a\nvmess://b");
}

#[test]
fn leaves_a_plain_list_alone() {
    let plain = "vless://a\nvmess://b";
    assert_eq!(decode_body(plain), plain);
}

/// A body that decodes as valid base64 but yields no links is not a subscription; keeping the
/// original text gives the line-level parser a chance to report something useful.
#[test]
fn keeps_the_original_when_decoding_produces_no_links() {
    assert_eq!(decode_body("abcd"), "abcd");
}

#[test]
fn extracts_links_and_skips_comments_and_blanks() {
    let body = "# my servers\n\nvless://a\n   \ntrojan://b\nnot a link\n";
    assert_eq!(extract_links(body), vec!["vless://a", "trojan://b"]);
}

/// The exact shape a panel returns: a base64 body, an allowance header and a title.
#[test]
fn assembles_a_real_subscription_response() {
    let links = [
        "vless://8f3c9d2e-4a17-4b8e-9c21-7de5f0a63b14@de4.example.net:443?security=reality#DE-4",
        "vless://8f3c9d2e-4a17-4b8e-9c21-7de5f0a63b15@nl1.example.net:443?security=tls#NL-1",
        "snell://nope@jp.example.net:8388",
        "# a comment line",
    ]
    .join("\n");

    // base64 of the above, as a panel would send it
    let body = encode_for_test(links.as_bytes());
    let quota = parse_userinfo(
        "upload=10737418240; download=79725330432; total=214748364800; expire=1790000000",
    );

    let fetched = assemble(Some(quota), Some("Example Nodes".into()), &body).unwrap();

    // The comment is dropped; the unsupported protocol is kept for the frontend to reject by
    // name, rather than disappearing silently here.
    assert_eq!(fetched.links.len(), 3);
    assert!(fetched.links[0].starts_with("vless://"));
    assert!(fetched.links[2].starts_with("snell://"));

    let q = fetched.quota.unwrap();
    assert_eq!(q.used_bytes, 90_462_748_672);
    assert_eq!(q.total_bytes, 214_748_364_800);
    assert_eq!(q.resets_at, Some(1_790_000_000_000));
    assert_eq!(fetched.title.as_deref(), Some("Example Nodes"));
}

#[test]
fn a_body_with_no_links_is_an_error_rather_than_an_empty_list() {
    let err = assemble(None, None, "<html>login required</html>").unwrap_err();
    assert!(matches!(err, SubscriptionError::Empty));
}

/// Minimal encoder, so the test above can build its own fixture.
fn encode_for_test(input: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - i * 6)) & 0x3F) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[test]
fn base64_rejects_invalid_alphabet() {
    assert!(base64_decode("!!!!").is_none());
    assert!(base64_decode("").is_none());
}

/// The shape a BPB panel serves to `?app=xray`: an array of whole client configurations, each
/// carrying one `proxy` outbound. Trimmed of the inbounds, routing and DNS that come with it
/// and are not read here.
fn bpb_array() -> &'static str {
    r#"[
          {
            "remarks": "💦 1. VLESS - Domain : 443",
            "outbounds": [
              {
                "protocol": "vless",
                "settings": {
                  "vnext": [{
                    "address": "edge.example.net",
                    "port": 443,
                    "users": [{ "id": "455c35ab-42b9-43d7-b63c-ea915c2c72ab", "encryption": "none" }]
                  }]
                },
                "streamSettings": {
                  "network": "ws",
                  "wsSettings": { "host": "edge.example.net", "path": "/vl/8NjpyzwBr4ARYZEeGqW?ed=2560" },
                  "security": "tls",
                  "tlsSettings": { "serverName": "edge.example.net", "fingerprint": "chrome", "alpn": ["http/1.1"] }
                },
                "tag": "proxy"
              },
              { "protocol": "freedom", "tag": "direct" }
            ]
          },
          {
            "remarks": "💦 1. Trojan - Domain : 443",
            "outbounds": [
              {
                "protocol": "trojan",
                "settings": { "servers": [{ "address": "edge.example.net", "port": 443, "password": "E1wy;,GYGPPhYEYXn" }] },
                "streamSettings": {
                  "network": "ws",
                  "wsSettings": { "host": "edge.example.net", "path": "/tr/FHxj7oXIlRMTvceFHCN?ed=2560" },
                  "security": "tls",
                  "tlsSettings": { "serverName": "edge.example.net", "fingerprint": "chrome" }
                },
                "tag": "proxy"
              }
            ]
          },
          {
            "remarks": "💦 Best Ping",
            "outbounds": [
              { "protocol": "vless", "tag": "proxy-1" },
              { "protocol": "trojan", "tag": "proxy-2" }
            ],
            "routing": { "balancers": [{ "tag": "all-proxies", "selector": ["proxy"] }] }
          }
        ]"#
}

#[test]
fn an_xray_configuration_array_becomes_share_links() {
    let links = extract_links(bpb_array());
    assert_eq!(links.len(), 2, "one per configuration with a proxy outbound");

    let vless = &links[0];
    assert!(vless.starts_with("vless://455c35ab-42b9-43d7-b63c-ea915c2c72ab@edge.example.net:443?"));
    assert!(vless.contains("type=ws"));
    assert!(vless.contains("security=tls"));
    assert!(vless.contains("host=edge.example.net"));
    assert!(vless.contains("sni=edge.example.net"));
    assert!(vless.contains("fp=chrome"));
    assert!(vless.contains("alpn=http%2F1.1"));
}

/// The path carries a query string of its own, which has to survive being written into one.
#[test]
fn a_websocket_path_is_escaped_rather_than_ending_the_query() {
    let links = extract_links(bpb_array());
    assert!(links[0].contains("path=%2Fvl%2F8NjpyzwBr4ARYZEeGqW%3Fed%3D2560"));
}

/// Emitted, not dropped. The frontend answers "Trojan is not supported yet"; a subscription
/// that silently lost half its entries could not.
#[test]
fn a_protocol_this_build_cannot_run_is_still_emitted_under_its_own_scheme() {
    let links = extract_links(bpb_array());
    let trojan = &links[1];
    assert!(trojan.starts_with("trojan://"));
    // The password is userinfo, so its punctuation has to be escaped.
    assert!(trojan.contains("E1wy%3B%2CGYGPPhYEYXn@edge.example.net:443"));
}

/// A chain traverses two hops. Reducing it to its last one would hand back something the
/// entry's own name contradicts, so it is refused instead.
#[test]
fn a_chained_configuration_is_refused_rather_than_flattened() {
    let body = r#"[{
          "remarks": "💦 1 - WoW",
          "outbounds": [
            { "protocol": "wireguard", "tag": "chain",
              "streamSettings": { "sockopt": { "dialerProxy": "proxy" } },
              "settings": { "peers": [{ "endpoint": "162.159.192.1:2408" }] } },
            { "protocol": "wireguard", "tag": "proxy",
              "settings": { "peers": [{ "endpoint": "engage.cloudflareclient.com:2408" }] } }
          ]
        }]"#;
    let links = extract_links(body);
    assert_eq!(links.len(), 1);
    assert!(links[0].starts_with("chain://162.159.192.1:2408#"), "got {}", links[0]);
}

/// A "Best Ping" entry holding exactly one proxy is a real configuration, not a duplicate of
/// anything, so the balancer rule must turn on the number of candidates rather than on the
/// tag's spelling. Skipping it on the tag alone lost a config the provider published.
#[test]
fn a_selector_over_a_single_proxy_is_still_that_proxy() {
    let body = r#"[{
          "remarks": "Best Ping",
          "outbounds": [{ "protocol": "wireguard", "tag": "proxy-1",
            "settings": { "peers": [{ "endpoint": "engage.cloudflareclient.com:2408", "publicKey": "k" }],
                          "address": ["172.16.0.2/32"], "secretKey": "s" } }],
          "routing": { "balancers": [{ "tag": "all", "selector": ["proxy"] }] }
        }]"#;
    let links = extract_links(body);
    assert_eq!(links.len(), 1, "got {links:?}");
    assert!(links[0].starts_with("wireguard://s@engage.cloudflareclient.com:2408?"));
}

/// A balancer has no `proxy` outbound, and the servers behind it are listed individually in
/// the same array — importing them again through the balancer would duplicate every one.
#[test]
fn a_balancer_configuration_contributes_nothing() {
    let links = extract_links(bpb_array());
    assert!(
        !links.iter().any(|l| l.contains("Best")),
        "the balancer entry should be skipped, got {links:?}"
    );
}

#[test]
fn remarks_become_the_name_and_survive_their_emoji() {
    let links = extract_links(bpb_array());
    assert!(links[0].ends_with("#%F0%9F%92%A6%201.%20VLESS%20-%20Domain%20%3A%20443"));
}

/// The other thing a panel may serve at a subscription path: its own settings object. It has
/// no outbounds, and the `://` inside its DNS addresses must not be read as a server.
#[test]
fn json_that_holds_no_outbounds_yields_nothing_rather_than_fragments() {
    let body = r#"{"remoteDNS":"https://8.8.8.8/dns-query","localDNS":"8.8.8.8"}"#;
    assert!(extract_links(body).is_empty());
}

#[test]
fn a_plain_list_of_share_links_is_untouched() {
    let body = "vless://uuid@example.net:443?type=ws#One\n# a comment\n\nvmess://blob\n";
    assert_eq!(
        extract_links(body),
        vec!["vless://uuid@example.net:443?type=ws#One", "vmess://blob"]
    );
}

#[test]
fn an_ipv6_address_keeps_its_brackets_so_the_port_is_readable() {
    let body = r#"{"outbounds":[{"protocol":"vless","tag":"proxy","settings":{"vnext":[{"address":"2001:db8::1","port":443,"users":[{"id":"u"}]}]}}]}"#;
    assert_eq!(extract_links(body), vec!["vless://u@[2001:db8::1]:443?type=tcp&security=none"]);
}

/// A WARP subscription is entirely WireGuard, which keeps its peer under `peers` and its
/// identity in keys rather than a user record — so it shares no field with the other
/// protocols and once fell through the address lookup and vanished entirely.
#[test]
fn a_wireguard_outbound_becomes_a_link_carrying_its_keys() {
    let body = r#"[{
          "remarks": "💦 1 - Warp",
          "outbounds": [{
            "protocol": "wireguard",
            "tag": "proxy",
            "settings": {
              "address": ["172.16.0.2/32", "2606:4700:110::1/128"],
              "mtu": 1280,
              "peers": [{
                "endpoint": "engage.cloudflareclient.com:2408",
                "publicKey": "bmXOC+F1FxEMF9dyiK2H5/1SUtzH0JuVo51h2wPfgyo=",
                "keepAlive": 5
              }],
              "reserved": [216, 253, 3],
              "secretKey": "f7m/C8NHWPWIkGAbxTBAMhYHlzu3Ya7lSCbOSqfGu68="
            }
          }]
        }]"#;
    let links = extract_links(body);
    assert_eq!(links.len(), 1);
    let link = &links[0];

    // The private key is userinfo, so its `/` and `+` have to be escaped.
    assert!(
        link.starts_with("wireguard://f7m%2FC8NHWPWIkGAbxTBAMhYHlzu3Ya7lSCbOSqfGu68%3D@engage.cloudflareclient.com:2408?"),
        "got {link}"
    );
    assert!(link.contains("publickey=bmXOC%2BF1FxEMF9dyiK2H5%2F1SUtzH0JuVo51h2wPfgyo%3D"));
    assert!(link.contains("address=172.16.0.2%2F32%2C2606%3A4700%3A110%3A%3A1%2F128"));
    // WARP's client id. A wrong value is dropped by the server without an error, so losing
    // it here would produce a tunnel that comes up and carries nothing.
    assert!(link.contains("reserved=216%2C253%2C3"));
    assert!(link.contains("mtu=1280"));
    assert!(link.contains("keepalive=5"));
}

/// An endpoint whose shape is unknown still has to reach the frontend to be refused by name,
/// or a subscription made only of them looks like a broken link.
#[test]
fn a_protocol_with_an_unfamiliar_shape_is_named_rather_than_dropped() {
    let body = r#"[{
          "remarks": "Something new",
          "outbounds": [{ "protocol": "hysteria2", "tag": "proxy",
            "settings": { "servers": [{ "address": "h2.example.net", "port": 443 }] } }]
        }]"#;
    let links = extract_links(body);
    assert_eq!(links.len(), 1);
    assert!(links[0].starts_with("hysteria2://h2.example.net#"), "got {}", links[0]);
}

#[test]
fn reality_keys_are_read_from_their_own_settings_object() {
    let body = r#"{"outbounds":[{"protocol":"vless","tag":"proxy",
          "settings":{"vnext":[{"address":"a.example.net","port":443,"users":[{"id":"u","flow":"xtls-rprx-vision"}]}]},
          "streamSettings":{"network":"tcp","security":"reality",
            "realitySettings":{"serverName":"www.example.net","publicKey":"PBK","shortId":"ab","fingerprint":"chrome"}}}]}"#;
    let link = &extract_links(body)[0];
    assert!(link.contains("security=reality"));
    assert!(link.contains("pbk=PBK"));
    assert!(link.contains("sid=ab"));
    assert!(link.contains("flow=xtls-rprx-vision"));
}

// ---------------------------------------------------------- the links a BPB panel offers

/// One BPB subscription, three ways, as its panel hands them out: an import link for sing-box
/// wrapping the address raw, and the address itself asked for Clash and for Xray. Each carries
/// the panel's display name in its fragment.
const SING_BOX_LINK: &str = "sing-box://import-remote-profile?url=https://edge.example.net/Xq3vT9pLmN2wR8sK/sub/normal?app=sing-box#%F0%9F%92%A6%20BPB%20Normal";
const CLASH_LINK: &str =
    "https://edge.example.net/Xq3vT9pLmN2wR8sK/sub/normal?app=clash#%F0%9F%92%A6%20BPB%20Normal";
const XRAY_LINK: &str =
    "https://edge.example.net/Xq3vT9pLmN2wR8sK/sub/normal?app=xray#%F0%9F%92%A6%20BPB%20Normal";

/// What BPB serves to `?app=sing-box`: one configuration holding every server as an outbound,
/// beside a selector, a `urltest` "Best Ping" group over the same servers, and `direct`. The
/// same two servers as `bpb_array`, trimmed of the inbounds, routing and most of the DNS.
fn bpb_sing_box() -> &'static str {
    r#"{
          "dns": { "servers": [{ "type": "https", "server": "8.8.8.8", "detour": "✅ Selector", "tag": "dns-remote" }] },
          "outbounds": [
            {
              "tag": "💦 1. VLESS - Domain : 443",
              "type": "vless",
              "server": "edge.example.net",
              "server_port": 443,
              "tcp_fast_open": false,
              "uuid": "455c35ab-42b9-43d7-b63c-ea915c2c72ab",
              "packet_encoding": "",
              "network": "tcp",
              "tls": {
                "enabled": true,
                "server_name": "edge.example.net",
                "record_fragment": false,
                "insecure": false,
                "alpn": ["http/1.1"],
                "utls": { "enabled": true, "fingerprint": "chrome" }
              },
              "transport": {
                "type": "ws",
                "path": "/vl/8NjpyzwBr4ARYZEeGqW",
                "max_early_data": 2560,
                "early_data_header_name": "Sec-WebSocket-Protocol",
                "headers": { "Host": "edge.example.net" }
              },
              "domain_resolver": "dns-direct"
            },
            {
              "tag": "💦 1. Trojan - Domain : 443",
              "type": "trojan",
              "server": "edge.example.net",
              "server_port": 443,
              "password": "E1wy;,GYGPPhYEYXn",
              "network": "tcp",
              "tls": {
                "enabled": true,
                "server_name": "edge.example.net",
                "insecure": false,
                "utls": { "enabled": true, "fingerprint": "chrome" }
              },
              "transport": {
                "type": "ws",
                "path": "/tr/FHxj7oXIlRMTvceFHCN",
                "max_early_data": 2560,
                "early_data_header_name": "Sec-WebSocket-Protocol",
                "headers": { "Host": "edge.example.net" }
              }
            },
            { "type": "selector", "tag": "✅ Selector",
              "outbounds": ["💦 Best Ping 🚀", "💦 1. VLESS - Domain : 443", "💦 1. Trojan - Domain : 443"] },
            { "type": "direct", "tag": "direct" },
            { "type": "urltest", "tag": "💦 Best Ping 🚀",
              "outbounds": ["💦 1. VLESS - Domain : 443", "💦 1. Trojan - Domain : 443"],
              "url": "https://www.gstatic.com/generate_204" }
          ]
        }"#
}

/// What BPB serves to `?app=clash`: a Clash configuration, but as JSON rather than YAML, with
/// the servers under `proxies` and the groups over them under `proxy-groups`. The same two
/// servers again.
fn bpb_clash() -> &'static str {
    r#"{
          "mixed-port": 7890,
          "dns": { "nameserver": ["https://8.8.8.8/dns-query"] },
          "proxies": [
            {
              "name": "💦 1. VLESS - Domain : 443",
              "type": "vless",
              "server": "edge.example.net",
              "port": 443,
              "ip-version": "ipv4",
              "tfo": false,
              "udp": false,
              "uuid": "455c35ab-42b9-43d7-b63c-ea915c2c72ab",
              "packet-encoding": "",
              "encryption": "",
              "tls": true,
              "servername": "edge.example.net",
              "client-fingerprint": "chrome",
              "skip-cert-verify": false,
              "alpn": ["http/1.1"],
              "network": "ws",
              "ws-opts": {
                "path": "/vl/8NjpyzwBr4ARYZEeGqW",
                "max-early-data": 2560,
                "early-data-header-name": "Sec-WebSocket-Protocol",
                "headers": { "Host": "edge.example.net" }
              }
            },
            {
              "name": "💦 1. Trojan - Domain : 443",
              "type": "trojan",
              "server": "edge.example.net",
              "port": 443,
              "password": "E1wy;,GYGPPhYEYXn",
              "tls": true,
              "sni": "edge.example.net",
              "client-fingerprint": "chrome",
              "skip-cert-verify": false,
              "network": "ws",
              "ws-opts": {
                "path": "/tr/FHxj7oXIlRMTvceFHCN",
                "max-early-data": 2560,
                "early-data-header-name": "Sec-WebSocket-Protocol",
                "headers": { "Host": "edge.example.net" }
              }
            }
          ],
          "proxy-groups": [
            { "name": "✅ Selector", "type": "select",
              "proxies": ["💦 Best Ping 🚀", "💦 1. VLESS - Domain : 443", "💦 1. Trojan - Domain : 443"] },
            { "name": "💦 Best Ping 🚀", "type": "url-test", "url": "https://www.gstatic.com/generate_204",
              "proxies": ["💦 1. VLESS - Domain : 443", "💦 1. Trojan - Domain : 443"] }
          ]
        }"#
}

#[test]
fn a_sing_box_import_link_is_fetched_at_the_address_it_carries() {
    assert_eq!(
        resolve(SING_BOX_LINK).unwrap(),
        "https://edge.example.net/Xq3vT9pLmN2wR8sK/sub/normal?app=sing-box"
    );
}

/// The fragment is the panel's name for the subscription, not part of its address.
#[test]
fn the_clash_and_xray_links_are_fetched_as_written_less_their_name() {
    assert_eq!(
        resolve(CLASH_LINK).unwrap(),
        "https://edge.example.net/Xq3vT9pLmN2wR8sK/sub/normal?app=clash"
    );
    assert_eq!(
        resolve(XRAY_LINK).unwrap(),
        "https://edge.example.net/Xq3vT9pLmN2wR8sK/sub/normal?app=xray"
    );
}

/// Each link asks for a different app's format, and each format comes back as the same two
/// servers — the whole path, from what the user pasted to the links the frontend parses.
#[test]
fn the_three_links_a_bpb_panel_offers_all_import() {
    for (link, app, body) in [
        (SING_BOX_LINK, "sing-box", bpb_sing_box()),
        (CLASH_LINK, "clash", bpb_clash()),
        (XRAY_LINK, "xray", bpb_array()),
    ] {
        assert!(resolve(link).unwrap().ends_with(&format!("?app={app}")), "{link}");
        let fetched = assemble(None, None, body).unwrap();
        assert_eq!(fetched.links.len(), 2, "{app}: {:?}", fetched.links);
        assert!(fetched.links[0].starts_with("vless://"), "{app}");
        assert!(fetched.links[1].starts_with("trojan://"), "{app}");
    }
}

/// Xray, sing-box and Clash spell one server three ways. Which button the user copied must not
/// change what gets imported, so all three have to come out as the very same links.
#[test]
fn every_format_of_one_subscription_imports_the_same_servers() {
    let xray = extract_links(bpb_array());
    assert_eq!(extract_links(bpb_sing_box()), xray);
    assert_eq!(extract_links(bpb_clash()), xray);
}

/// sing-box and Clash keep early data in two fields beside the path; the link form carries it
/// inside the path as `?ed=N`, where the frontend lifts it back out.
#[test]
fn early_data_is_written_back_into_the_path_the_way_links_carry_it() {
    for body in [bpb_sing_box(), bpb_clash()] {
        let links = extract_links(body);
        assert!(links[0].contains("path=%2Fvl%2F8NjpyzwBr4ARYZEeGqW%3Fed%3D2560"), "{}", links[0]);
    }
}

#[test]
fn early_data_under_another_header_names_it_in_the_path() {
    let body = r#"{"outbounds":[{"type":"vless","tag":"x","server":"a.example.net","server_port":443,"uuid":"u",
          "transport":{"type":"ws","path":"/p","max_early_data":2048,"early_data_header_name":"X-Early"}}]}"#;
    assert!(extract_links(body)[0].contains("path=%2Fp%3Fed%3D2048%26eh%3DX-Early"));
}

/// The groups are not servers, and every member of one is listed on its own already.
#[test]
fn sing_box_selectors_and_url_tests_are_not_imported_as_servers() {
    let links = extract_links(bpb_sing_box());
    assert!(!links.iter().any(|l| l.contains("Best") || l.contains("Selector")), "{links:?}");
}

/// Clash's groups hold a health-check URL, which must not be read as a server either.
#[test]
fn clash_proxy_groups_are_not_imported_as_servers() {
    let links = extract_links(bpb_clash());
    assert!(!links.iter().any(|l| l.contains("gstatic") || l.contains("Best")), "{links:?}");
}

/// Trojan is always TLS in Clash, so a configuration is free to leave `tls` out.
#[test]
fn a_clash_trojan_is_tls_even_when_it_does_not_say_so() {
    let body = r#"{"proxies":[{"name":"t","type":"trojan","server":"a.example.net","port":443,
          "password":"p","sni":"a.example.net"}]}"#;
    let link = &extract_links(body)[0];
    assert!(link.contains("security=tls"), "{link}");
    assert!(link.contains("sni=a.example.net"), "{link}");
}

#[test]
fn an_import_link_may_carry_its_address_percent_encoded() {
    // The encoded form keeps the carried address's own `&` inside it.
    let link = "sing-box://import-remote-profile?url=https%3A%2F%2Fedge.example.net%2Fsub%3Fapp%3Dsing-box%26lang%3Den#Normal";
    assert_eq!(resolve(link).unwrap(), "https://edge.example.net/sub?app=sing-box&lang=en");
}

/// Clash's import link puts the name in its own parameter, which is not part of the address.
#[test]
fn a_clash_import_link_is_unwrapped_and_its_name_left_behind() {
    let link = "clash://install-config?url=https%3A%2F%2Fedge.example.net%2Fsub%2Fnormal%3Fapp%3Dclash&name=BPB%20Normal";
    assert_eq!(resolve(link).unwrap(), "https://edge.example.net/sub/normal?app=clash");
}

/// The wrapper changes nothing about the rule: the address inside is a credential, and plain
/// HTTP would hand it to the network.
#[test]
fn an_import_link_wrapping_plain_http_is_refused() {
    let err = resolve("sing-box://import-remote-profile?url=http://edge.example.net/sub#x").unwrap_err();
    assert!(err.to_string().contains("plain HTTP"), "{err}");
}

#[test]
fn an_import_link_carrying_no_address_is_refused() {
    let err = resolve("sing-box://import-remote-profile?name=x").unwrap_err();
    assert!(matches!(err, SubscriptionError::Rejected(_)), "{err}");
}

#[test]
fn a_sing_box_detour_is_a_chain_and_refused_by_name() {
    let body = r#"{"endpoints":[
          { "type": "wireguard", "tag": "💦 1 - WoW", "detour": "💦 1 - Warp",
            "private_key": "s", "address": ["172.16.0.2/32"],
            "peers": [{ "address": "162.159.192.1", "port": 2408, "public_key": "k" }] }
        ]}"#;
    let links = extract_links(body);
    assert_eq!(links.len(), 1);
    assert!(links[0].starts_with("chain://162.159.192.1:2408#"), "got {}", links[0]);
}

#[test]
fn a_clash_dialer_proxy_is_a_chain_and_refused_by_name() {
    let body = r#"{"proxies":[{"name":"WoW","type":"wireguard","server":"162.159.192.1","port":2408,
          "dialer-proxy":"Warp","private-key":"s","public-key":"k","ip":"172.16.0.2"}]}"#;
    assert!(extract_links(body)[0].starts_with("chain://162.159.192.1:2408#"));
}

/// sing-box moved WireGuard from `outbounds` to `endpoints`, with the peer under `peers`. The
/// WARP client id has to come through, or the tunnel comes up and carries nothing.
#[test]
fn a_sing_box_wireguard_endpoint_keeps_its_keys_and_reserved_bytes() {
    let body = r#"{"endpoints":[{
          "type": "wireguard", "tag": "💦 1 - Warp", "mtu": 1280,
          "address": ["172.16.0.2/32", "2606:4700:110::1/128"],
          "private_key": "f7m/C8NHWPWIkGAbxTBAMhYHlzu3Ya7lSCbOSqfGu68=",
          "peers": [{ "address": "engage.cloudflareclient.com", "port": 2408,
                      "public_key": "bmXOC+F1FxEMF9dyiK2H5/1SUtzH0JuVo51h2wPfgyo=",
                      "reserved": [216, 253, 3], "persistent_keepalive_interval": 5,
                      "allowed_ips": ["0.0.0.0/0", "::/0"] }]
        }]}"#;
    let link = &extract_links(body)[0];
    assert!(
        link.starts_with("wireguard://f7m%2FC8NHWPWIkGAbxTBAMhYHlzu3Ya7lSCbOSqfGu68%3D@engage.cloudflareclient.com:2408?"),
        "got {link}"
    );
    assert!(link.contains("publickey=bmXOC%2BF1FxEMF9dyiK2H5%2F1SUtzH0JuVo51h2wPfgyo%3D"));
    assert!(link.contains("address=172.16.0.2%2F32%2C2606%3A4700%3A110%3A%3A1%2F128"));
    assert!(link.contains("reserved=216%2C253%2C3"));
    assert!(link.contains("mtu=1280"));
    assert!(link.contains("keepalive=5"));
}

/// Clash gives the interface bare addresses; the link needs them as prefixes.
#[test]
fn a_clash_wireguard_proxy_gets_prefixes_on_its_bare_addresses() {
    let body = r#"{"proxies":[{"name":"Warp","type":"wireguard","server":"engage.cloudflareclient.com",
          "port":2408,"ip":"172.16.0.2","ipv6":"2606:4700:110::1","private-key":"s","public-key":"k",
          "reserved":[216,253,3],"mtu":1280}]}"#;
    let link = &extract_links(body)[0];
    assert!(link.starts_with("wireguard://s@engage.cloudflareclient.com:2408?"), "{link}");
    assert!(link.contains("address=172.16.0.2%2F32%2C2606%3A4700%3A110%3A%3A1%2F128"), "{link}");
    assert!(link.contains("reserved=216%2C253%2C3"), "{link}");
}

#[test]
fn clash_reality_keys_are_read_from_reality_opts() {
    let body = r#"{"proxies":[{"name":"R","type":"vless","server":"a.example.net","port":443,"uuid":"u",
          "flow":"xtls-rprx-vision","tls":true,"servername":"www.example.net","client-fingerprint":"chrome",
          "reality-opts":{"public-key":"PBK","short-id":"ab"},"network":"tcp"}]}"#;
    let link = &extract_links(body)[0];
    assert!(link.contains("security=reality"), "{link}");
    assert!(link.contains("pbk=PBK"));
    assert!(link.contains("sid=ab"));
    assert!(link.contains("sni=www.example.net"));
    assert!(link.contains("flow=xtls-rprx-vision"));
}

#[test]
fn sing_box_reality_keys_are_read_from_the_tls_block() {
    let body = r#"{"outbounds":[{"type":"vless","tag":"R","server":"a.example.net","server_port":443,
          "uuid":"u","flow":"xtls-rprx-vision",
          "tls":{"enabled":true,"server_name":"www.example.net","utls":{"enabled":true,"fingerprint":"chrome"},
                 "reality":{"enabled":true,"public_key":"PBK","short_id":"ab"}}}]}"#;
    assert_eq!(
        extract_links(body)[0],
        "vless://u@a.example.net:443?type=tcp&security=reality&sni=www.example.net&fp=chrome&pbk=PBK&sid=ab&flow=xtls-rprx-vision#R"
    );
}

/// VMess over gRPC: the cipher and alterId live on the outbound, the service name on the
/// transport, and each has a different name in the link.
#[test]
fn sing_box_vmess_over_grpc_keeps_its_cipher_and_service_name() {
    let body = r#"{"outbounds":[{"type":"vmess","tag":"V","server":"a.example.net","server_port":443,
          "uuid":"u","security":"auto","alter_id":0,
          "tls":{"enabled":true,"server_name":"a.example.net"},
          "transport":{"type":"grpc","service_name":"svc"}}]}"#;
    assert_eq!(
        extract_links(body)[0],
        "vmess://u@a.example.net:443?type=grpc&security=tls&serviceName=svc&sni=a.example.net&encryption=auto&alterId=0#V"
    );
}

/// `shadowsocks` is sing-box's word; the frontend knows the scheme as `ss` and names it in its
/// refusal. Under the long spelling it could only say "not a protocol this build knows".
#[test]
fn shadowsocks_is_written_under_the_scheme_the_frontend_can_name() {
    let body = r#"{"outbounds":[{"type":"shadowsocks","tag":"S","server":"a.example.net","server_port":8388,
          "method":"aes-128-gcm","password":"p"}]}"#;
    assert!(extract_links(body)[0].starts_with("ss://"));
}

/// Clash's usual form is YAML, which this build does not read. Scanned as lines it would
/// report its DNS servers and health-check URLs as servers, so it is refused by name instead.
#[test]
fn a_clash_yaml_configuration_is_refused_by_name_rather_than_scanned() {
    let body = "mixed-port: 7890\n\
                    dns:\n  nameserver:\n    - https://8.8.8.8/dns-query\n\
                    proxies:\n  - {name: A, type: vless, server: a.example.net, port: 443, uuid: u}\n\
                    proxy-groups:\n  - {name: Auto, type: url-test, url: https://www.gstatic.com/generate_204, proxies: [A]}\n";
    let err = assemble(None, None, body).unwrap_err();
    assert!(matches!(err, SubscriptionError::Rejected(_)), "{err}");
    assert!(err.to_string().contains("Clash"), "{err}");
}
