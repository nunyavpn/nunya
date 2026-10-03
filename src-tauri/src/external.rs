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
    crate::platform::open_browser(allowed(url)?)
}

#[cfg(test)]
mod tests;
