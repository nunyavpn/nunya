//! Pointing the desktop's system proxy at the proxy-mode listener, and putting it back.
//!
//! Proxy mode covers only what is pointed at it. The system proxy is how most of a desktop gets
//! pointed at something — browsers, GTK and Qt apps, anything using the platform's HTTP stack — so
//! setting it widens proxy mode's coverage without the privilege a TUN needs. It is still not the
//! whole machine: a CLI tool, a game or anything with its own network stack ignores it, which is
//! why the status card keeps saying so.
//!
//! **Putting it back is the part that matters.** A system proxy left pointing at a port nothing
//! listens on breaks every browser on the machine, and the user has no reason to suspect a VPN
//! client they already closed. So the previous settings are captured *before* anything is
//! changed and written to a file in the data directory; they are restored on disconnect, on quit,
//! and — if the app died holding them — on the next launch, before the user can notice.
//!
//! Restoring replays what was there rather than switching the proxy off: someone who had their
//! own proxy configured gets it back, not a blank setting.
//!
//! Each desktop keeps this in a different place, and the platform tools are the supported way to
//! change it (they also notify running apps), so this shells out to them:
//!
//! - GNOME and its relatives: `gsettings`, `org.gnome.system.proxy`.
//! - KDE Plasma: `kioslaverc` through `kwriteconfig6`/`kwriteconfig5`, then a D-Bus signal so KIO
//!   rereads it.
//! - macOS: `networksetup`, per network service.
//!
//! Anything else is refused by name rather than guessed at.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// Where the pre-change settings are kept while ours are applied.
pub const SAVED_FILE: &str = "system-proxy.json";

/// Hosts the system proxy is told to leave alone: the listener itself and the local machine.
const LOCAL_HOSTS: [&str; 3] = ["localhost", "127.0.0.0/8", "::1"];

/// What the system proxy was before this app changed it, in the form needed to put it back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "desktop", rename_all = "camelCase")]
pub enum Saved {
    /// `(schema, key, value)`, the value as `gsettings get` printed it — GVariant text, which
    /// `gsettings set` accepts back verbatim.
    Gnome { values: Vec<(String, String, String)> },
    /// `(key, value)` in the `Proxy Settings` group; `None` means the key was not set at all and
    /// is deleted on restore rather than written back empty.
    Kde {
        tool: String,
        values: Vec<(String, Option<String>)>,
    },
    Mac { services: Vec<MacService> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacService {
    pub name: String,
    pub web: MacProxy,
    pub secure: MacProxy,
    /// Defaulted so a file saved before FTP was handled still reads back.
    #[serde(default)]
    pub ftp: MacProxy,
    pub socks: MacProxy,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MacProxy {
    pub enabled: bool,
    pub server: String,
    pub port: u16,
}

// ---------------------------------------------------------------- the three operations

/// Reads the current system proxy settings, so they can be restored later.
pub fn capture() -> Result<Saved, String> {
    match desktop()? {
        Desktop::Gnome => gnome::capture(),
        Desktop::Kde(tool) => kde::capture(&tool),
        Desktop::Mac => mac::capture(),
    }
}

/// Points the system proxy at `127.0.0.1:port` for every kind the desktop has a slot for — HTTP,
/// HTTPS, FTP and SOCKS. The listener is a `mixed` inbound, so one port answers them all, and a
/// slot left empty is a class of app that silently goes around the tunnel. (FTP URLs are carried
/// by the HTTP proxy protocol, which is what an FTP proxy setting means to the apps that read it.)
///
/// Always loopback, even with Allow LAN on: that setting is about who else may connect, and this
/// machine reaches its own listener on loopback either way.
pub fn apply(port: u16) -> Result<(), String> {
    match desktop()? {
        Desktop::Gnome => gnome::apply(port),
        Desktop::Kde(tool) => kde::apply(&tool, port),
        Desktop::Mac => mac::apply(port),
    }
}

/// Puts back what `capture` read.
pub fn restore(saved: &Saved) -> Result<(), String> {
    match saved {
        Saved::Gnome { values } => gnome::restore(values),
        Saved::Kde { tool, values } => kde::restore(tool, values),
        Saved::Mac { services } => mac::restore(services),
    }
}

// ---------------------------------------------------------------- the saved file

pub fn saved_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SAVED_FILE)
}

pub fn write_saved(data_dir: &Path, saved: &Saved) -> Result<(), String> {
    std::fs::create_dir_all(data_dir).map_err(|e| format!("cannot create {}: {e}", data_dir.display()))?;
    let json = serde_json::to_string_pretty(saved).map_err(|e| e.to_string())?;
    let path = saved_path(data_dir);
    std::fs::write(&path, json).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

pub fn read_saved(data_dir: &Path) -> Option<Saved> {
    let text = std::fs::read_to_string(saved_path(data_dir)).ok()?;
    match serde_json::from_str(&text) {
        Ok(saved) => Some(saved),
        Err(e) => {
            log::warn!("ignoring an unreadable saved system proxy: {e}");
            None
        }
    }
}

pub fn remove_saved(data_dir: &Path) {
    let _ = std::fs::remove_file(saved_path(data_dir));
}

// ---------------------------------------------------------------- which desktop

enum Desktop {
    Gnome,
    /// The `kwriteconfig` generation that is installed; `kreadconfig` has the same suffix.
    Kde(String),
    Mac,
}

fn desktop() -> Result<Desktop, String> {
    if cfg!(target_os = "macos") {
        return Ok(Desktop::Mac);
    }
    if !cfg!(target_os = "linux") {
        return Err("setting the system proxy is not supported on this platform".into());
    }

    let current = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if current.split(':').any(|d| d.eq_ignore_ascii_case("KDE")) {
        for tool in ["kwriteconfig6", "kwriteconfig5"] {
            if run(tool, &["--help"]).is_ok() {
                return Ok(Desktop::Kde(tool.to_string()));
            }
        }
        return Err("KDE is running, but neither kwriteconfig6 nor kwriteconfig5 was found".into());
    }

    // GNOME, and the desktops that share its settings schema (Budgie, Cinnamon's GTK apps,
    // Pantheon, Unity). Asked of gsettings itself rather than inferred from the desktop name.
    if run("gsettings", &["list-keys", "org.gnome.system.proxy"]).is_ok() {
        return Ok(Desktop::Gnome);
    }

    Err(format!(
        "no supported way to set the system proxy on this desktop ({}); GNOME and KDE are supported",
        if current.is_empty() { "unknown" } else { &current }
    ))
}

/// Runs a tool and returns its trimmed stdout, or its stderr as the error.
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let out = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("{program}: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(format!(
            "{program} {} failed: {}",
            args.join(" "),
            if err.is_empty() { out.status.to_string() } else { err }
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

// ---------------------------------------------------------------- GNOME

mod gnome {
    use super::{run, LOCAL_HOSTS};

    /// Every key this module writes, so every one of them is captured and restored.
    const KEYS: [(&str, &str); 11] = [
        ("org.gnome.system.proxy", "mode"),
        ("org.gnome.system.proxy", "ignore-hosts"),
        ("org.gnome.system.proxy.http", "enabled"),
        ("org.gnome.system.proxy.http", "host"),
        ("org.gnome.system.proxy.http", "port"),
        ("org.gnome.system.proxy.https", "host"),
        ("org.gnome.system.proxy.https", "port"),
        ("org.gnome.system.proxy.ftp", "host"),
        ("org.gnome.system.proxy.ftp", "port"),
        ("org.gnome.system.proxy.socks", "host"),
        ("org.gnome.system.proxy.socks", "port"),
    ];

    pub fn capture() -> Result<super::Saved, String> {
        let mut values = Vec::with_capacity(KEYS.len());
        for (schema, key) in KEYS {
            values.push((schema.to_string(), key.to_string(), run("gsettings", &["get", schema, key])?));
        }
        Ok(super::Saved::Gnome { values })
    }

    pub fn apply(port: u16) -> Result<(), String> {
        let port = port.to_string();
        let ignore = ignore_hosts();
        // The mode goes last, so the proxy is never switched on while half of it is configured.
        for (schema, key, value) in [
            ("org.gnome.system.proxy.http", "host", "'127.0.0.1'"),
            ("org.gnome.system.proxy.http", "port", port.as_str()),
            // The GConf-era switch. `mode` supersedes it, but older readers still check it and
            // treat HTTP as off — falling through to SOCKS alone — when it is false.
            ("org.gnome.system.proxy.http", "enabled", "true"),
            ("org.gnome.system.proxy.https", "host", "'127.0.0.1'"),
            ("org.gnome.system.proxy.https", "port", port.as_str()),
            ("org.gnome.system.proxy.ftp", "host", "'127.0.0.1'"),
            ("org.gnome.system.proxy.ftp", "port", port.as_str()),
            ("org.gnome.system.proxy.socks", "host", "'127.0.0.1'"),
            ("org.gnome.system.proxy.socks", "port", port.as_str()),
            ("org.gnome.system.proxy", "ignore-hosts", ignore.as_str()),
            ("org.gnome.system.proxy", "mode", "'manual'"),
        ] {
            run("gsettings", &["set", schema, key, value])?;
        }
        Ok(())
    }

    pub fn restore(values: &[(String, String, String)]) -> Result<(), String> {
        // Mode first this time, so the machine stops using the listener before its address goes.
        let mut ordered: Vec<&(String, String, String)> = values.iter().collect();
        ordered.sort_by_key(|(_, key, _)| key != "mode");
        let mut first_error = None;
        for (schema, key, value) in ordered {
            // Carries on past a failure: restoring most of it beats stopping at the first key.
            if let Err(e) = run("gsettings", &["set", schema, key, value]) {
                first_error.get_or_insert(e);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// `['localhost', '127.0.0.0/8', '::1']`, as GVariant text.
    pub fn ignore_hosts() -> String {
        let quoted: Vec<String> = LOCAL_HOSTS.iter().map(|h| format!("'{h}'")).collect();
        format!("[{}]", quoted.join(", "))
    }
}

// ---------------------------------------------------------------- KDE

mod kde {
    use super::{run, LOCAL_HOSTS};

    const GROUP: &str = "Proxy Settings";
    const KEYS: [&str; 6] = ["ProxyType", "httpProxy", "httpsProxy", "ftpProxy", "socksProxy", "NoProxyFor"];

    fn reader(tool: &str) -> String {
        tool.replace("kwriteconfig", "kreadconfig")
    }

    pub fn capture(tool: &str) -> Result<super::Saved, String> {
        let read = reader(tool);
        let mut values = Vec::with_capacity(KEYS.len());
        for key in KEYS {
            let value = run(&read, &["--file", "kioslaverc", "--group", GROUP, "--key", key])?;
            // kreadconfig prints nothing for a key that is not set; restoring that as an empty
            // string would leave a key behind that was never there.
            values.push((key.to_string(), (!value.is_empty()).then_some(value)));
        }
        Ok(super::Saved::Kde {
            tool: tool.to_string(),
            values,
        })
    }

    pub fn apply(tool: &str, port: u16) -> Result<(), String> {
        // KDE writes a proxy as "scheme://host port", with a space rather than a colon.
        let http = format!("http://127.0.0.1 {port}");
        let socks = format!("socks://127.0.0.1 {port}");
        let no_proxy = LOCAL_HOSTS.join(",");
        for (key, value) in [
            ("httpProxy", http.as_str()),
            ("httpsProxy", http.as_str()),
            ("ftpProxy", http.as_str()),
            ("socksProxy", socks.as_str()),
            ("NoProxyFor", no_proxy.as_str()),
            // 1 is "manually specified"; last, for the same reason as GNOME's mode.
            ("ProxyType", "1"),
        ] {
            run(tool, &["--file", "kioslaverc", "--group", GROUP, "--key", key, value])?;
        }
        notify();
        Ok(())
    }

    pub fn restore(tool: &str, values: &[(String, Option<String>)]) -> Result<(), String> {
        let mut first_error = None;
        for (key, value) in values {
            let result = match value {
                Some(v) => run(tool, &["--file", "kioslaverc", "--group", GROUP, "--key", key, v]),
                None => run(tool, &["--file", "kioslaverc", "--group", GROUP, "--key", key, "--delete"]),
            };
            if let Err(e) = result {
                first_error.get_or_insert(e);
            }
        }
        notify();
        first_error.map_or(Ok(()), Err)
    }

    /// Tells running KIO apps to reread the file; without it they keep the old proxy until they
    /// restart. Best effort — the file is already right if this fails.
    fn notify() {
        let _ = run(
            "dbus-send",
            &[
                "--type=signal",
                "/KIO/Scheduler",
                "org.kde.KIO.Scheduler.reparseSlaveConfiguration",
                "string:",
            ],
        );
    }
}

// ---------------------------------------------------------------- macOS

mod mac {
    use super::{run, MacProxy, MacService};

    pub fn capture() -> Result<super::Saved, String> {
        let listing = run("networksetup", &["-listallnetworkservices"])?;
        let mut services = Vec::new();
        for name in service_names(&listing) {
            services.push(MacService {
                web: parse_proxy(&run("networksetup", &["-getwebproxy", &name])?),
                secure: parse_proxy(&run("networksetup", &["-getsecurewebproxy", &name])?),
                ftp: parse_proxy(&run("networksetup", &["-getftpproxy", &name])?),
                socks: parse_proxy(&run("networksetup", &["-getsocksfirewallproxy", &name])?),
                name,
            });
        }
        Ok(super::Saved::Mac { services })
    }

    pub fn apply(port: u16) -> Result<(), String> {
        let listing = run("networksetup", &["-listallnetworkservices"])?;
        let port = port.to_string();
        for name in service_names(&listing) {
            for verb in ["-setwebproxy", "-setsecurewebproxy", "-setftpproxy", "-setsocksfirewallproxy"] {
                run("networksetup", &[verb, &name, "127.0.0.1", &port])?;
            }
        }
        Ok(())
    }

    pub fn restore(services: &[MacService]) -> Result<(), String> {
        let mut first_error = None;
        for service in services {
            for (proxy, set, state) in [
                (&service.web, "-setwebproxy", "-setwebproxystate"),
                (&service.secure, "-setsecurewebproxy", "-setsecurewebproxystate"),
                (&service.ftp, "-setftpproxy", "-setftpproxystate"),
                (&service.socks, "-setsocksfirewallproxy", "-setsocksfirewallproxystate"),
            ] {
                // The old address first — even for a proxy that was off, so a user who keeps one
                // configured but disabled finds it as they left it — then the old on/off state,
                // since setting an address switches the proxy on.
                let mut steps: Vec<Vec<String>> = Vec::new();
                if !proxy.server.is_empty() {
                    steps.push(vec![set.into(), service.name.clone(), proxy.server.clone(), proxy.port.to_string()]);
                }
                steps.push(vec![
                    state.into(),
                    service.name.clone(),
                    if proxy.enabled { "on" } else { "off" }.into(),
                ]);
                for step in steps {
                    let args: Vec<&str> = step.iter().map(String::as_str).collect();
                    if let Err(e) = run("networksetup", &args) {
                        first_error.get_or_insert(e);
                    }
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// The services `-listallnetworkservices` names, minus its header line and the disabled ones
    /// (marked with a leading `*`), which reject proxy changes.
    pub fn service_names(listing: &str) -> Vec<String> {
        listing
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('*'))
            .map(str::to_string)
            .collect()
    }

    /// Reads `-getwebproxy` output: `Enabled: Yes`, `Server: …`, `Port: …`.
    pub fn parse_proxy(text: &str) -> MacProxy {
        let mut proxy = MacProxy::default();
        for line in text.lines() {
            let Some((key, value)) = line.split_once(':') else { continue };
            let value = value.trim();
            match key.trim() {
                "Enabled" => proxy.enabled = value.eq_ignore_ascii_case("yes"),
                "Server" => proxy.server = value.to_string(),
                "Port" => proxy.port = value.parse().unwrap_or(0),
                _ => {}
            }
        }
        proxy
    }
}

#[cfg(test)]
mod tests {
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
}
