# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Nunya is a **TUN-only** desktop VPN client for [nunya-core](https://github.com/nunyavpn/nunya-core),
built as a Tauri v2 app (TypeScript frontend, Rust shell, Swift packet tunnel extension on macOS).
Both repos are GPL-3.0 forks of [Throne](https://github.com/throneproj/Throne).

The single-transport constraint is the product thesis, not a limitation to route around: with no
proxy mode there is no partial coverage, so "the device is in the tunnel" is a claim the UI can make
honestly. Do not add a proxy/mixed inbound. Deliberately out of scope: system proxy, OTP, global
hotkeys, speed tests, WARP registration, the dashboard installer, diagnostics capture.

`README.md` is the long-form rationale and is unusually complete — read it before any substantial
change, and keep it current when you change behaviour it describes.

## Setup and commands

Nothing compiles until the core is on disk: `src-tauri/build.rs` generates the RPC bindings from
`vendor/core/proto/nunya.proto`, and Tauri refuses to build (even `cargo check`) while the sidecar
named in `externalBin` is missing. Both are installed by the fetch script.

```bash
npm install
./scripts/fetch-core.sh              # install the pinned core into vendor/core/ (gitignored)
npm run tauri dev
```

| Task | Command |
| --- | --- |
| Typecheck + build frontend | `npm run build` (`tsc --noEmit && vite build`) |
| Lint Rust | `cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` |
| Test Rust | `cargo test --manifest-path src-tauri/Cargo.toml` |
| One test | `cargo test --manifest-path src-tauri/Cargo.toml <name>` |
| Frontend in a browser, no Rust | `VITE_MOCK=1 npm run dev` |
| App with the list full | `VITE_MOCK=1 npm run tauri dev` |
| Style guide | `npm run design` |
| Release bundle | `./scripts/build-app.sh` |

There is no JS linter and **no `cargo fmt` gate** — the tree is not rustfmt-clean, so do not
reformat files you are not otherwise editing.

### Tests that need more than a checkout

Tests requiring a live core or a real TUN are `#[ignore]`d, so the default run is the pure ones.

```bash
# needs a built core
./scripts/fetch-core.sh --source ../nunya-core
NUNYA_CORE_PATH="$PWD/vendor/core/bin/nunya-core" \
  cargo test --manifest-path src-tauri/Cargo.toml -- --ignored --nocapture

# needs CAP_NET_ADMIN and /dev/net/tun — runs in a container, no root on the host
./scripts/dev-linux.sh                    # everything, including tests/tunnel_linux.rs
./scripts/dev-linux.sh test <name>        # one test or one target
./scripts/dev-linux.sh shell
```

`dev-linux.sh` is the preferred way to exercise a real tunnel. It keeps **the core as a child of the
Rust side, both inside the container**, so `rpc/peer.rs` verification runs unchanged; splitting
GUI-here/core-there would break it, because peer identity is a pid over a unix socket. It does not
run the UI (WebKitGTK over X11 is a worse loop than the browser one) — use `VITE_MOCK=1` for that.

`./scripts/dev-tunnel.sh` is a last resort: it runs the *whole app* under `sudo -E`, which inherits
your `HOME`, so the first save makes `data.json` root-owned and every later unprivileged write
fails silently.

### Transports

```bash
NUNYA_TRANSPORT=networkextension npm run tauri dev
```

The default is `subprocess` on every platform and must stay that way until the extension can be
signed — see [transport/select.rs](src-tauri/src/transport/select.rs). Defaulting to
NetworkExtension would leave anyone without a paid Apple Developer membership looking at a tunnel
that silently never starts.

## Architecture

### The two-repository split

This repo never builds the core. `core.lock` pins a nunya-core release plus the SHA256 of that
release's `SHA256SUMS`, so pinning one digest makes the whole artefact set tamper-evident;
`fetch-core.sh` verifies every asset against it. A re-cut release fails the check instead of
installing. Repin with `./scripts/fetch-core.sh --update <tag>` and commit the lockfile.

A core built with `--source` carries the `noparentcheck` build tag (during development the parent
process is `cargo`, not `Nunya`). `build-app.sh` refuses to bundle one, and none is ever published —
a downloadable core with its parent check off is a root-capable binary anything local could drive.

### The IPC is not gRPC

Despite `service NunyaCoreService` in the proto, nothing on the wire is gRPC. The proto exists only
to generate message types for both sides. The framing is two little-endian frames over a unix
socket, implemented and unit-tested in [rpc/codec.rs](src-tauri/src/rpc/codec.rs):

```text
request   [u32 id][u16 method_len][method][u32 payload_len][protobuf]
response  [u32 id][u8  status    ][u32 payload_len][protobuf or error text]
```

Two things invert the usual expectations:

- **The GUI listens and the core dials in.** Startup order is bind → spawn → accept, and the core is
  a child process of the app. `expect_core_pid` must be called before the first accept.
- **Both ends verify each other.** The core checks its listener's owning process is its own parent;
  [rpc/peer.rs](src-tauri/src/rpc/peer.rs) adds the symmetric check, so a local process that won the
  race to the socket cannot impersonate the core and report a healthy tunnel that does not exist.

Method names are string keys into a Go handler map — a typo is a runtime "unknown method", so they
live only in `rpc::method`. The proto is proto2, so scalars arrive as `Option<T>`.

### The transport seam

[transport/mod.rs](src-tauri/src/transport/mod.rs) defines one trait, `TunnelTransport`, kept free
of anything platform-shaped (no process handles, fds or socket paths). Everything above it — config
generation, share-link parsing, the entire UI — is shared; everything below is per-platform. The
subprocess transport is implemented and tested; the NetworkExtension one is a skeleton.

macOS uses a real system VPN (NetworkExtension), the same mechanism WireGuard and Tailscale use:
nothing runs as root, nothing is setuid. The app side is only the control plane —
`macos/tunnel_manager.m`, compiled by `build.rs`; the tunnel itself lives in the `.appex`.

### Where things actually live (Rust)

`main.rs` is a five-line shim. **All Tauri commands and the startup sequence are in
[lib.rs](src-tauri/src/lib.rs)**, as a library so integration tests use the same modules the binary
does. (The README's layout table still says `main.rs` — it is wrong on that one point.)

- [config.rs](src-tauri/src/config.rs) — the only file that assumes *which* core is running. It
  emits sing-box JSON, always validated by the core's `CheckConfig` before anything starts. The
  long-term plan is to move generation into the core behind a `GenerateConfig` RPC.
- [storage.rs](src-tauri/src/storage.rs) — the data file holds every server credential, so it is
  written like a key file: owner-only dir and file, replaced atomically. Payload is opaque JSON; the
  schema lives in the frontend. A corrupt file is an error, never a silent reset.
- [subscription.rs](src-tauri/src/subscription.rs) — fetching is in Rust because a subscription URL
  is a credential (it must not sit in the webview's network log) and because the request has to
  travel through the tunnel when one is up. `http://` and non-HTTP schemes are refused before any
  request is made.

### Frontend

No framework, deliberately — four screens and one list that changes on refresh does not justify a
runtime dependency in a client where every shipped byte is something a user has to trust.
[dom.ts](src/dom.ts) is the whole abstraction (`h`, `render`, `qs`).

Data flows one way: every mutation goes through `store.update()`, which persists and notifies in one
place, so a view can never change state without the rest of the app hearing about it. Views in
[src/views/](src/views/) render from the store and call back up to [main.ts](src/main.ts); they know
nothing about persistence or Tauri.

[persist.ts](src/persist.ts) picks one of three backends behind one interface — the Rust-owned file
in the app, `localStorage` in a browser, a discard-everything fixture under `VITE_MOCK=1` — and
debounces writes. [bridge.ts](src/bridge.ts) makes `invoke`/`listen` degrade instead of throwing
outside Tauri, which is what lets the UI be developed in a browser.

`VITE_MOCK` is substituted at build time, so `npm run build` drops [mock.ts](src/mock.ts) entirely
rather than shipping plausible-looking servers. Every host in the fixture is under `example.net`
(RFC 2606 reserved, unregistrable).

### The application icon

The artwork lives at `design/nonya.png`; everything under `src-tauri/icons/` is generated from it
with `npm run tauri icon <master>` and should never be edited by hand.

The source is not the master. It is 1254x1254, RGB with **no alpha**, and its rounded square is
surrounded by pure black rather than transparency — which macOS renders as a black tile behind
the icon in the Dock. Preparing a master means flood-filling that surround to transparent from
the corners inward (a plain threshold eats the icon's own near-black navy body), then fitting the
artwork to 824px inside a transparent 1024x1024 canvas, which is Apple's grid. Skipping the inset
leaves the icon visibly larger than every other icon in the Dock.

`bundle.icon` in `tauri.conf.json` lists the five files the desktop bundle uses. `tauri icon` also
emits Android and iOS sets, which this project has no targets for and which are deleted after
generating; the `Square*Logo.png` and `StoreLogo.png` files are part of the original scaffold and
are kept.

`public/favicon.png` is the same artwork at 64px, for the webview's tab and window.

### Modes

`Settings.mode` is `proxy` or `vpn`, and it is **not** a preference between two equivalent things.
VPN mode builds a TUN and carries the whole device; proxy mode opens a `mixed` SOCKS/HTTP listener
and carries only what is pointed at it. `config::build` branches on it: a different inbound, and no
`hijack-dns` rule in proxy mode, because only a TUN sees the system's DNS traffic to hijack.

**Proxy is the default**, because it is the mode that works: a TUN needs privilege this build
cannot obtain on macOS without a signed packet tunnel extension, so defaulting to VPN would hand a
new user a Connect button that fails. `TunnelTransport::availability` takes the mode for the same
reason — proxy mode binds an unprivileged port and must not be refused for want of a permission it
never uses.

This repository's README was written around a TUN-only thesis: "with no proxy mode there is no
partial coverage, so *the device is in the tunnel* is a claim the UI can make honestly." Proxy mode
gives that claim up, so the status card must not make it. `HEADLINE` is keyed by mode — "You're
protected" exists only for VPN — and in proxy mode the chips lead with the listen address and say
"Only apps set to use it" in place of the "DNS no leak" badge. Both would be false for everything
not pointed at the listener, which is most of the machine. Treat that copy as load-bearing, not
decoration.

### Locating servers (the flags)

The flag beside a server used to be guessed from the share link's name — whatever the provider
typed. `geo::locate` measures it instead, and `main.ts` runs it after a latency sweep for the
servers that answered.

**A measurement changes the flag and nothing else.** It writes `Server.exitCountry`, which the
flag chip and the map pin read; the row's label keeps using `Server.country`, which is read off
the name. The two disagree constantly — a provider's "Iran" routinely exits in the Netherlands —
and overwriting the label would silently rename the user's servers, which is not what a latency
sweep is for. `replaceSubscriptionServers` preserves `exitCountry` across a refresh.

**The core cannot supply this.** `IPTest` and `SpeedTest` (with `only_country`) look up geo
internally, against endpoints compiled in — `api.ip2location.io` and `speedtest.net` — both
Cloudflare-fronted and therefore unreachable from a Cloudflare Workers proxy, which is the shape
most free subscriptions take. Neither request message has a URL field. Measured against a real
subscription, both answered for 2 servers out of 10.

So the client asks itself, in **two steps**, and the split is the whole design:

1. **Through each server**, ask only *what address am I coming out of* — `checkip.amazonaws.com`,
   falling back to `ifconfig.me/ip`. This needs a proxy port per server, which is what
   `config::build_probe` emits (one `mixed` inbound per profile, pinned by a route rule to that
   profile's outbound) and what `geo::locate` runs in **its own short-lived core process** —
   a second instance with its own socket, not a reconfiguration of the running core.
2. **Directly, from the user's own connection**, turn each *distinct* address into a country.

Doing it in one step is the obvious design and it fails. Servers share exits — four of ten in a
real subscription came back on one Cloudflare address — so asking a geo service through every
server puts the whole sweep on three or four client IPs and most requests come back `429`. That
version located 2 of 10 while all 8 reachable servers could reach the endpoint perfectly well.
Splitting it means the rate-limited half runs once per distinct exit, from an address the user
owns. The two-step version locates 8 of 10, which is every server that is up.

Both halves are endpoint *chains*, for the same reason: a free tier is exhaustible, and one day
of testing exhausted `ipinfo.io` for this machine entirely. Expect the resolvers to disagree
sometimes — a Cloudflare anycast address has no single physical location.

### Latency testing

`test_servers` in [lib.rs](src-tauri/src/lib.rs) measures against `TEST_URLS` **in order**, retrying
only the servers that failed, because no single endpoint reaches every server and the failures are
structural rather than flaky:

- A Cloudflare Workers proxy — what most free VLESS and Trojan subscriptions are — cannot make a
  subrequest to Cloudflare's own addresses, so it can never reach `cp.cloudflare.com`.
- A WARP endpoint egresses somewhere that frequently cannot reach Google, so `gstatic.com` fails
  for exactly the servers a Cloudflare endpoint suits.

Measured against a real subscription, `cp.cloudflare.com` timed out for all eight Workers servers
while both WARP endpoints answered, and `gstatic.com` was the precise inverse. It was the default,
so a working subscription reported every server unreachable. Do not reduce the list to one entry or
put a Cloudflare endpoint first; a unit test guards both.

A caller that passes its own `url` gets that one endpoint and no fallback — an explicit choice is
not second-guessed.

The list shows `—` for both "never tested" and "unreachable", separated only by colour, so
`Server.latencyError` carries the reason and the row states it on hover. Without it there is no way
to tell a dead server from one the test endpoint could not reach.

### Editing and deleting

Every modal goes through `openSheet` in [main.ts](src/main.ts), which owns the scrim, the
click-outside, Escape, listener cleanup and focusing the first field. Adding a sheet means writing
its contents, not its plumbing.

Editing a server is a **form over the profile**, in [views/editor.ts](src/views/editor.ts) — never
a share link in a text box. A link is a serialization: changing a port by finding it between an `@`
and a `?` makes the user the parser, and a typo there does not fail, it produces a different
server. The profile is already structured data on disk, so `ProfileEditor` edits that. The
generated link is shown read-only at the bottom of the sheet as an export path, which is also what
makes the form's effect legible.

`ProfileEditor` keeps a deep-copied draft, so Cancel costs nothing, and **rebuilds** rather than
diffs when a structural choice changes — the protocol, the transport, the security layer — because
those decide which fields exist. Fields belonging to other protocols are kept, not cleared: both
the config builder and the link writer emit them only where they apply, so carrying an unused
`flow` is free, while clearing it would discard a setting for anyone who clicked through the
control to look. Its controls reuse the Advanced panel's classes (`set-group`, `srow`, `slab`,
`val`, `seg`) so the two read as one app.

`toShareLink` in [share.ts](src/share.ts) is the inverse of `parseShareLink`, used for that export
line and for moving a server to another client. Round-tripping is the contract:
`parseShareLink(toShareLink(p))` must equal `p`, which is why the WebSocket `?ed=N` parameter is
written back into the path it was lifted out of.

The name is a separate field, and an explicit rename sets `Server.renamed`. The list normally shows
a row's *country* rather than its profile name, because a share link's name is usually the country
and city restated — but a name someone typed is the one thing that is certainly not redundant, and
nothing distinguishes the two after the fact except recording it.

Deletion always confirms, and the sheet states what is lost rather than asking "are you sure?":
whether the server returns on the next subscription update or is gone for good, that a subscription
URL is a credential with no other copy, and whether the tunnel is currently running on the target
(in which case it disconnects).

One CSS trap worth not repeating: row action buttons are revealed with `opacity` on
`:hover`/`:focus-within`, and are **not** gated with `pointer-events: none`. That reads as the safer
choice and is the opposite — the buttons sit inside the row, so a cursor can only reach them when
they are already visible, while gating on `:hover` makes them unreachable to anything that
hit-tests before it moves, which is every automation tool.

## Conventions

- **Comments explain why, not what.** Every module opens with a rationale header covering the
  decision and the alternative rejected. Match this — it is the codebase's defining characteristic,
  and a change that arrives without it will read as foreign.
- **Reject by name; never silently downgrade.** A share link with an unrunnable transport
  (mKCP, XHTTP, SplitHTTP, meek) is refused with a reason. Quietly treating it as TCP produces a
  config the core accepts and a tunnel that never passes traffic.
- **Test names are sentences** — `the_tunnel_takes_the_default_route_and_gives_it_back`.
- **Do not edit `.xcodeproj`**: it is generated from `project.yml` by XcodeGen and gitignored.
- Emitted config shapes differ from what unit tests here can see (`host` is a string for HTTPUpgrade
  and a list for HTTP/2; plain TCP means *no* `transport` key). Every transport is therefore checked
  against a real core in `tests/core_link.rs::every_transport_is_accepted_by_the_core`.

## Current state

Working: toolchain, checksum-pinned core install, IPC codec with peer verification, config
generation and validation, connect/disconnect, throughput polling, VLESS, VMess, Trojan and
WireGuard parsing over TCP, WebSocket, gRPC, HTTP/2, HTTPUpgrade and QUIC (none/TLS/Reality, uTLS
fingerprints, ALPN), the transport seam, the core's Darwin TUN-descriptor support.

**WireGuard is an endpoint, not an outbound.** sing-box moved it to `endpoints` — it is an
interface with its own addresses rather than a dialer — and this core rejects the old outbound form
with *unknown field "local_address"*. `config::proxy_node` returns `ProxyNode::Outbound` or
`ProxyNode::Endpoint` so `build` and `build_test` put it in the right array; the router names a tag
either way. Cloudflare WARP rides this path, and its `reserved` client id must survive: a wrong
value is dropped by the server silently, producing a tunnel that comes up and carries nothing.

Trojan is the URL form with a password where the UUID goes. Emitting a `uuid` field alongside it
is an unknown field the core refuses outright, so `proxy_outbound` sets the credential per arm.

Blocked on Xcode and an Apple Developer account: the Swift packet tunnel provider, the
`.xcframework` build, the `.appex` target, `NETunnelProviderManager` wiring.

Not yet built: bypass rules in the UI, the menu-bar popover, protocols beyond VLESS and VMess
(Trojan, Shadowsocks, Hysteria2, TUIC are separate outbound types, not more transports), and bundled
fonts — the CSP forbids remote font hosts, so Manrope must ship in the bundle.

### Subscriptions

A subscription URL is pasted into the same "Add servers" box as share links; `classify` in
[main.ts](src/main.ts) sorts each pasted line into a server, a subscription, or a rejection.
An `https://` line is taken as a subscription — which means a pasted `https://` *proxy* link is no
longer reachable, an acceptable trade in a TUN-only client that does not run HTTP proxies anyway.
The group is created before the first fetch, so a dead endpoint shows up as a row carrying an error
rather than a dialog that hangs, and re-pasting a known URL refreshes it instead of duplicating it.

**Two body formats, told apart by content because no header distinguishes them.** The common one is
a list of share links, optionally base64-encoded. The other is a JSON Xray configuration, or an
array of them — what a BPB panel serves to `?app=xray`: each entry is a whole client config
(inbounds, routing, DNS) wrapped around one `proxy` outbound. `xray_config_links` in
[subscription.rs](src-tauri/src/subscription.rs) rewrites those outbounds back into share links, so
everything downstream is unchanged and the frontend still reports rejections per entry.

Three rules there are load-bearing:

- **A body that parses as JSON never falls back to the line scan.** JSON is never a list of share
  links, and scanning it anyway finds the `://` inside a DNS address and reports a "server" made of
  configuration fragments.
- **`subscription::carrier` decides what a configuration is actually offering**, and the three
  cases are not distinguishable by tag alone. A chain — one outbound naming another in
  `sockopt.dialerProxy`, which is what BPB's "WoW" (WARP-over-WARP) entries are — is refused by
  name, because reducing it to its last hop would connect somewhere the entry's own name
  contradicts. A balancer is *several* interchangeable `proxy-N` outbounds behind a `leastPing`
  selector, and is skipped because those servers appear individually elsewhere in the same array.
  The balancer test turns on the **number of candidates, not the tag's spelling**: BPB's WARP
  subscription has a "Best Ping" entry holding exactly one `proxy-1`, which is a real configuration
  and not a duplicate of anything.
- **Protocols this build cannot run are still emitted under their own scheme**, so the frontend
  names them ("Trojan is not supported yet") instead of handing back a list quietly shorter than
  the provider's. This extends to protocols whose *shape* is unknown: WireGuard keeps its endpoint
  under `peers` rather than `servers`, so the usual address lookup finds nothing, and a subscription
  made only of such entries would otherwise come back as "nothing that looks like a server list",
  indistinguishable from a broken link. Those entries get a marker link carrying the scheme and
  endpoint, which is enough for the frontend to refuse them by name. (WireGuard itself is now
  encoded properly by `wireguard_link`; the marker path is for whatever turns up next.)

Query values must be percent-encoded on the way out: a WebSocket path is routinely
`/vl/abc?ed=2560`, whose `?` would otherwise terminate the query it is written into.
