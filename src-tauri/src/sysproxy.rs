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
//! - macOS: `networksetup`, per network service — then `scutil --proxy`, which is what apps are
//!   actually given, to confirm it took. `networksetup` exits 0 on many of its own errors, so its
//!   status alone proves nothing.
//! - Windows: the per-user WinINet settings (`HKCU\…\Internet Settings`) through `reg.exe`,
//!   which is what Edge, Chrome and the Windows proxy settings page all read.
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
    /// `(value name, value)`; `None` means the value did not exist and is deleted on restore.
    Windows { values: Vec<(String, Option<RegValue>)> },
}

/// A registry value as `reg query` printed it: its type (`REG_DWORD`, `REG_SZ`) and its data,
/// which `reg add` accepts back in the same form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegValue {
    pub kind: String,
    pub data: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MacService {
    pub name: String,
    pub web: MacProxy,
    pub secure: MacProxy,
    // No FTP: current macOS has dropped `networksetup`'s FTP proxy verbs, and asking for one
    // fails the whole capture. A file saved by a build that had it still reads back — the field
    // is simply ignored.
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
        Desktop::Windows => windows::capture(),
    }
}

/// Points the system proxy at `127.0.0.1:port` for every kind the desktop has a slot for — HTTP,
/// HTTPS, FTP (where there still is one; not on current macOS) and SOCKS. The listener is a `mixed` inbound, so one port answers them all, and a
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
        Desktop::Windows => windows::apply(port),
    }
}

/// Puts back what `capture` read.
pub fn restore(saved: &Saved) -> Result<(), String> {
    match saved {
        Saved::Gnome { values } => gnome::restore(values),
        Saved::Kde { tool, values } => kde::restore(tool, values),
        Saved::Mac { services } => mac::restore(services),
        Saved::Windows { values } => windows::restore(values),
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
    Windows,
}

fn desktop() -> Result<Desktop, String> {
    if cfg!(target_os = "macos") {
        return Ok(Desktop::Mac);
    }
    if cfg!(target_os = "windows") {
        return Ok(Desktop::Windows);
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
///
/// Also an error: output that says it is one. `networksetup` prints `** Error: …` and exits 0 for
/// a bad service name or a rejected value, so its status alone would report a failure as done.
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new(program);
    command.args(args);
    // A GUI app starting a console tool on Windows gets a console window flashed up for each
    // call; this flag (CREATE_NO_WINDOW) runs it without one.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let out = command.output().map_err(|e| format!("{program}: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || reports_error(&stdout) {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        let said = if reports_error(&stdout) { stdout } else { err };
        return Err(format!(
            "{program} {} failed: {}",
            args.join(" "),
            if said.is_empty() { out.status.to_string() } else { said }
        ));
    }
    Ok(stdout)
}

/// Whether a tool's output is `networksetup`'s way of failing while exiting 0.
fn reports_error(stdout: &str) -> bool {
    stdout.lines().any(|line| line.trim_start().starts_with("** Error"))
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
    use std::collections::HashMap;
    use std::time::Duration;

    use super::{run, MacProxy, MacService};

    /// Full paths: an app started from Finder gets a minimal `PATH`, and a tool found by name is a
    /// tool anything earlier on that path could stand in for.
    const NETWORKSETUP: &str = "/usr/sbin/networksetup";
    const SCUTIL: &str = "/usr/sbin/scutil";

    /// The proxies each service has, as `networksetup` names their verbs.
    ///
    /// Not FTP: current macOS has removed `-getftpproxy` and `-setftpproxy`, and printing its
    /// usage with "The command is not recognized" instead failed every capture — which is what
    /// kept the system proxy from ever being set on a Mac.
    const KINDS: [(&str, &str, &str); 3] = [
        ("-getwebproxy", "-setwebproxy", "-setwebproxystate"),
        ("-getsecurewebproxy", "-setsecurewebproxy", "-setsecurewebproxystate"),
        ("-getsocksfirewallproxy", "-setsocksfirewallproxy", "-setsocksfirewallproxystate"),
    ];

    pub fn capture() -> Result<super::Saved, String> {
        let listing = run(NETWORKSETUP, &["-listallnetworkservices"])?;
        let mut services = Vec::new();
        for name in service_names(&listing) {
            let get = |verb: &str| run(NETWORKSETUP, &[verb, &name]).map(|out| parse_proxy(&out));
            services.push(MacService {
                web: get(KINDS[0].0)?,
                secure: get(KINDS[1].0)?,
                socks: get(KINDS[2].0)?,
                name,
            });
        }
        Ok(super::Saved::Mac { services })
    }

    /// Points every service at the listener, then checks that macOS is actually using it.
    ///
    /// Every service, and past a failure: a VPN configuration or an unplugged adapter that
    /// refuses a proxy must not stop the one carrying traffic from getting it. Whether it worked
    /// is then read from `scutil --proxy` — the settings of the service in use, as apps are given
    /// them — rather than inferred from `networksetup` having exited 0.
    pub fn apply(port: u16) -> Result<(), String> {
        let listing = run(NETWORKSETUP, &["-listallnetworkservices"])?;
        let port = port.to_string();
        let mut failures = Vec::new();
        for name in service_names(&listing) {
            for (_, set, _) in KINDS {
                if let Err(e) = run(NETWORKSETUP, &[set, &name, "127.0.0.1", &port]) {
                    failures.push(format!("{name}: {e}"));
                }
            }
        }

        // configd applies a change a moment after networksetup returns.
        for _ in 0..12 {
            if in_effect(&parse_scutil(&run(SCUTIL, &["--proxy"])?), &port) {
                for failure in &failures {
                    log::warn!("system proxy not set on one network service: {failure}");
                }
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(150));
        }
        Err(if failures.is_empty() {
            "networksetup accepted the proxy, but macOS is not using it (scutil --proxy)".into()
        } else {
            failures.join("; ")
        })
    }

    pub fn restore(services: &[MacService]) -> Result<(), String> {
        let mut first_error = None;
        for step in restore_steps(services) {
            let args: Vec<&str> = step.iter().map(String::as_str).collect();
            if let Err(e) = run(NETWORKSETUP, &args) {
                first_error.get_or_insert(e);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// The `networksetup` calls that put each proxy back exactly: its old address, then its old
    /// on/off state, since setting an address switches the proxy on.
    ///
    /// The address is written back even when it was empty — `-setwebproxy <service> "" 0` clears
    /// it — or ours would stay behind, switched off, in every service's settings. And even for a
    /// proxy that was off, so one a user keeps configured but disabled is found as they left it.
    pub fn restore_steps(services: &[MacService]) -> Vec<Vec<String>> {
        let mut steps = Vec::new();
        for service in services {
            for (proxy, (_, set, state)) in
                [&service.web, &service.secure, &service.socks].into_iter().zip(KINDS)
            {
                steps.push(vec![set.into(), service.name.clone(), proxy.server.clone(), proxy.port.to_string()]);
                steps.push(vec![
                    state.into(),
                    service.name.clone(),
                    if proxy.enabled { "on" } else { "off" }.into(),
                ]);
            }
        }
        steps
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

    /// Reads `scutil --proxy`: `HTTPEnable : 1`, `HTTPProxy : 127.0.0.1`, `HTTPPort : 2080`…
    pub fn parse_scutil(text: &str) -> HashMap<String, String> {
        text.lines()
            .filter_map(|line| line.split_once(" : "))
            .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
            .collect()
    }

    /// Whether the web, secure web and SOCKS proxies apps are given all point at our listener.
    pub fn in_effect(settings: &HashMap<String, String>, port: &str) -> bool {
        ["HTTP", "HTTPS", "SOCKS"].iter().all(|kind| {
            let get = |field: &str| settings.get(&format!("{kind}{field}")).map(String::as_str);
            get("Enable") == Some("1") && get("Proxy") == Some("127.0.0.1") && get("Port") == Some(port)
        })
    }
}

// ---------------------------------------------------------------- Windows

mod windows {
    use super::{run, RegValue, LOCAL_HOSTS};

    /// Where WinINet keeps the per-user proxy: what Edge, Chrome, the Windows proxy settings page
    /// and most apps using the platform's HTTP stack read. Per user, so no elevation is needed.
    pub const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";

    /// Every value this module writes or removes, so every one is captured and put back.
    /// `AutoConfigURL` is a PAC script, which takes precedence over a manual proxy; it is set
    /// aside while ours is applied.
    pub const NAMES: [&str; 4] = ["ProxyEnable", "ProxyServer", "ProxyOverride", "AutoConfigURL"];

    const REG: &str = "reg";

    pub fn capture() -> Result<super::Saved, String> {
        // The whole key at once: a missing value is then simply absent, rather than an error whose
        // wording `reg` localises into the system's language.
        let listing = run(REG, &["query", KEY])?;
        let present = parse_query(&listing);
        let values = NAMES
            .iter()
            .map(|name| {
                let value = present.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.clone());
                (name.to_string(), value)
            })
            .collect();
        Ok(super::Saved::Windows { values })
    }

    pub fn apply(port: u16) -> Result<(), String> {
        for step in apply_steps(port) {
            let args: Vec<&str> = step.iter().map(String::as_str).collect();
            // Deleting a PAC URL that is not there is the one step allowed to fail.
            match run(REG, &args) {
                Err(_) if args.first() == Some(&"delete") => {}
                other => {
                    other?;
                }
            }
        }
        Ok(())
    }

    pub fn restore(values: &[(String, Option<RegValue>)]) -> Result<(), String> {
        let mut first_error = None;
        for (step, may_fail) in restore_steps(values) {
            let args: Vec<&str> = step.iter().map(String::as_str).collect();
            if let Err(e) = run(REG, &args) {
                if !may_fail {
                    first_error.get_or_insert(e);
                }
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// The `reg` calls that point WinINet at the listener. The switch goes last, so the proxy is
    /// never on while half configured. One server for every scheme: the listener is `mixed`, and
    /// a single `host:port` is what the Windows settings page itself writes.
    pub fn apply_steps(port: u16) -> Vec<Vec<String>> {
        let add = |name: &str, kind: &str, data: String| {
            vec!["add".into(), KEY.into(), "/v".into(), name.into(), "/t".into(), kind.into(), "/d".into(), data, "/f".into()]
        };
        let bypass: Vec<String> = LOCAL_HOSTS
            .iter()
            .map(|h| match *h {
                "127.0.0.0/8" => "127.*".to_string(),
                "::1" => "[::1]".to_string(),
                other => other.to_string(),
            })
            .chain(["<local>".to_string()])
            .collect();
        vec![
            add("ProxyServer", "REG_SZ", format!("127.0.0.1:{port}")),
            add("ProxyOverride", "REG_SZ", bypass.join(";")),
            vec!["delete".into(), KEY.into(), "/v".into(), "AutoConfigURL".into(), "/f".into()],
            add("ProxyEnable", "REG_DWORD", "1".into()),
        ]
    }

    /// The `reg` calls that put every captured value back, each paired with whether it may fail:
    /// deleting a value that did not exist before, and may still not, is not a failure. The
    /// switch goes first, so the machine stops using the listener before its address goes.
    pub fn restore_steps(values: &[(String, Option<RegValue>)]) -> Vec<(Vec<String>, bool)> {
        let mut ordered: Vec<&(String, Option<RegValue>)> = values.iter().collect();
        ordered.sort_by_key(|(name, _)| name != "ProxyEnable");
        ordered
            .into_iter()
            .map(|(name, value)| match value {
                Some(v) => (
                    vec![
                        "add".into(),
                        KEY.into(),
                        "/v".into(),
                        name.clone(),
                        "/t".into(),
                        v.kind.clone(),
                        "/d".into(),
                        v.data.clone(),
                        "/f".into(),
                    ],
                    false,
                ),
                None => (vec!["delete".into(), KEY.into(), "/v".into(), name.clone(), "/f".into()], true),
            })
            .collect()
    }

    /// Reads `reg query` output: `    ProxyEnable    REG_DWORD    0x1`, four spaces apart.
    pub fn parse_query(text: &str) -> Vec<(String, RegValue)> {
        text.lines()
            .filter_map(|line| {
                let line = line.trim_start().trim_end_matches('\r');
                let at = line.find("    REG_")?;
                let name = line[..at].trim_end().to_string();
                let rest = &line[at + 4..];
                let (kind, data) = rest.split_once("    ").unwrap_or((rest.trim_end(), ""));
                Some((
                    name,
                    RegValue {
                        kind: kind.to_string(),
                        data: data.to_string(),
                    },
                ))
            })
            .collect()
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
}
