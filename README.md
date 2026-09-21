# Nunya

A TUN-only desktop VPN client for [nunya-core](https://github.com/nunyavpn/nunya-core), built as a
Tauri v2 app.

## What this is, and what it deliberately is not

Nunya supports one transport — TUN — and a small set of protocols. That constraint is the point:
with no proxy mode there is no partial coverage, so "the device is in the tunnel" is a claim the UI
can make honestly.

It does not implement OTP, global hotkeys, speed tests, WARP registration, the dashboard installer,
or diagnostics capture.

Proxy mode has since been added alongside TUN — a local SOCKS/HTTP listener, optionally set as the
desktop's system proxy while connected — for machines where a TUN cannot be had. It gives up the
whole-device claim, and the UI says so rather than making it.

Both this repository and the core are forks of [Throne](https://github.com/throneproj/Throne),
which is GPL-3.0; so is this.

## The two repositories

| | |
| --- | --- |
| [nunyavpn/nunya](https://github.com/nunyavpn/nunya) | this one: the UI, config generation, share links, the transports |
| [nunyavpn/nunya-core](https://github.com/nunyavpn/nunya-core) | the sing-box / Xray engine and its RPC surface |

This repository does **not** build the core. It pins a core release in [`core.lock`](core.lock) and
`scripts/fetch-core.sh` downloads that release's assets, verifying each one against a `SHA256SUMS`
whose own digest is pinned in the lockfile. So a bad or re-cut release fails the check instead of
being installed, and the RPC contract between the two repos is a published, versioned interface
rather than a relative path into a sibling checkout.

## Layout

```text
.
├── core.lock             the nunya-core release this client builds against
├── src/                  frontend (TypeScript, no framework)
├── design/               the style guide, rendered from src/ (dev only, never bundled)
├── src-tauri/
│   ├── src/
│   │   ├── main.rs           Tauri commands and startup sequence
│   │   ├── config.rs         builds the sing-box config
│   │   ├── core_proc.rs      spawns and supervises the core
│   │   ├── transport/        the TunnelTransport seam
│   │   └── rpc/              IPC client: codec, link, peer verification
│   └── build.rs          generates prost types from vendor/core/proto/nunya.proto
├── NunyaTunnel/          the macOS packet tunnel extension (Swift)
├── project.yml           XcodeGen spec for that extension
├── docker/               a Linux box for exercising the tunnel in its own netns
├── scripts/
│   ├── fetch-core.sh         install the pinned core (or build one from source)
│   ├── build-app.sh          the full release build
│   ├── build-extension.sh    the .appex, embedded into the bundle
│   ├── dev-linux.sh          core + tunnel tests in a container, no root on the host
│   └── dev-tunnel.sh         a tunnel that comes up without an Apple account
└── vendor/core/          installed by fetch-core.sh, gitignored
```

## Prerequisites

| Tool | Why |
| --- | --- |
| Rust (rustup) | the Tauri shell |
| `protoc` | the Rust bindings generate from the core's proto |
| Node 20+ | the frontend |
| Go 1.26+ | only to build a core from source (`--source`); not needed to use a release |
| XcodeGen + full Xcode | only for the macOS packet tunnel extension |

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
brew install protobuf xcodegen
```

## Building

```bash
npm install
./scripts/fetch-core.sh     # installs the pinned core into vendor/core/
npm run tauri dev
```

`fetch-core.sh` also stages the core as a Tauri sidecar, which is what lets `cargo check` and
`tauri dev` run at all — Tauri refuses to build while the binary named in `externalBin` is missing.

For a release bundle with the packet tunnel extension:

```bash
./scripts/build-app.sh
```

### Filling the UI with test data

Every screen is a list of things a provider gave you, so an empty store shows almost nothing. A
fixture in `src/mock.ts` stands in:

```bash
VITE_MOCK=1 npm run tauri dev   # the app, with the list full
VITE_MOCK=1 npm run dev         # the same UI in a browser, no Rust side at all
```

It covers the states that are otherwise awkward to reach by clicking: every latency grade, an
unreachable server, an untested one, a subscription whose last refresh failed, a quota past the
amber threshold, a collapsed group, a retired server — one a refresh dropped while the tunnel
was running on it — and weeks of usage history on a few configs, so the usage charts have
something to draw.

Two things keep it out of the way of real data. Writes are discarded, so clicking through the
fixture cannot overwrite the data file that holds your actual credentials; and `VITE_MOCK` is
substituted at build time, so an ordinary `npm run build` drops the module entirely rather than
shipping a list of plausible-looking servers. Every host in it is under `example.net`, which RFC
2606 reserves so it can never be registered.

### Tests

```bash
cargo test --manifest-path src-tauri/Cargo.toml   # the Rust side
npm test                                          # frontend logic, on Node's built-in runner
```

`npm test` needs no dependency: Node runs the TypeScript directly. It covers the logic that decides
whose traffic is whose — usage arithmetic, and which config a refreshed subscription entry is.

### The style guide

```bash
npm run design
```

A page that renders the design system out of the app rather than describing it: the tokens are read
from `src/styles.css` through the browser's own CSSOM, the icons are enumerated from
`views/icons.ts`, and the status card is the real component mounted with a plain model. Both themes
are shown side by side, because the app follows the system setting and has no switch of its own.

It is dev-only — `vite build` takes just the root `index.html`, so nothing under `design/` is
bundled. See [design/README.md](design/README.md).

### Testing the tunnel without root

```bash
brew install colima docker && colima start --cpu 4 --memory 8   # once
./scripts/dev-linux.sh
```

Bringing a TUN up on your own machine means `sudo`, and it means rewriting the routing table you are
currently using in order to assert something about routing. A container avoids both: the tunnel gets
its own network namespace, so the table it takes over is the container's and it disappears when the
container does. The privilege budget is `CAP_NET_ADMIN` plus `/dev/net/tun` — not `--privileged`,
not root on the host, and nothing that outlives the process.

The boundary is deliberately placed so that **the core stays a child process of the Rust side**,
both inside the container. That keeps the unix socket local and means `rpc/peer.rs` verification is
exercised unchanged on every run. Splitting GUI-here/core-there would have broken it: peer identity
is a pid over a unix socket, and nothing survives being forwarded across a VM boundary.

`src-tauri/tests/tunnel_linux.rs` holds what this makes possible — that the tunnel takes the default
route *and gives it back*, that a private range is not swallowed, that stopping a tunnel that never
started leaves nothing behind. None of those can run on a developer's macOS machine without root.

This does not run the UI. Tauri on Linux renders through WebKitGTK, and forwarding that to macOS
over X11 is a worse loop than the browser one already available — work on the interface with
`VITE_MOCK=1 npm run tauri dev` on the host.

### Working on the core at the same time

```bash
./scripts/fetch-core.sh --source ../nunya-core
```

This builds the core from a local checkout instead of downloading it, and needs Go. The result is a
**development** core: its parent-process check is compiled out, because the parent of a dev build is
`cargo` rather than `Nunya`. `build-app.sh` refuses to put one in a bundle, and no such build is
ever published — a downloadable core with its parent check off is a root-capable binary anything on
the machine could drive.

### Moving to a new core release

```bash
./scripts/fetch-core.sh --update v0.2.0    # rewrites core.lock; commit it
```

## How the client talks to the core

Despite the `service NunyaCoreService` block in the core's `proto/nunya.proto`, **nothing on the
wire is gRPC**. `rpc.Serve` in the core's `internal/rpc/dispatch.go` reads two little-endian frames
over a unix socket and dispatches through a `map[string]handlerFn`:

```text
request   [u32 id][u16 method_len][method][u32 payload_len][protobuf]
response  [u32 id][u8  status    ][u32 payload_len][protobuf or error text]
```

The proto file exists to generate message types for both sides. `src-tauri/src/rpc/codec.rs`
implements the framing and is unit-tested against the layout above.

### The link is backwards from what you would expect

The GUI **listens** and the core **dials in**. `RunCore` in the core's `main.go` reads
`NUNYA_CORE_SOCKET` and connects. So startup order is bind → spawn → accept, and the core is a
child process of the app.

### Both ends verify each other

The core refuses to talk to a listener whose owning process is not its own parent, by reading
`LOCAL_PEERPID` off the socket (the core's `internal/ipc/ipc_unix.go`). It also refuses to run at all
unless its parent executable is named `Nunya` and sits in the same directory
(the core's `internal/parentcheck`).

Nothing checked the other direction, so this client adds the symmetric check in
`src-tauri/src/rpc/peer.rs`: on accept, the peer's uid must be ours or root, and its pid must be the
core we spawned. Without it, any local process that won the race to the socket could impersonate the
core and report a healthy tunnel that did not exist.

The socket is created inside a `0700` directory and set to `0600`.

### Development builds disable the parent check

A core built with `fetch-core.sh --source` carries the `noparentcheck` tag, because during
development the parent is `cargo` rather than `Nunya`. Every published core enforces the check.

## Privilege: the macOS tunnel is a real system VPN

Nunya uses Apple's NetworkExtension framework, the same mechanism WireGuard, Mullvad, NordVPN and
Tailscale use. The system creates the utun and launches a bundled packet tunnel extension; nothing
runs as root, nothing is setuid, and the VPN appears in System Settings > VPN beside every other
VPN. The user approves it once, through the standard system prompt.

```text
Nunya.app/Contents/
├── MacOS/Nunya                     UI only, unprivileged
└── PlugIns/NunyaTunnel.appex/      system-launched, holds the utun
    └── NunyaCore.xcframework       the core's mobile package, linked in
```

This replaces the Qt client's approach, which makes the core setuid-root by way of an `osascript`
call that opens Terminal.app (`src/sys/macos/MacOS.cpp`). A setuid-root binary any local process can
exec is a privilege-escalation surface, and it is why that build's README carries an apology about
quarantine attributes.

### How the core takes a TUN it did not create

`PlatformInterface.OpenTun` in the core's `mobile/platform.go` returns a *file descriptor*, and
its `mobile/service.go` dups it into `options.FileDescriptor`. That is exactly the
NEPacketTunnelProvider contract, and it is already how the Android build works. The Darwin half of
it lives in the core's `mobile/sys_darwin.go`.

Finding the descriptor is indirect: `NEPacketTunnelFlow` exposes no fd, so the provider scans its
own open descriptors for the one that is a `utunN` kernel-control socket — the approach WireGuard's
Apple clients use. The core independently validates what it is handed, by checking the socket is a
kernel control and the name really begins with `utun`. That check matters: `getsockopt` on an
unrelated descriptor can *succeed* and return garbage (an `AF_UNIX` socket yields `"Sc"`), which
without validation would be passed to sing-box as a real interface name.

### Building the Apple side

```bash
./scripts/fetch-core.sh                # installs NunyaCore.xcframework into vendor/core/apple/
npm run tauri build -- --bundles app   # the app bundle
./scripts/build-extension.sh           # the .appex, embedded into the bundle
```

`./scripts/build-app.sh` runs all three in order.

`build-extension.sh` generates the Xcode project from `project.yml` (via XcodeGen — do not edit the
`.xcodeproj`, it is generated and gitignored), builds the extension, and copies it into
`Contents/PlugIns`.

Two things that are easy to lose an afternoon to:

- **gomobile generates a class *and* a protocol for every Go interface.** Swift resolves the bare
  name to the class, so the protocol needs the `Protocol` suffix: `MobilePlatformInterfaceProtocol`,
  `MobileTunOptionsProtocol`, and so on. Swift also renames some members on import
  (`usePlatformAutoDetectInterfaceControl` becomes `usePlatformAutoDetectControl`).
- **Go's `net` package resolves through cgo on Darwin**, which needs `res_9_ninit`, `res_9_nsearch`
  and `res_9_nclose` from libresolv. `project.yml` adds `-lresolv`; without it the link fails with
  three undefined symbols and no hint as to why.

### What is still required to actually run it

Signing. `NUNYA_TEAM_ID=<your team> ./scripts/build-extension.sh` signs the extension; the team
must have the NetworkExtension capability enabled for both bundle identifiers. That capability comes
only with a paid Apple Developer Program membership — a Personal Team cannot sign it, and an
unsigned Network Extension will not load, so there is no way to test the tunnel first and sign it
later.

Unsigned, everything still compiles, and `NetworkExtensionTransport` reports the system's own error
rather than pretending.

## Developing without an Apple Developer account

You do not need one to work on this. `TunnelTransport` has two implementations and the choice is an
environment variable:

```bash
./scripts/dev-linux.sh                        # preferred: a real tunnel, no root on your machine
./scripts/dev-tunnel.sh                       # last resort: runs the whole app under sudo
NUNYA_TRANSPORT=networkextension npm run tauri dev   # once the extension can be signed
```

Reach for `dev-linux.sh` first. `dev-tunnel.sh` elevates the *entire app* — WebKit webview,
frontend and all — when only the core needs to create a utun, and because it uses `sudo -E` it
inherits your `HOME`: the moment that root instance saves, `data.json` becomes root-owned and your
normal unprivileged instance silently fails every write afterwards.

`dev-tunnel.sh` builds a development core from `../nunya-core` (override with `NUNYA_CORE_SRC`),
builds everything unprivileged and then runs the app under `sudo` for that one
session, because creating a utun needs root and the system is not doing it for us yet. Nothing
privileged is left behind: no setuid bit, no installed daemon, nothing that outlives the process.

That last point is the reason it uses `sudo` rather than `chmod u+s` on the core, which is what the
Qt client does. A setuid-root binary stays root-owned and executable by any local process for as
long as it is on disk; a `sudo` run ends when you quit.

The transport default is deliberately `subprocess` — see `transport/select.rs`. Defaulting to
NetworkExtension before it can be signed would leave anyone without a membership looking at a
tunnel that silently never starts.

When the membership arrives: sign the extension, flip `NUNYA_TRANSPORT`, and nothing above the
trait changes.

## Replacing the core

The core is reached through exactly two seams, both narrow on purpose:

- `rpc/` speaks the framing above, generated from the pinned `proto/nunya.proto`. Any core that implements `Start`, `Stop`, `CheckConfig` and
  `QueryStats` over that protocol is a drop-in.
- `config.rs` generates sing-box JSON, which is the one place that assumes *which* core is running.

Writing a different core means reimplementing the protocol, or replacing `SubprocessTransport` with
one that speaks something else entirely. Nothing in the UI, the share-link parser or the bypass
rules depends on the core being this one.

## The transport seam

`src-tauri/src/transport/` holds one trait, `TunnelTransport`, with a platform implementation
behind it. Everything above it — config generation, share-link parsing, the UI — is shared.

| Platform | Mechanism | Core runs as | Privilege |
| --- | --- | --- | --- |
| macOS | `NEPacketTunnelProvider` in a bundled `.appex` | a linked library | none; the system owns the utun |
| Windows | a service with WinTun | a subprocess | the service |
| Linux | systemd unit or `CAP_NET_ADMIN` | a subprocess | capability on the core |

The subprocess transport is implemented and tested; the NetworkExtension one is a skeleton.

## Status

Working and tested: the toolchain, the split core build and its checksum-pinned install, the IPC codec with peer verification, config
generation, config validation against a real core, connect/disconnect, throughput polling, per-config
usage history with daily charts, VLESS and
VMess, Trojan and WireGuard parsing over every transport the core implements, subscriptions in
both formats below, the transport seam, and the core's Darwin TUN-descriptor support.

Written but not yet buildable: the Swift packet tunnel provider and the `.xcframework` build, both
blocked on Xcode and an Apple Developer account.

Not yet built: the Xcode target that produces and embeds the `.appex`, `NETunnelProviderManager`
wiring, bypass rules in the UI, the menu-bar popover,
protocols beyond the four above, multi-hop chains, and bundled fonts (the CSP forbids remote font hosts, so Manrope
has to ship with the bundle).

### Subscriptions

A subscription URL goes into the same box as share links; an `https://` line is taken as a
subscription and gets its own group, which is then refreshable. So is a panel's import link —
`sing-box://import-remote-profile?url=…` or `clash://install-config?url=…` — which wraps the real
address and is unwrapped each time it is fetched. Several body formats are in circulation and
nothing in the headers tells them apart, so they are distinguished by content:

| | |
| --- | --- |
| A list of share links | one per line, plain or base64-encoded. The common case. |
| A JSON Xray configuration, or an array of them | what a BPB panel serves to `?app=xray`: each entry is a whole client config — inbounds, routing, DNS — wrapped around a single `proxy` outbound. |
| A sing-box configuration | what BPB serves to `?app=sing-box`: every server is an outbound (or, for WireGuard, an endpoint), beside groups that are skipped. |
| A Clash configuration in JSON | what BPB serves to `?app=clash`: every server is in `proxies`. Clash YAML is refused by name. |

The configurations are rewritten back into share links in `subscription.rs`, all through one
writer, so the three forms of one subscription import identically, everything downstream is
unchanged, and rejections are still reported entry by entry.

Two decisions inside that are easy to get wrong. A configuration with no outbound tagged `proxy`
is a load balancer — BPB's "Best Ping" carries `proxy-1` through `proxy-8` behind a `leastPing`
selector — and is skipped rather than flattened, because the servers behind it are already listed
individually in the same array and flattening imports each of them twice. And protocols this build
cannot run are emitted anyway, under their own scheme, so a Trojan entry comes back named rather
than missing: a subscription that quietly returned four of its eight servers, with nothing to say
why, would be worse than one that explains itself.

That second rule covers protocols whose shape is unfamiliar too, not just unfamiliar names. A WARP
subscription is entirely WireGuard, which keeps its endpoint under `peers` where every other
protocol uses `servers` — so every entry falls through the address lookup. Emitting a marker link
under the protocol's own scheme is what turns "the subscription returned nothing that looks like a
server list" into "WireGuard is not supported yet".

### Protocols and transports

| | |
| --- | --- |
| Protocols | VLESS, VMess (both share-link forms, including the base64 blob), Trojan, WireGuard |
| Transports | TCP, WebSocket, gRPC, HTTP/2, HTTPUpgrade, QUIC |
| Security | none, TLS, Reality, with uTLS fingerprints and ALPN |
| Rejected by name | mKCP, XHTTP, SplitHTTP, meek — the core has no implementation, and silently downgrading one to TCP would connect to the wrong thing |
| Also rejected by name | multi-hop chains. One outbound dialling through another is a topology this client has no way to express, and reducing one to its last hop would connect somewhere the entry's own name contradicts |
| Not yet | Shadowsocks, Hysteria2, TUIC — separate outbound types rather than another transport |

WireGuard is the one that is not an outbound at all. sing-box moved it to `endpoints`, because it
is an interface with its own addresses rather than a dialer, and the core rejects the old outbound
form outright (*unknown field "local_address"*). `config::proxy_node` is where that fork lives, so
the route table names a tag and does not care which of the two produced it. Cloudflare WARP works
through this path, `reserved` client id included — a wrong `reserved` is dropped by the server
without an error, so it is carried rather than treated as optional decoration.

Each transport is validated against a real core in
`src-tauri/tests/core_link.rs::every_transport_is_accepted_by_the_core`, because the shape this
client emits and the shape sing-box accepts differ in ways unit tests here cannot see — `host` is a
string for HTTPUpgrade and a list for HTTP/2, and plain TCP means *no* `transport` key rather than
an empty one.
