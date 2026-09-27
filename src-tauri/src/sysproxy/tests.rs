use super::*;

#[test]
fn the_ignore_list_is_written_as_a_gvariant_string_array() {
    assert_eq!(gnome::ignore_hosts(), "['localhost', '127.0.0.0/8', '::1']");
}

#[test]
fn disabled_and_header_lines_are_not_taken_for_mac_network_services() {
    let listing = "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Bluetooth PAN\nThunderbolt Bridge\n";
    assert_eq!(mac::service_names(listing), vec!["Wi-Fi", "Thunderbolt Bridge"]);
}

#[test]
fn a_mac_proxy_reads_back_as_it_was_set() {
    let text = "Enabled: Yes\nServer: proxy.corp.example.net\nPort: 3128\nAuthenticated Proxy Enabled: 0";
    assert_eq!(
        mac::parse_proxy(text),
        MacProxy {
            enabled: true,
            server: "proxy.corp.example.net".into(),
            port: 3128
        }
    );
    assert_eq!(mac::parse_proxy("Enabled: No\nServer: \nPort: 0"), MacProxy::default());
}

#[test]
fn networksetup_saying_error_is_an_error_even_when_it_exits_0() {
    assert!(reports_error("** Error: The parameters were not valid."));
    assert!(!reports_error("Enabled: No\nServer: \nPort: 0"));
}

/// What apps are given is the proof it worked, not networksetup's exit status.
#[test]
fn the_mac_proxy_counts_as_set_only_when_apps_are_given_it() {
    let ours = "<dictionary> {\n  HTTPEnable : 1\n  HTTPPort : 2080\n  HTTPProxy : 127.0.0.1\n  HTTPSEnable : 1\n  HTTPSPort : 2080\n  HTTPSProxy : 127.0.0.1\n  SOCKSEnable : 1\n  SOCKSPort : 2080\n  SOCKSProxy : 127.0.0.1\n}";
    assert!(mac::in_effect(&mac::parse_scutil(ours), "2080"));
    assert!(!mac::in_effect(&mac::parse_scutil(ours), "2081"));
    let off = "<dictionary> {\n  ExceptionsList : <array> {\n    0 : *.local\n  }\n  FTPPassive : 1\n  HTTPEnable : 0\n}";
    assert!(!mac::in_effect(&mac::parse_scutil(off), "2080"));
}

/// An address that was empty is cleared, not left behind switched off.
#[test]
fn a_mac_restore_clears_an_address_that_was_empty() {
    let untouched = MacService {
        name: "Wi-Fi".into(),
        web: MacProxy::default(),
        secure: MacProxy::default(),
        socks: MacProxy {
            enabled: true,
            server: "socks.example.net".into(),
            port: 1080,
        },
    };
    let steps = mac::restore_steps(&[untouched]);
    assert_eq!(steps[0], vec!["-setwebproxy", "Wi-Fi", "", "0"]);
    assert_eq!(steps[1], vec!["-setwebproxystate", "Wi-Fi", "off"]);
    assert_eq!(steps[4], vec!["-setsocksfirewallproxy", "Wi-Fi", "socks.example.net", "1080"]);
    assert_eq!(steps[5], vec!["-setsocksfirewallproxystate", "Wi-Fi", "on"]);
    assert!(!steps.iter().any(|s| s[0].contains("ftp")), "current macOS has no FTP proxy verbs");
}

/// A file written by a build that still captured FTP must read back, or a crash-left proxy
/// could not be restored after an upgrade.
#[test]
fn a_saved_mac_proxy_with_ftp_still_reads_back() {
    let old = r#"{"desktop":"mac","services":[{"name":"Wi-Fi","web":{"enabled":false,"server":"","port":0},"secure":{"enabled":false,"server":"","port":0},"ftp":{"enabled":false,"server":"","port":0},"socks":{"enabled":false,"server":"","port":0}}]}"#;
    assert!(matches!(serde_json::from_str::<Saved>(old), Ok(Saved::Mac { .. })));
}

#[test]
fn a_windows_registry_listing_reads_back_value_by_value() {
    let listing = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\r\n    CertificateRevocation    REG_DWORD    0x1\r\n    ProxyEnable    REG_DWORD    0x0\r\n    ProxyServer    REG_SZ    proxy.corp.example.net:3128\r\n    ProxyOverride    REG_SZ    \r\n";
    let values = windows::parse_query(listing);
    let get = |name: &str| values.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());
    assert_eq!(get("ProxyEnable"), Some(RegValue { kind: "REG_DWORD".into(), data: "0x0".into() }));
    assert_eq!(get("ProxyServer").unwrap().data, "proxy.corp.example.net:3128");
    assert_eq!(get("ProxyOverride").unwrap().data, "");
    assert_eq!(get("AutoConfigURL"), None);
}

#[test]
fn windows_is_pointed_at_the_listener_with_the_switch_last() {
    let steps = windows::apply_steps(2080);
    assert_eq!(steps[0][3..8], ["ProxyServer", "/t", "REG_SZ", "/d", "127.0.0.1:2080"]);
    assert_eq!(steps[1][7], "localhost;127.*;[::1];<local>");
    assert_eq!(steps[2][0], "delete", "a PAC script would take precedence over ours");
    assert_eq!(steps[3][3..8], ["ProxyEnable", "/t", "REG_DWORD", "/d", "1"]);
}

/// What was there is written back, what was not is removed, and the switch goes first.
#[test]
fn windows_is_put_back_exactly() {
    let values = vec![
        ("ProxyEnable".to_string(), Some(RegValue { kind: "REG_DWORD".into(), data: "0x0".into() })),
        ("ProxyServer".to_string(), None),
        ("AutoConfigURL".to_string(), Some(RegValue { kind: "REG_SZ".into(), data: "http://wpad.example.net/p.pac".into() })),
    ];
    let steps = windows::restore_steps(&values);
    assert_eq!(steps[0].0[3..8], ["ProxyEnable", "/t", "REG_DWORD", "/d", "0x0"]);
    assert_eq!((steps[1].0[0].as_str(), steps[1].0[3].as_str(), steps[1].1), ("delete", "ProxyServer", true));
    assert_eq!(steps[2].0[7], "http://wpad.example.net/p.pac");
}

/// The whole macOS cycle on a real machine: capture, apply, check apps are given the proxy,
/// restore, and end exactly where it started.
///
/// Ignored, and refuses to run unless asked: it points this Mac's proxy at a local port for a
/// few seconds, which breaks browsing until it is put back.
///
///     NUNYA_TOUCH_SYSTEM_PROXY=1 cargo test --manifest-path src-tauri/Cargo.toml mac_proxy -- --ignored
#[cfg(target_os = "macos")]
#[test]
#[ignore]
fn the_mac_proxy_is_set_and_then_put_back_exactly() {
    assert_eq!(
        std::env::var("NUNYA_TOUCH_SYSTEM_PROXY").as_deref(),
        Ok("1"),
        "refusing to change this Mac's system proxy; see this test's doc comment"
    );
    let before = capture().expect("capture");
    let applied = apply(2099);
    let seen = run("/usr/sbin/scutil", &["--proxy"]).unwrap_or_default();
    restore(&before).expect("restore");
    applied.expect("apply");
    assert!(seen.contains("HTTPProxy : 127.0.0.1") && seen.contains("HTTPPort : 2099"), "{seen}");
    assert_eq!(capture().expect("capture again"), before, "not put back exactly");
}

/// The whole cycle against a real `gsettings`, which must end exactly where it started.
///
/// Ignored, and refuses to run against the real session: it would change the machine's proxy.
/// Point gsettings at a throwaway keyfile instead:
///
///     GSETTINGS_BACKEND=keyfile XDG_CONFIG_HOME=$(mktemp -d) XDG_CURRENT_DESKTOP=GNOME \
///       cargo test --manifest-path src-tauri/Cargo.toml gnome_proxy -- --ignored
#[test]
#[ignore]
fn the_gnome_proxy_is_set_and_then_put_back_exactly() {
    assert_eq!(
        std::env::var("GSETTINGS_BACKEND").as_deref(),
        Ok("keyfile"),
        "refusing to change the real system proxy; see this test's doc comment"
    );
    let before = capture().expect("capture");
    apply(2080).expect("apply");

    let get = |schema: &str, key: &str| run("gsettings", &["get", schema, key]).unwrap();
    assert_eq!(get("org.gnome.system.proxy", "mode"), "'manual'");
    assert_eq!(get("org.gnome.system.proxy.http", "enabled"), "true");
    for kind in ["http", "https", "ftp", "socks"] {
        let schema = format!("org.gnome.system.proxy.{kind}");
        assert_eq!(get(&schema, "host"), "'127.0.0.1'", "{kind} host");
        assert_eq!(get(&schema, "port"), "2080", "{kind} port");
    }

    restore(&before).expect("restore");
    assert_eq!(capture().expect("capture again"), before);
}

/// The saved file is what rescues a machine after a crash, so it has to read back exactly.
#[test]
fn the_saved_settings_survive_a_round_trip_through_the_file() {
    let dir = std::env::temp_dir().join(format!("nunya-sysproxy-test-{}", std::process::id()));
    let saved = Saved::Kde {
        tool: "kwriteconfig6".into(),
        values: vec![("ProxyType".into(), Some("0".into())), ("httpProxy".into(), None)],
    };
    write_saved(&dir, &saved).unwrap();
    assert_eq!(read_saved(&dir), Some(saved));
    remove_saved(&dir);
    assert_eq!(read_saved(&dir), None);
    let _ = std::fs::remove_dir_all(&dir);
}
