# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Nunya is a **TUN-only** desktop VPN client for [nunya-core](https://github.com/nunyavpn/nunya-core),
built as a Tauri v2 app (TypeScript frontend, Rust shell, Swift packet tunnel extension on macOS).
Both repos are GPL-3.0 forks of [Throne](https://github.com/throneproj/Throne).

The single-transport constraint is the product thesis, not a limitation to route around: with no
proxy mode there is no partial coverage, so "the device is in the tunnel" is a claim the UI can make
honestly. Proxy mode (a `mixed` inbound, optionally set as the system proxy) has since been added
for machines where a TUN cannot be had; see **Modes** for what that costs the UI's claims.
Deliberately out of scope: OTP, global hotkeys, speed tests, WARP registration, the dashboard
installer, diagnostics capture.

`README.md` is for users: what the app does and how to use it, with screenshots from the mock
fixture (`docs/screenshots/`). `CONTRIBUTING.md` is the developer's long-form rationale — building,
dev and mock modes, the core link, macOS privilege, releasing and the issue → branch → PR flow —
read it before any substantial change. Keep both current when you change behaviour they describe,
and retake a screenshot when its screen changes (`VITE_MOCK=1 npm run dev`; `mockcore.ts` lets the
browser connect).

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
| Test frontend logic | `npm test` (Node's own `node --test`, no dependency) |
| Frontend in a browser, no Rust | `VITE_MOCK=1 npm run dev` |
| App with the list full | `VITE_MOCK=1 npm run tauri dev` |
| Style guide | `npm run design` |
| Local bundle, as CI releases it | `npm run tauri build -- --bundles app,dmg` (macOS) · `deb,appimage` (Linux) |
| Signed bundle with the packet tunnel | `./scripts/build-app.sh` (needs an Apple Developer team) |
| Release | bump the version in three places, merge, `git tag vX.Y.Z && git push origin vX.Y.Z` |

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

**A release core's parent must be a binary named exactly `Nunya` in the same directory**
(`Nunya.exe` on Windows) — nunya-core's `internal/parentcheck`. That is why `mainBinaryName` is
`Nunya`: the bundles put the core beside it (`Contents/MacOS/`, `/usr/bin/`, the AppImage's
`usr/bin/`), and a renamed binary would leave the app unable to start its own core. It also means
`npm run tauri dev` cannot run a *release* core (its parent is `target/debug/nunya`); develop against
one built with `--source`.

**Releases** are tags: `release.yml` builds and publishes on `vX.Y.Z` (see CONTRIBUTING.md's
*Releasing*). The default macOS build is ad-hoc signed and carries **no** entitlements; the
NetworkExtension ones need an Apple Developer team, and macOS kills an app that claims them without
one — they are merged in only by `build-app.sh`, from `tauri.networkextension.conf.json`.
`fetch-core.sh` fetches `NunyaCore.xcframework.zip` only when the release lists it.

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

**Until the extension can be signed, VPN mode on macOS is a setuid-root core.** The status card's
**Allow…** calls `request_permission`, which runs `core_proc::grant_root` (the administrator
password through `osascript`, then `chown root:wheel` and `chmod 4755` on the bundled core) and
restarts the core. Setuid rather than `sudo`, because anything wrapping the core becomes its parent
and fails both the core's parent check and `rpc/peer.rs`. Only a core beside `Nunya` is granted,
since the release core's parent check is the only thing keeping other programs from driving it,
and `build.rs` refuses a release build over a `noparentcheck` core, so no bundle carries one. See CONTRIBUTING.md's *Privilege today*.

### Where things actually live (Rust)

`main.rs` is a five-line shim. **All Tauri commands and the startup sequence are in
[lib.rs](src-tauri/src/lib.rs)**, as a library so integration tests use the same modules the binary
does.

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

The project was first written around a TUN-only thesis: "with no proxy mode there is no
partial coverage, so *the device is in the tunnel* is a claim the UI can make honestly." Proxy mode
gives that claim up, so the status card must not make it. `HEADLINE` is keyed by mode — "You're
protected" exists only for VPN — and in proxy mode the chips lead with the listen address and say
"Only apps set to use it" in place of the "DNS no leak" badge. Both would be false for everything
not pointed at the listener, which is most of the machine. Treat that copy as load-bearing, not
decoration.

### The system proxy

`Settings.systemProxy` (proxy mode, off by default) points every proxy slot the desktop has —
HTTP, HTTPS, FTP and SOCKS — at `127.0.0.1:<port>` after connect, and puts them back on disconnect.
One port serves all of them because the listener is a `mixed` inbound; an empty slot is a class of
app that silently goes around the tunnel. On GNOME the legacy `http.enabled` key is set too, since
older readers treat HTTP as off without it. [sysproxy.rs](src-tauri/src/sysproxy.rs)
shells out to the desktop's own tool — `gsettings` (GNOME family), `kwriteconfig6/5` (KDE),
`networksetup` (macOS), `reg.exe` on `HKCU\…\Internet Settings` (Windows) — and refuses any other
desktop by name.

**macOS has no FTP slot any more**: current `networksetup` has dropped `-getftpproxy`/`-setftpproxy`
and answers them with its usage text and `** Error: The command is not recognized.` Asking for it
failed every capture, which is why the system proxy was never set on a Mac. `networksetup` also
exits 0 on errors like that, so `run()` treats `** Error` output as a failure; `apply` sets every
service without stopping at one that refuses (VPN configurations are services too), then **checks
`scutil --proxy`** — what apps are actually given — and succeeds only if the listener is there.
Restore writes each address back even when it was empty (`-setwebproxy <svc> "" 0`), so no
`127.0.0.1` is left behind in a disabled slot. Tools are called by full path: an app started from
Finder has a minimal `PATH`. `the_mac_proxy_is_set_and_then_put_back_exactly` runs the real cycle on
a Mac, but only with `NUNYA_TOUCH_SYSTEM_PROXY=1`.

**Windows** sets `ProxyEnable`, `ProxyServer` (one `127.0.0.1:<port>` for every scheme, as the
settings page writes it) and `ProxyOverride`, and sets a PAC `AutoConfigURL` aside while ours is
applied, since a PAC takes precedence. The whole key is read at once, so a missing value is absent
rather than a localised error message. It is unit-tested everywhere but has not run on Windows:
the app does not build there yet (the core link is a Unix socket).

**Restoring is the load-bearing half.** A system proxy left pointing at a dead port breaks every
browser on the machine. So the previous settings are captured *before* anything changes, written to
`system-proxy.json` in the data directory, and replayed (not merely switched off — a user's own
proxy comes back) on disconnect, in `stop_tunnel`, on quit, when the core dies, and at the next
launch if the app crashed holding them. Capture happens once per application of ours, so
re-applying never records our own settings as "the user's".

A failure to set it does not fail the connection; the status card shows "System proxy not set".
With it set, the card still says "Apps that ignore it are not covered" — a system proxy is not the
whole machine, and only VPN mode may claim that.

`the_gnome_proxy_is_set_and_then_put_back_exactly` runs the real cycle, but only against a throwaway
keyfile backend (`GSETTINGS_BACKEND=keyfile XDG_CONFIG_HOME=$(mktemp -d)`); it refuses otherwise.

### Ad blocker and anti-tracker

`Settings.blockAds` and `Settings.blockTrackers` each switch on one list, which `config::build`
names as a `local` rule set with a DNS rule answering NXDOMAIN and a `reject` route rule, both
ahead of the bypass list. **Ads** is SagerNet/sing-geosite's `geosite-category-ads-all.srs`;
**trackers** is HaGeZi's Native Tracker lists, merged by `blocklists.rs` into one source-format
set (geosite has no tracker category: `category-public-tracker` is BitTorrent).

**The app downloads the lists, not the core.** A `remote` rule set with nothing cached fails the
core's Start when the download fails, which on a network that blocks GitHub means a Connect button
that never works. `update_blocklist` fetches a list (through the listener in proxy mode; the TUN
carries it in VPN mode), stages it beside the old one, has the core's `CheckConfig` read it, and
renames it into place only if that passes. An error page or a truncated body never replaces a good
list. `blocklists::on_disk` then names only the lists that exist, so a switch whose list has not
arrived is left out of the config, and its row says "not blocking yet" (`listLine` in
[blocking.ts](src/blocking.ts)).

A running core reloads a replaced list by itself (it watches the directory: sagernet/fswatch), so
refreshing one needs no reconnect. A list that arrives *while connected* after being missing at
connect was left out of the running config, so `syncBlockLists` in `main.ts` reconnects once to
apply it. Lists are fetched when a switch is turned on, after each connect (so one that could not
come directly comes through the tunnel), and hourly; `STALE_MS` (a day) decides which are due.
`core_link.rs` checks the published lists against a real core, and that an error page is refused.

### Connecting and disconnecting

`ConnectionState` is `off`, `connecting`, `on` and **`disconnecting`**. Both transitions take
seconds (the core, then the system proxy), and each half has an inverse that must not run in the
middle of it. A Disconnect that ran while Connect was still setting the system proxy once put the
user's settings back and then had ours land on top, pointing every browser at a port nothing
listened on. So:

- **One at a time.** `connect`, `disconnect` and `reconnect` in `main.ts` run through one `Serial`
  queue ([serial.ts](src/serial.ts)). Each checks the state when its turn comes, so a Connect
  queued behind a Connect is nothing, and a reconnect (`once("reconnect")`) only restarts a tunnel
  that is up. Several reconnects asked for while one waits are one reconnect. Disabled buttons are
  not enough on their own: the map, the popover, the tray menu and a block list arriving can all ask.
- **"On" means the system proxy is set.** `connectNow` applies it *before* switching to `on`.
  `disconnectNow` switches to `disconnecting` before awaiting anything, puts the system proxy back
  *before* stopping the tunnel (so apps go direct instead of at a closing listener), then stops it.
- **The step is on screen** (`transitionStep`: "Setting the system proxy…"): on the status card in
  place of its subtitle, and in the popover. The button stays amber with a spinner, *working* rather
  than the grey of *unavailable*, and disabled. Everything that could start another transition is
  disabled while one runs (`working()`).
- **In Rust the lock covers the whole operation.** `set_system_proxy` holds the slot's lock until
  ours are applied, and `release_system_proxy` until the user's are back (`restore_held` for a
  caller already holding it). They used to hold it only to read or write the slot, which is what
  let a restore overtake an apply, and deleted the saved copy the next launch would have used.

### Settings are locked while connected

Settings are read when the tunnel starts, so the Advanced panel is disabled (a `<fieldset disabled>`)
while connected or connecting, with a banner and a Disconnect button. Change them after
disconnecting.

The Rules, Advanced and Diagnostics panels share one container, `#panel`. Each has an `active` flag
set by `show()` in `main.ts` and renders only while active — before that, whichever re-rendered last
on a store change painted itself over the panel the user was looking at.

### The rail shield

The shield at the top of the rail (`.logo`) is the connection's state on every screen — the
panels that replace the list hide the status card — decided by `shieldState` in
[shield.ts](src/shield.ts) and painted by `paintShield` on every `refresh()`:
**green** connected (check), **amber** connecting (pulsing; steady under reduced motion), **grey**
off (slash), **red** not working (exclamation mark). Not working is a tunnel that is up with no
public exit (`exitIps.failed`), an attempt that failed, or a core that stopped while connected —
`tunnelFault` in `main.ts`, cleared only by a new attempt or a disconnect, never by itself. Not
being *ready* (core starting, no permission) is grey, not red: the status card explains it, and red
at every launch would teach users to ignore red. The tooltip follows the modes' rule — in proxy
mode it names what the listener covers, never the device.

### The tray icon

The tray icon is the app's mark in the rail shield's colours, in the macOS menu bar and the Linux
top bar — the way OpenVPN's changes colour — with, on Linux, a menu of the server, the status,
Connect/Disconnect, Show and Quit, and on macOS the popover below
([tray.rs](src-tauri/src/tray.rs)). The menu owns no connection logic: Connect emits `tray-toggle`
and the frontend runs `toggleConnection`; the frontend reports every change through
`set_tray_status` (`syncTray` in `main.ts`).

**The frontend paints the icon.** `trayIcon` in [trayicon.ts](src/trayicon.ts) fills the `mark`
icon's paths (`iconPaths`) with `Path2D` in `.logo`'s tokens — green, amber, red by the shield's
tone — and sends RGBA with the status; a Rust or PNG copy would drift. **Off is the mark at 55%**,
and on macOS a *template* (the system tints it to the menu bar). A thin stroke in the same colour
(`WEIGHT`) thickens it for 18pt. 36px on macOS (18pt at 2x), 44px elsewhere. A theme change
repaints it (`darkScheme` in `main.ts`). The status line agrees with the icon: a red tone says
"Not working", never "Connected". With the shield's check and "!" gone, colour alone tells
connected from not working in the bar; the tooltip and the popover say it in words.

**The mark is the artwork's silhouette**, not the artwork: at 18pt the painting's gradients and
dark inner rings turn to mud, so only the bright arch and road are kept. `scripts/trace-mark.py`
traces them out of `design/nonya.png` into `MARK_ARCH` and `MARK_ROAD` in `views/icons.ts` (a
brightness threshold, outlines, simplification, fitted to the 24-unit grid). Run it again when the
artwork changes; do not edit the path data by hand.

**The first status creates the tray**, not `setup`, so the full-colour app icon never flashes in
the bar before the tinted mark. Closing the window hides it only once the tray is up (`tray::is_up` in
`hide_on_close`); until then — or with no tray library on Linux — closing quits as before, because
a hidden window with no icon leaves a tunnel nobody can reach. The Dock icon (`RunEvent::Reopen`)
brings the window back on macOS. The style guide shows all four states in both appearances.

### The menu-bar popover (macOS)

A click on the tray icon opens a panel under it, NordVPN's shape: the connection and a big
switch, the server, Quick Connect, a search over the configs, Proxy / VPN, and the two blockers.
Linux keeps the menu, since a StatusNotifierItem gets no clicks to anchor a panel to.

**On macOS the status item carries no menu at all.** On macOS 27 a menu attached to it takes every
click, the left one included, before tray-icon sees it: the popover never opened and the menu did,
whatever `show_menu_on_left_click` said. tray-icon 0.25.1 fixes that (it attaches the menu only
while showing it), but Tauri 2 is held to 0.24. So the macOS `install` builds the item without
one, and either button opens the popover, which carries everything the menu did. Revisit a
right-click menu when Tauri moves to tray-icon 0.25.

[popover.rs](src-tauri/src/popover.rs) makes the window: created with the tray and hidden (a
webview that loads on the click opens half a second late), transparent (`macOSPrivateApi`, so the
page draws the rounded panel and its shadow inside `MARGIN`, which must match `--margin` in
`popover.css`), hidden again on losing focus, with `REOPEN_GUARD` so the click that closed it by
taking focus does not reopen it.

**It owns no state.** It is a second webview, and a store of its own would be a second writer to
the data file. The main window builds a `PopoverModel` ([popover-build.ts](src/popover-build.ts))
and sends it with `emitTo`; the popover sends back `PopoverIntent`s
([popover-model.ts](src/popover-model.ts)), which `onPopoverIntent` in `main.ts` answers through
the window's own paths — `toggleConnection`, `quickConnect` (shared with the Quick Connect prompt),
`connectTo`. The model carries names, places and words, never a profile, so credentials do not
travel to a webview that has no use for them. It is built only while the popover is showing
(`hello` with `shown`, and `popover-hidden` from Rust): Quick Connect's picks are worked out over
the whole list. Search runs in the main window too, which answers with at most `RESULTS_MAX`
matches through `serverMatches`, the list's own rule.

Unlike the Advanced panel, its mode and blocker switches work while connected: they reconnect, as
choosing a server does. A blocker whose list is not on disk yet is the exception — `syncBlockLists`
fetches it through the tunnel and reconnects when it lands. A mode change, from either place,
re-asks readiness (`watchMode`), since the answer is about one mode.

`VITE_MOCK=1 npm run dev` then `/popover.html` previews it in a browser; `popover-preview.ts` plays
the main window against the fixture with the real `popoverModel`, and is dropped from other builds.

### Locating servers (the flags)

The flag beside a server used to be guessed from the share link's name — whatever the provider
typed. Now every server carries **two measured locations**, both in the data file as `geo::Spot`
(ip, country, city, coordinates, `checkedAt`):

- **`entry`** — where its *address* is: the host name resolved (`geo::entry_ip`) and placed. Needs
  DNS only, so it is known seconds after a server is added, and works for a server that is down.
- **`exit`** — where its traffic *leaves*: a real request through the server, end to end, and the
  public address it came out of, placed. Needs the server to be up.

They differ whenever the server relays — a Cloudflare Workers proxy, a domestic server tunnelled on
to a foreign one — and `isRelayed` says so. The row's flag shows the **exit** (what a website
sees); a relay also gets a small entry flag on the corner and "via XX" in its subtitle; the map
dot sits at the exit city; and a connected route to a relay goes you → entry → exit.

`checkServers` in `main.ts` is the one path that measures a server: entry first (fast), then the
end-to-end test (latency and exit together, per server) — a server that fails keeps its last exit. It
runs on servers just added (link, QR or manual), after every subscription reload, and from each
row's **Check**. There is deliberately no Test all: those three already cover every server, and a
button that re-measured the whole list would only repeat them. `locate_servers` takes `entries`/`exits` flags so the two passes
do not look the same addresses up twice. Old data with `exitCountry`/`exitAt` is migrated to
`exit` on load.

Two guards exist because an exit was once recorded as **the user's own address**. The probe
config's catch-all is a `block` outbound, not `direct`, so nothing can leave from this machine;
and `geo::whereabouts` rejects an answer about any address other than the one it asked about
(`answers_for`), because a service that ignores the path describes the caller. The frontend also
drops any exit equal to `home.ip`, and `store.forgetExitsAt` clears ones saved before this.

**Which address is a Worker's exit.** Each server is asked through twice: a plain endpoint not on
Cloudflare, and Cloudflare's `/cdn-cgi/trace`. `geo::prefer_trace` saves the trace answer as the
exit when the plain one is on Cloudflare's network (AS13335) and the trace one is not — BPB's proxy
IP, a fixed machine, over the Worker's shared egress, which geo databases scatter across Europe.
The network comes from the geo services (`Whereabouts.asn`); unknown means no switch.

**The status card shows where the selected config is**, connected or not: an **Exit** chip (city,
country, data center, AS number) and an **Entry** chip when the address is elsewhere — labelled
**Address** before any test. The data center is `Spot.org` / `Whereabouts.org`, read from each geo
service's network field. Connected, the live measurement from `locate_exit` is shown *and saved*
as the config's exit, so the row's flag and the card agree.

**CDN-fronted configs.** `geo::cdn_of` marks an address as Cloudflare or Fastly by its network, or
by the providers' published ranges when no network was reported, and stores it as `Spot.cdn`. On
the **entry** that means the config connects to a CDN edge; `cdnOf()` in `store.ts` reads it, the
row shows a **CDN · CF / Fastly** tag beside the name, and it is there for a future edge-address
optimisation. Add a provider in `cdn_of` and `Cdn`/`CDN_NAMES` together.

**A Cloudflare entry is placed where it answers, never by GeoIP.** An anycast address has no place:
`104.21.77.84` is "Los Angeles" in every geo database and was measured answering from Newark on
one network and Frankfurt on another. [cloudflare.rs](src-tauri/src/cloudflare.rs) asks
(`observe_edge`): a TLS connection to the address itself with the config's name as SNI and `Host`
(`edge_host`: SNI, else transport host, else server) — `curl --resolve`, natively — reading the
data center from the `CF-Ray` suffix, else `/cdn-cgi/trace`'s `colo=`. The outcome is one of
`Edge`'s cases (observed, not observable, no host name, TLS failed, timeout, probe failed, not
Cloudflare, invalid address), and none is ever filled in from GeoIP: a colo code missing from the
table is reported as its code with no place. Its RTT is the **TLS handshake**, not the TCP connect,
because a local TUN or transparent proxy answers the SYN itself (0.26 ms connect, 660 ms TLS,
measured). Ownership (`owner`) comes from Cloudflare's published ranges and its colo table from
`speed.cloudflare.com/locations`, both compiled in from `src-tauri/data/cloudflare/` and refreshed
with `./scripts/update-cloudflare-data.sh`.

**An observation belongs to a network, not an address.** `EdgeCache` keys it by host, address and
`SourceNetwork` (the route's local address plus the public one) for `OBSERVATION_TTL`; failures are
not kept. The frontend holds observations in memory only (`edges` in `main.ts`, cleared on
`network-changed`), never in the data file, and asks only while `canLocateHome()` — in VPN mode the
probe would go through the tunnel and see the exit's edge. [edge.ts](src/edge.ts) is the one rule
for what views make of it: the Entry chip names the data center, `isRelayed` and the map's entry
hop use its country and coordinates, and an anycast entry not observed has **no** place (no city,
no flag, no hop) rather than GeoIP's.

**A BPB / Cloudflare Workers server has two real exits.** A Worker cannot connect to Cloudflare's
own addresses, so BPB sends requests for Cloudflare-hosted sites through its *proxy IP* (a relay
outside Cloudflare), and everything else straight from the Worker (a Cloudflare address). A
"what is my IP" page hosted on Cloudflare therefore shows the proxy IP, while the probe — which
deliberately asks endpoints *not* on Cloudflare — shows the Worker's egress. Both are true.

Once connected, `geo::exit_addresses` asks one IPv4-only and one IPv6-only endpoint through the
tunnel, concurrently, and the status card shows an **IPv4** and an **IPv6** chip for whichever
answered — a relay can leave from different places per family. It also reads Cloudflare's
`/cdn-cgi/trace` through the tunnel (`geo::CloudflareExit`), which is what Cloudflare-hosted sites
see; the card adds a **Cloudflare sites** chip only when that address differs from both, i.e. when
the server splits its traffic the way BPB does.

**Where a server is comes from its addresses, never its name.** `located()` in `store.ts` is the
one rule every view uses — the row's label, city and flag, search, the map dot, the status card and
the tray: the **exit** once a full test has placed it, else the **entry** (the config's address, a
domain resolved first — so a Cloudflare-fronted config shows as a Cloudflare node until tested),
else `Server.country`/`city` guessed from the name, which is only a placeholder for the seconds
before the address is looked up. **The row's title is always the config's own name** — location is
the flag and the subtitle. Titling rows by country made measured configs look renamed, and gave
every config exiting in one country the same title.
Editing a server's address or port clears both locations and re-checks it.
`replaceSubscriptionServers` preserves `entry`, `exit` and `usage` across a refresh.

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
2. **Directly, from the user's own connection**, turn each *distinct* address — entries and exits
   together — into a place —
   coordinates and city from `geo::whereabouts`, falling back to the country alone from the
   `COUNTRY_URLS` chain so a spent coordinate quota never costs the flag.

Doing it in one step is the obvious design and it fails. Servers share exits — four of ten in a
real subscription came back on one Cloudflare address — so asking a geo service through every
server puts the whole sweep on three or four client IPs and most requests come back `429`. That
version located 2 of 10 while all 8 reachable servers could reach the endpoint perfectly well.
Splitting it means the rate-limited half runs once per distinct exit, from an address the user
owns. The two-step version locates 8 of 10, which is every server that is up.

Both halves are endpoint *chains*, for the same reason: a free tier is exhaustible, and one day
of testing exhausted `ipinfo.io` for this machine entirely. Expect the resolvers to disagree
sometimes — a Cloudflare anycast address has no single physical location.

### The splash screen

At launch a cover ([views/splash.ts](src/views/splash.ts)) waits for the three things the window
needs to be worth looking at: the data file loaded, the core up, and this machine placed
(`locate_me`) — the answer, not the attempt, since on a filtered network the first tries fail and
`retryHome` keeps asking. It leaves when all three are done, or after `MAX_MS` (20 s), whichever
is first, and stays at least `MIN_MS` so the mark finishes being drawn. A machine that will not be
placed at all is not trapped: after `SKIP_AFTER_MS` (6 s) the splash offers a way past. A slow geo lookup must not hold the
app hostage, and a core that never comes up is the status card's to explain. A data file that
will not load takes it down at once (`leave`), because that error is the window's to show now.
The cover is in `index.html` from the first paint (a plain field in the page's colour, and a
drag region), so the empty window never shows before the script runs.

The animation is the mark being made, from the `mark` icon's own paths: the arch draws itself
(`pathLength` 1, a dash offset), the road runs out of the tunnel, rings recede into it, a light
passes over the finished mark, and leaving, it flies into the tunnel as it fades. The colours are
the artwork's gradient. Reduced motion shows the finished mark, still.

### Where the user is (the map's route)

The map starts from the user's own dot and, once connected, draws the route from it to the exit.
`locate_me` places this machine's public address and `locate_exit` places the tunnel's, both
through `geo::whereabouts`, which returns coordinates rather than a country (a country's centroid
puts everyone in Russia in Siberia) from a chain of endpoints, like the country lookup.

`locate_me` asks directly, so it is only ever asked when a direct request still answers for this
machine: with the tunnel down, or **connected in proxy mode**, where only what is pointed at the
listener goes through it and these requests are the Rust side's own (`canLocateHome`). In VPN mode
a direct request goes through the TUN and would put the user at their exit. It is asked at launch, after every
disconnect, and **when the network changes**: [netwatch.rs](src-tauri/src/netwatch.rs) looks
every 4 s at the address the system would send from to reach the internet (a UDP socket
"connected" to a public address, which sends nothing), and emits `network-changed` when it moves.
The webview's `online` event misses a move straight from one network to another. `locateHome`
runs one lookup at a time, and a request made during one runs once more after it.

**On a filtered network the services that place an address are the problem**, so three things
answer it. `geo::whereabouts(None)` asks **every endpoint at once** and takes the first answer:
asked in turn, two blocked endpoints cost ten seconds before a reachable one is tried, because a
blocked service hangs rather than refusing. (Placing a *server's* exit stays one at a time — dozens
of addresses, and free tiers to spare.) `api.ip.sb` is in the list for its Cloudflare front, which
such networks cannot block wholesale. And a failed lookup is tried again on a growing delay
(`HOME_RETRY_MS`, up to a minute), starting over on a network change or a disconnect, so a machine
that could not be placed at launch still gets placed later — including after a spell with no DNS at
all, which is what an Iranian network did once (every name failed in 92 ms, then resolved again).
`scripts/net-check.sh` tells a name that will not resolve from an address that will not answer; run
it on the failing network before building anything for it. `tunnelEpoch` in `main.ts` is
bumped on every connect and disconnect so an answer that was in flight across one is discarded.
`locate_exit` takes the same two steps as `geo::locate` in proxy mode — the address through the
listener, the place from the user's own connection — to keep the rate-limited half off a shared
exit.

`WorldMap.setPins` takes the route as a list of hops and draws one arc per leg. Today it is
`[home, exit]`; a proxy chain puts its servers in between and the map needs no change.

**The map is countries, not tiles.** Natural Earth borders from `world-atlas` (1:110m bundled,
1:50m loaded from the app's own assets past `DETAIL_ZOOM`), decoded from TopoJSON in `map.ts` into
one `Path2D` in degree space and drawn under the view transform — never fetched from a map server,
which would tell a third party every user's address whenever they looked at the map. Wheel/drag/
double-click are taken on the whole pane (pins sit above the canvas); rings crossing the antimeridian
are unwrapped and drawn twice, then clipped to the band. Route legs carry arrows (mid and end) so
the path reads you → hops → exit. While a route is drawn its nodes are bold (`Pin.hop` for the
ones between, the pulsing exit, the heavier ring of the user) and every other dot fades until
hovered. The user's own pin is an ink ring, never a brand-blue dot, and
opens a details card (public IP, ISP, city) from `home`.

Server dots are **selectable**. `groupPlaces` in `main.ts` puts every server exiting in the same
place (coordinates rounded to ~50 km, or the country for unmeasured ones) behind one dot; a dot
with one server selects it, a dot with several opens `MapPicker` ([views/mappick.ts](src/views/mappick.ts)):
the city, a Fastest button, and each server with its latency. `WorldMap` itself knows nothing
about servers — a pin carries a `key` and the map hands it back through `onPick`. Selecting from
the map and from the list go through the same `selectServer`, which reconnects if the tunnel is up.

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

A check **streams, end to end**: the `check_servers` command starts one scratch core for the run
(`geo::ProbeSession`, a local port per server) and tests six servers at a time. Each server gets
its latency, then a real request *through* it for the public address it comes out of, then that
address placed (`geo::PlaceCache`, one lookup per distinct address). The result is emitted as a
`server-checked` event the moment that server is done, so rows fill in one by one, each showing
"testing…" until its own result lands. **A server works only if traffic comes out of it**: a dial that succeeds
and then carries nothing is reported failed, and so is one whose traffic came out of the user's
own address. A geo service that could not place the exit does not fail the server. The list keeps
its scroll position and the search box its focus across every re-render (`LocationsPanel.render`).

**Lists run to tens of thousands.** Public subscriptions on GitHub are plain-text files of 20,000+
links (4.7 MB); one crashed the client by being treated like a provider's list of twenty. So:
the list renders `ROW_PAGE` (100) rows per group behind a "Show more" button, always including
the selected server; servers are auto-checked on add or update only up to `AUTO_CHECK_MAX` (100),
and a larger list is checked a row at a time; `checkServers` runs in `CHECK_BATCH` (50) batches,
each its own probe core; and `geo::ProbeSession::start` refuses more than `MAX_PROBE` servers so a caller that forgets to
batch fails with a reason instead of exhausting file descriptors.

A caller that passes its own `url` gets that one endpoint and no fallback — an explicit choice is
not second-guessed.

The list shows `—` for both "never tested" and "unreachable", separated only by colour, so
`Server.latencyError` carries the reason and the row states it on hover. Without it there is no way
to tell a dead server from one the test endpoint could not reach.

### Usage history

Every config records what the tunnel carried on it, by local day, in `Server.usage`
([usage.ts](src/usage.ts)). The source is the counters `query_stats` already returns for the status
card's rate — cumulative since the tunnel came up — so `sample()` in `main.ts` files the difference
between two readings (`advance`; a reading lower than the last means the core restarted, and the
new reading is all new) under `usageServerId`, the config the tunnel was started on. That is **not**
`selectedServerId`: choosing another server changes the selection before the reconnect, and the
tail of the old session belongs to the old config.

**The history lives on the config, so it lasts exactly as long as the config does.** Deleting a
config, or a refresh dropping it, takes its history with it; a refresh that keeps it keeps the
history, because the config keeps its id. That makes identity load-bearing: each config's id is a
random UUID (`newId`), and a refresh hands the id to the incoming config that *is* the old one via
`matchExisting` in [identity.ts](src/identity.ts) — protocol, host, port and credential (UUID,
Trojan password or WireGuard key), never the name, path or SNI case, which BPB randomises per
fetch — claiming each old config at most once. The earlier key was `server:port:uuid`, which gave
every Trojan and WireGuard config on one host and port the same id.

Counted traffic is written into the store every `USAGE_SAVE_MS` (15 s), not every poll — each write
re-renders the list and queues a save of the data file — and at once on disconnect (after one last
`sample()`) and when the core goes away. Quitting while connected can lose up to 15 s of count.

The usage sheet (`openUsage`, drawn by [views/usage.ts](src/views/usage.ts)) opens from a group
header's chart button or a row's **⋯ → Usage**: 30-day and all-time totals, the provider's
`subscription-userinfo` figure beside ours (theirs counts every device, ours only this app), a
stacked daily SVG chart, and for a group a per-config breakdown. It repaints on store changes while
open, so the numbers climb during a session. **Clear history** confirms in the footer by saying
what goes — the history, never the configs. No chart library: see *Frontend*.

`npm test` runs `src/**/*.test.ts` under Node's built-in runner, which strips types itself. Tested
modules must not import anything with a runtime value (type imports are erased), which is why
`usage.ts` and `identity.ts` stand apart from the store. Test files are excluded from `tsc`, which
would need `@types/node` to check them.

### Quick Connect

The **Quick Connect** button opens a prompt (`openQuickConnect` in `main.ts`, drawn by
[views/quickpick.ts](src/views/quickpick.ts)) where the user picks by criterion, chosen in
[quick.ts](src/quick.ts):

- **Fastest**: lowest latency among configs whose last test passed.
- **Most used**: most bytes in the last `RECENT_DAYS` (30), from the usage history.
- **Most recent**: the newest `Server.lastConnectedAt`, set in `connect()` only once the tunnel is
  really up. Not the selection: selecting a row is not connecting to it.

Each choice is answered on its own and names the config it would connect to (flag, name, group),
so two may name the same config. All three are always listed; one with no answer is disabled and
says what is missing, rather than disappearing. Retired configs are never offered. The button is
disabled only when none of the three has an answer. The tray's Connect still connects the
selected server.

Choosing **Fastest** when its result is older than `QUICK_STALE_MS` (10 min) re-tests the top
`QUICK_RETEST` (3) first, with the prompt still open and the option saying "Re-testing…"; the
fastest that answers wins, and if none does the option says so and nothing connects. Closing the
prompt meanwhile cancels. Most used and Most recent connect to exactly the config they name, and a
failed connect is reported like any other — nothing falls back to a config the user did not pick.
The prompt repaints on store changes while open, like the usage sheet.

`quick.ts` is generic over the item and imports nothing, so `npm test` covers it on plain objects;
the store turns servers into candidates (`quickCandidates`, `quickPicks`).

### Support (donations)

Donations have exactly **two channels, Buy Me a Coffee and crypto wallets**, written once in
[support.ts](src/support.ts) (`SUPPORT`) and repeated in the README's Support section and
`.github/FUNDING.yml` — keep all three in step. The panel says these are the only channels, so a
payment request "for Nunya" anywhere else can be recognised as not ours; do not add a third
channel without that sentence still being true.

The panel ([views/support.ts](src/views/support.ts), the rail's heart) shows each wallet's
address **whole** — a shortened address is what address-swapping scams count on nobody reading —
with Copy, a QR code, and the network it is on: "Send only USDT on TRON (TRC-20)". USDT on TRON and
on Ethereum look alike and are not interchangeable.

Buy Me a Coffee is a plain link, never its widget (the CSP forbids it, and it would tell a third
party whenever the panel opened). It opens in the **system browser** through `open_external` in
[external.rs](src-tauri/src/external.rs), which opens only `https` pages whose host is exactly one
of `ALLOWED_HOSTS` — the webview has no shell capability and gets no general opener. The frontend
mirrors the hosts in `SUPPORT_HOSTS`; a link the Rust side refuses is a button that does nothing.

`npm test` checks every shipped address against its network's format (`supportProblems`) — a typo
in a donation address sends money to nobody — and fails if the panel lists no channel at all.

**Until the channels are real they ship as placeholders, never as made-up addresses** (a string
that merely looks valid can belong to a stranger): the Buy Me a Coffee page with `live: false`, so
its button reads "Coming soon" and opens nothing, and wallets with `address: null`, listed by coin
and network with no address, Copy or QR (`payable`). The panel's closing line changes with
`acceptsDonations`. Going live is `live: true` and real addresses in `SUPPORT`, the README section,
and `.github/FUNDING.yml` (left out until the page exists, since it puts a Sponsor button on the
repository). Issue #23 stays open until then.

### Editing and deleting

Every modal goes through `openSheet` in [main.ts](src/main.ts), which owns the scrim, the
click-outside, Escape, listener cleanup and focusing the first field. Adding a sheet means writing
its contents, not its plumbing.

Editing a server is a **form over the profile**, in [views/editor.ts](src/views/editor.ts) — never
a share link in a text box. A link is a serialization: changing a port by finding it between an `@`
and a `?` makes the user the parser, and a typo there does not fail, it produces a different
server. The profile is already structured data on disk, so `ProfileEditor` edits that. The sheet
shows no share link: the core runs the JSON config built from the profile, and a link is generated
from it only when one is exported, so a live link beside the form would be a second view of the
same data to keep in step.

`ProfileEditor` keeps a deep-copied draft, so Cancel costs nothing, and **rebuilds** rather than
diffs when a structural choice changes — the protocol, the transport, the security layer — because
those decide which fields exist. Fields belonging to other protocols are kept, not cleared: both
the config builder and the link writer emit them only where they apply, so carrying an unused
`flow` is free, while clearing it would discard a setting for anyone who clicked through the
control to look. Its controls reuse the Advanced panel's classes (`set-group`, `srow`, `slab`,
`val`, `seg`) so the two read as one app.

`toShareLink` in [share.ts](src/share.ts) is the inverse of `parseShareLink`, for moving a server
to another client. Round-tripping is the contract:
`parseShareLink(toShareLink(p))` must equal `p`, which is why the WebSocket `?ed=N` parameter is
written back into the path it was lifted out of.

**WireGuard also shares as a wg-quick config** (`toWgQuick`), and its share sheet opens on it: the
official WireGuard apps scan only `[Interface]`/`[Peer]` text, never a `wireguard://` link. The
config carries `AllowedIPs = 0.0.0.0/0, ::/0` (what this client does with a tunnel), the name in a
`# Name =` comment wg-quick skips, and a `DNS` line from the app's DNS setting when that names an
address (`dnsAddressOf`; a host name gives none, and the sheet says so rather than picking a
resolver). **WARP is refused by name** (`wgQuickRefusal`): wg-quick has no place for `reserved`,
and the official apps would connect without it and carry nothing. The inverse, `parseWgQuick`,
lets such a config be pasted or scanned back — `parseShareLink` detects it — and refuses by name
what it cannot carry: a `PresharedKey`, AmneziaWG's `Jc`/`S1`/`H1`… keys, more than one peer. The
paste box lifts configs out whole (`extractWgQuick`) before reading the rest a word at a time.
Round-tripping holds here too: `parseWgQuick(toWgQuick(p))` equals `p`, guarded in `share.test.ts`.

The name is a separate field, and an explicit rename sets `Server.renamed`. The list, the tray and
Quick Connect show the config's name; the location is shown beside it, never instead of it.

Deletion always confirms, and the sheet states what is lost rather than asking "are you sure?":
whether the server returns on the next subscription update or is gone for good, that a subscription
URL is a credential with no other copy, and whether the tunnel is currently running on the target
(in which case it disconnects).

A row has one action button, **⋯**, which opens a menu: Check, Usage, Share, Edit, then Delete…
alone below a divider, in red. It used to be three buttons side by side, and Delete sat a misclick away
from Edit. The menu is fixed to the viewport (the list clips its overflow) and closes on scroll.

One CSS trap worth not repeating: the row action button is revealed with `opacity` on
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

`ENGINEERING_STANDARDS.md` holds the rules for structural change specifically: when a module is
large enough to split (and where the actual seams are, not just where a comment banner claims one
is), the `store.update()` state pattern, reusable-view conventions for the `dom.ts` approach, and
CSS token discipline. It also records, in writing, that this project considered and rejected
React/Redux and Tailwind — read it before proposing either again.

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

Not yet built: bypass rules in the UI, protocols beyond VLESS and VMess (Trojan, Shadowsocks,
Hysteria2, TUIC are separate outbound types, not more transports), and bundled fonts — the CSP
forbids remote font hosts, so Manrope must ship in the bundle.

### Subscriptions

A subscription URL is pasted into the same "Add servers" box as share links; `classify` in
[main.ts](src/main.ts) sorts each pasted line into a server, a subscription, or a rejection.
An `https://` line is taken as a subscription — which means a pasted `https://` *proxy* link is no
longer reachable, an acceptable trade in a TUN-only client that does not run HTTP proxies anyway.
So is a panel's **import link** — `sing-box://import-remote-profile?url=…` or
`clash://install-config?url=…` — which wraps the real address. It is stored whole as the group's
URL and unwrapped on every fetch by `subscription::resolve`, the one parser for it (the frontend
only recognises it, via `IMPORT_LINK`). The carried address may be percent-encoded, as sing-box's
scheme says, or raw, as BPB writes it; a raw one runs to the end of the link, because its own `&`s
belong to it. The fragment is always a display name, and is never sent.
An update fetches and parses the new list first, leaving the old servers usable meanwhile; if the
tunnel runs on one of the group's servers it is disconnected right before the swap, rather than the
old server being kept alive as "retired".
The group is created before the first fetch, so a dead endpoint shows up as a row carrying an error
rather than a dialog that hangs, and re-pasting a known URL refreshes it instead of duplicating it.

**Two kinds of body, told apart by content because no header distinguishes them.** The common one
is a list of share links, optionally base64-encoded. The other is a client's whole JSON
configuration — what a BPB panel serves when the link names an app — and `config_links` in
[subscription.rs](src-tauri/src/subscription.rs) tells the three apps apart by shape:

| Asked for | Served as | Servers are |
| --- | --- | --- |
| `?app=xray` | an array of Xray configs, one server each | the outbound tagged `proxy` (see `carrier`) |
| `?app=sing-box` | one sing-box config | every `outbounds`/`endpoints` entry that is not a group or local (`type`, not `protocol`) |
| `?app=clash` | one Clash config, **as JSON** | every entry of `proxies`; `proxy-groups` is ignored |

Each reader fills a `Node` (or a `WireGuard`) and **one writer** turns it into a share link, so the
three formats of one subscription produce the very same links —
`every_format_of_one_subscription_imports_the_same_servers` guards that, and it held against a real
BPB panel. sing-box and Clash keep WebSocket early data in two fields beside the path; the writer
puts it back into the path as `?ed=N` (plus `eh=` for a non-standard header), which is how links
carry it. A chain is `detour` in sing-box and `dialer-proxy` in Clash, and is refused like Xray's.

Clash is usually **YAML**, which is not read: that would mean carrying a parser for an untrusted
body in a format the panels this client is built around serve as JSON anyway. A YAML body is
refused by name (`is_clash_yaml`) before the line scan, which would otherwise report its DNS
servers and health-check URLs as servers.

Three rules are load-bearing:

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
  encoded properly by the `WireGuard` writer; the marker path is for whatever turns up next.)
  Scheme names follow the link, not the config: sing-box's `shadowsocks` is written `ss`, which is
  the name the frontend can refuse by.

Query values must be percent-encoded on the way out: a WebSocket path is routinely
`/vl/abc?ed=2560`, whose `?` would otherwise terminate the query it is written into.
