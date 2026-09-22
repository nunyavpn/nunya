<p align="center">
  <img src="docs/logo.png" width="112" height="112" alt="Nunya">
</p>

<h1 align="center">Nunya</h1>

<p align="center">
  <strong>A VPN client that is safe, fast, reliable, secure and easy to use.</strong><br>
  One app for your VPN and proxy configs, from a single link to a subscription of hundreds.
</p>

<p align="center">
  <a href="https://github.com/nunyavpn/nunya/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/nunyavpn/nunya/actions/workflows/ci.yml/badge.svg"></a>
  <img alt="macOS and Linux" src="https://img.shields.io/badge/platforms-macOS%20%C2%B7%20Linux-4c6ef5">
  <img alt="Beta" src="https://img.shields.io/badge/status-beta-f59f00">
  <a href="LICENSE"><img alt="GPL-3.0" src="https://img.shields.io/badge/license-GPL--3.0-2f9e44"></a>
</p>

<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="docs/screenshots/main-dark.png">
    <img src="docs/screenshots/main.png" width="880" alt="Nunya connected through a server in Helsinki: the server list, the route from the user to the exit on the map, and the status card with live traffic and the exit's address">
  </picture>
</p>

<p align="center"><sub>Screenshots show made-up example servers.</sub></p>

## Why Nunya

Nunya connects you through the VPN and proxy servers you already have, whether that's links a
provider sent you, a subscription or a WireGuard config, and tells you plainly what it's doing.

| | |
| --- | --- |
| **Safe** | Nunya never claims more than it covers. In VPN mode the whole device goes through the tunnel; in proxy mode it tells you exactly which apps are covered, and which aren't. |
| **Fast** | Every server is tested end to end: how quickly it answers, and where your traffic really comes out. Quick Connect takes you to the fastest, the most used or the most recent. |
| **Reliable** | A server only counts as working if traffic actually gets through it, and the shield in the corner turns red when a connection is up but nothing comes out of it. |
| **Secure** | No account and no telemetry. Your servers stay on your device, the connection engine is a verified release, and the app and the engine check each other's identity. |
| **Easy to use** | Paste a link or a subscription, scan a QR code, and connect. |

## Features

- **Two ways to connect.** *VPN mode* carries the whole device. *Proxy mode* opens a local SOCKS and
  HTTP port, and can set it as your system proxy while you're connected (macOS, GNOME and KDE),
  putting your own settings back exactly when you disconnect.
- **Every kind of link, in one box.** VLESS, VMess, Trojan and WireGuard links; subscriptions,
  including Xray, sing-box and Clash configurations; import links; WireGuard configs; QR codes.
- **Servers you can trust at a glance.** Each server is tested for latency and located by where its
  traffic really exits, not by its name. Relays and CDN-fronted servers are marked as such.
- **A live map.** Your location, the servers, and the route your connection takes.
- **Quick Connect.** Choose the fastest server, the one you use most, or the one you used last.
- **Usage.** How much each server and subscription has carried, day by day, beside what your
  provider reports.
- **Sharing.** Send a server to another device as a link or a QR code, or as a WireGuard config the
  official WireGuard apps can scan.
- **Bypass rules.** Keep chosen domains, addresses and ranges off the tunnel. Your local network
  always is.
- **Light and dark**, following your system.

## A tour

### The main window

The **server list** on the left groups your servers by where they came from: the ones you added
yourself, then each subscription, with its data allowance and when it was last updated. Each row
shows the server's flag and city (where its traffic really comes out), how it connects, and its
latency. The **map** shows you, your servers, and the route of your connection. The **status card**
at the bottom shows the connection, live traffic, and the public address the internet sees.

**To connect:** pick a server in the list (or a dot on the map) and press **Connect**.

The icons down the left edge are the **status shield** (green connected, amber connecting, grey
off, red not working), then the server list, bypass rules, settings, support, and diagnostics.

### Adding servers

<p align="center"><img src="docs/screenshots/add-servers.png" width="420" alt="The Add servers sheet, having found a subscription, a WireGuard config and a VLESS server in pasted text"></p>

Press **+** above the list and paste whatever your provider gave you. One paste can hold several
things at once, and Nunya sorts them before anything is added:

- **Share links**: `vless://`, `vmess://`, `trojan://`, `wireguard://`
- **Subscriptions**: an `https://` address, or a panel's *import to sing-box / Clash* link. Each
  gets its own group, which you can update later.
- **WireGuard configs**: the `[Interface]` / `[Peer]` text the WireGuard apps export

Anything Nunya can't run yet is named, with the reason, rather than silently dropped. The **QR
code** tab reads a code from a screenshot or an image, and **Manual** lets you fill in a server by
hand. New servers are tested and located straight away.

### Quick Connect

<p align="center"><img src="docs/screenshots/quick-connect.png" width="420" alt="The Quick Connect prompt offering the fastest, the most used and the most recent server"></p>

**Quick Connect**, at the top of the list, lets you choose by what matters this time:

- **Fastest**: the lowest latency among servers that passed their last test. If that result is
  old, the fastest few are tested again first.
- **Most used**: the server that carried the most data in the last 30 days.
- **Most recent**: the server you were last connected to.

### Checking servers

Servers are tested when they're added and whenever their subscription updates. To test one again,
use its **⋯** menu → **Check**. A test measures latency *and* makes a real request through the
server, so the flag shows where your traffic actually comes out. A server that relays through
another country shows both, as in "via NL". A dash means untested or unreachable; hover over it to
see why.

### Usage

<p align="center"><img src="docs/screenshots/usage.png" width="420" alt="The usage of a subscription: totals for 30 days and all time, a daily chart, and a breakdown by server"></p>

Press the chart button on a subscription, or choose **⋯ → Usage** on a server, to see what it has
carried: the last 30 days and all time, a daily chart, and, for a subscription, each server's
share. Where the provider reports your allowance, its figure is shown too. Usage is counted on your
device only, and is kept for as long as the server is in your list.

### Sharing a server

<p align="center"><img src="docs/screenshots/share.png" width="420" alt="The Share sheet with a QR code and the server's share link"></p>

**⋯ → Share** shows a server as a QR code and a link, ready to scan on a phone or import in Nunya on
another device. A WireGuard server can also be shared as a WireGuard config, which the official
WireGuard apps scan. A share link contains the server's credentials, so share it only with people
you'd give access to.

### Settings: VPN or proxy

<p align="center"><img src="docs/screenshots/settings.png" width="720" alt="The Advanced settings in proxy mode: port, Allow LAN, Set system proxy, and the DNS resolver"></p>

- **Proxy mode** (the default) opens a SOCKS and HTTP port on your machine (2080 unless you change
  it). Apps you point at it go through the connection. Turn on **Set system proxy** to point your
  desktop's proxy setting at it while you're connected; your previous setting comes back when you
  disconnect. Apps that ignore the system proxy aren't covered, and Nunya says so.
- **VPN mode** carries all of the device's traffic through the tunnel. It needs system privileges
  this beta doesn't set up for you yet: on macOS, the signed system extension, and on Linux,
  network-admin rights for the engine. Both are coming in later releases.
- **Allow LAN** lets other devices on your network use the proxy. **DNS** sets the resolver used
  inside the connection.

Settings apply when you connect, so they're locked while a connection is running.

### Bypass rules

<p align="center"><img src="docs/screenshots/rules.png" width="720" alt="Bypass rules for domains, an address and a range, beside the local network ranges that are always bypassed"></p>

Anything matching a bypass rule leaves on your normal connection: a domain (with or without
`*.`), an address, or a range such as `10.0.0.0/8`. Your local network is always bypassed.

### Diagnostics

The pulse icon at the bottom of the left edge shows the engine's log, the state of the connection
and your data, and the exact configuration a server would run with. It's the place to look first
when something doesn't connect, and what to include in a bug report.

## Supported protocols

| | |
| --- | --- |
| **Protocols** | VLESS, VMess, Trojan, WireGuard (Cloudflare WARP included) |
| **Transports** | TCP, WebSocket, gRPC, HTTP/2, HTTPUpgrade, QUIC |
| **Security** | TLS and Reality, with browser fingerprints and ALPN |
| **Subscriptions** | lists of share links (plain or base64), Xray, sing-box and Clash (JSON) configurations, and sing-box and Clash import links |
| **Coming** | Shadowsocks, Hysteria2, TUIC, SSH, AmneziaWG, proxy chains, and more ([roadmap](#roadmap)) |

## Privacy and security

- **No account, no telemetry, no analytics.**
- **Your data stays on your device.** Servers, subscriptions and usage are stored in a file only
  your user account can read. A subscription address is treated as the credential it is.
- **A verified engine.** Nunya runs a pinned release of [nunya-core](https://github.com/nunyavpn/nunya-core),
  checked against its published checksums, and the app and the engine verify each other's
  identity when they connect.
- **Honest coverage.** The app only says you're protected when the whole device is in the tunnel.
- **Location lookups.** To place servers on the map, Nunya asks public IP-location services where
  your servers' addresses are, and where your own public address is, to draw your dot. The map
  itself is drawn from data inside the app; no map service is contacted.

## Installing

Nunya is in **beta**. Builds are published on the
[Releases](https://github.com/nunyavpn/nunya/releases) page, each with a `SHA256SUMS` to check your
download against.

- **macOS** (Apple Silicon, macOS 12 or later): open the `.dmg` and drag Nunya into Applications.
  The beta isn't signed by an Apple Developer account yet, so the first time you open it, go to
  **System Settings → Privacy & Security** and choose **Open Anyway**.
- **Linux** (x86-64 and arm64): install the `.deb` with `sudo apt install ./Nunya_<version>_<arch>.deb`,
  or make the `.AppImage` executable and run it.
- **Windows**: planned ([#34](https://github.com/nunyavpn/nunya/issues/34)).

This beta is built around **proxy mode**, on both platforms. VPN mode follows once the app can set
up the privileges it needs: the signed system extension on macOS, network-admin rights on Linux.

## Roadmap

- More protocols: Shadowsocks, Hysteria2, TUIC, SSH, AmneziaWG, and more
- Proxy chains, routing rules and a built-in ad blocker
- VPN mode out of the box on macOS and Linux, and Windows support
- Cloudflare WARP: create and register configs from the app
- On the server side: **nunya-server-core** (Docker), **nunya-cluster** (auto-scaling across nodes)
  and **nunya-server-panel** (config and user management)

Follow along, or suggest something, in the [issues](https://github.com/nunyavpn/nunya/issues).

## Contributing

Bug reports, ideas and pull requests are welcome. Building from source, running in development and
mock modes, tests, and how the app is put together are all in **[CONTRIBUTING.md](CONTRIBUTING.md)**.

## Supporting Nunya

Nunya is free and open source. Donations are being set up, through two channels only: **Buy Me a
Coffee** and **crypto wallets**. They will be listed here and in the app's Support panel (the heart
in the left edge) once they are open.

Until then, and anywhere else afterwards, anyone asking for payment in Nunya's name is not us.

## License

Nunya is free software under the [GNU General Public License v3.0](LICENSE). It began as a fork of
[Throne](https://github.com/throneproj/Throne).
