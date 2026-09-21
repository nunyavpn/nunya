//! Opening a web page in the system browser — for the Support panel's Buy Me a Coffee link.
//!
//! The app's capabilities deliberately grant the webview no shell permission, and adding the
//! opener plugin would hand it one for any URL at all. So the webview asks this command instead, and
//! it opens only `https` pages on a host named in `ALLOWED_HOSTS`. Injected script that reached the
//! bridge could at most open a donation page; it could not send the user to a lookalike.
//!
//! The browser is the desktop's own (`open`, `xdg-open`, `url.dll`), never a webview window: a
//! payment page belongs in the browser the user already trusts, with their own extensions, and a
//! page loaded inside this app would be a page the app's CSP was written to keep out.

use std::process::Command;

/// Hosts `open_external` will open. Exact matches: `buymeacoffee.com.example.net` is not one.
///
/// Keep in step with `SUPPORT` in `src/support.ts`; a channel the command refuses is a button that
/// does nothing.
pub const ALLOWED_HOSTS: &[&str] = &["buymeacoffee.com", "www.buymeacoffee.com"];

/// The URL, if it may be opened; otherwise why not.
///
/// Parsed by hand rather than with a URL crate for the same reason the subscription reader's
/// escaping is: it is a dozen lines, and the rules are narrow enough to read at a glance.
pub fn allowed(url: &str) -> Result<&str, String> {
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("the address contains spaces or control characters".into());
    }
    let rest = url
        .strip_prefix("https://")
        .ok_or("only https:// pages are opened")?;
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    // `https://buymeacoffee.com@evil.example/` is a page on evil.example.
    if authority.contains('@') {
        return Err("the address carries a user name, which hides its real host".into());
    }
    let host = authority.to_ascii_lowercase();
    if !ALLOWED_HOSTS.contains(&host.as_str()) {
        return Err(format!("{host} is not a page this app opens"));
    }
    Ok(url)
}

/// Opens an allowed page in the system browser, and waits only for the opener to hand it off.
pub fn open(url: &str) -> Result<(), String> {
    let url = allowed(url)?;

    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        c.arg(url);
        c
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        // Not `cmd /C start`, which would read the URL's `&` as a command separator.
        let mut c = Command::new("rundll32");
        c.args(["url.dll,FileProtocolHandler", url]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };

    let status = command
        .status()
        .map_err(|e| format!("could not start the system browser: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the system browser could not be opened ({status})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_donation_page_is_allowed() {
        assert!(allowed("https://buymeacoffee.com/nunya").is_ok());
        assert!(allowed("https://www.buymeacoffee.com/nunya?ref=app#top").is_ok());
        // Host names are case-insensitive, and a link typed by hand is not always lowercase.
        assert!(allowed("https://BuyMeACoffee.com/nunya").is_ok());
    }

    #[test]
    fn plain_http_is_refused() {
        assert!(allowed("http://buymeacoffee.com/nunya").is_err());
    }

    #[test]
    fn any_other_host_is_refused() {
        assert!(allowed("https://example.net/").is_err());
        assert!(allowed("https://pay.buymeacoffee.com/nunya").is_err());
    }

    /// The three ways a URL can look like an allowed host and not be one.
    #[test]
    fn lookalike_hosts_are_refused() {
        assert!(allowed("https://buymeacoffee.com.example.net/nunya").is_err());
        assert!(allowed("https://buymeacoffee.com@example.net/").is_err());
        assert!(allowed("https://buymeacoffee.com:443@example.net/").is_err());
    }

    #[test]
    fn other_schemes_and_hidden_characters_are_refused() {
        assert!(allowed("javascript:alert(1)").is_err());
        assert!(allowed("file:///etc/passwd").is_err());
        assert!(allowed("https://buymeacoffee.com/a b").is_err());
        assert!(allowed("https://buymeacoffee.com/\nhttps://example.net").is_err());
        assert!(allowed("").is_err());
    }
}
