# Contributing to Nunya

Thank you for helping. This is the developer's guide: how to build Nunya from source, how to work on
it day to day, how the pieces fit together, and how a change gets from an idea to a release.
For what the app does and how to use it, see the [README](README.md).

- [How work is done](#how-work-is-done)
- [Building from source](#building-from-source)
- [Day-to-day development](#day-to-day-development)
- [Tests](#tests)
- [How the pieces fit](#how-the-pieces-fit)
- [How the client talks to the core](#how-the-client-talks-to-the-core)
- [Privilege: the macOS tunnel is a real system VPN](#privilege-the-macos-tunnel-is-a-real-system-vpn)
- [Developing without an Apple Developer account](#developing-without-an-apple-developer-account)
- [Subscriptions, protocols and transports](#subscriptions-protocols-and-transports)
- [Releasing](#releasing)
- [Conventions](#conventions)

## How work is done

Every change follows the same path, so it can be traced from an issue to a single commit on `main`:

1. **An issue** describes the work: what exists, what to do, what is still to be settled, and where
   in the code it lives.
2. **A branch linked to the issue**, named in gitflow style: `feature/…`, `bugfix/…` or `hotfix/…`,
   created from `main` (GitHub's *Create a branch* on the issue, or
   `gh issue develop <n> --name feature/<name> --base main --checkout`).
3. **A pull request** into `main` whose description says `Closes #<n>`, with what changed, why, and
   how it was tested.
4. **A squash merge**, so `main` has one commit per change.

`main` is the only long-lived branch. Pushing a `vX.Y.Z` tag on it publishes a release (see
[Releasing](#releasing)).

## Building from source

### The two repositories

| | |
| --- | --- |
| [nunyavpn/nunya](https://github.com/nunyavpn/nunya) | this one: the UI, config generation, share links, the transports |
| [nunyavpn/nunya-core](https://github.com/nunyavpn/nunya-core) | the sing-box / Xray engine and its RPC surface |

This repository does **not** build the core. It pins a core release in [`core.lock`](core.lock), and
`scripts/fetch-core.sh` downloads that release's assets, verifying each one against a `SHA256SUMS`
whose own digest is pinned in the lockfile. A bad or re-cut release fails the check instead of
being installed, and the RPC contract between the two repositories is a published, versioned
interface rather than a relative path into a sibling checkout.

### Prerequisites

| Tool | Why |
| --- | --- |
| Rust (rustup) | the Tauri shell |
| `protoc` | the Rust bindings are generated from the core's proto |
| Node 24 | the frontend; `npm test` runs TypeScript directly, which needs Node's type stripping |
| Go 1.26+ | only to build a core from source (`--source`); not needed to use a release |
| XcodeGen and full Xcode | only for the macOS packet tunnel extension |

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
brew install protobuf xcodegen
```

On Linux, Tauri also needs WebKitGTK and friends:

```bash
sudo apt-get install -y libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
  libxdo-dev libssl-dev protobuf-compiler
```

### Build and run

```bash
npm install
./scripts/fetch-core.sh     # installs the pinned core into vendor/core/
npm run tauri dev
```

`fetch-core.sh` also stages the core as a Tauri sidecar, which is what lets `cargo check` and
`tauri dev` run at all: Tauri refuses to build while the binary named in `externalBin` is missing.

A **release** core refuses to run unless its parent is a binary named exactly `Nunya` in the same
directory, and `tauri dev`'s parent is `target/debug/nunya`. To run the app in dev mode, build a
development core from a local checkout of nunya-core (see
[Working on the core at the same time](#working-on-the-core-at-the-same-time)).

### Bundles

As the CI release builds them, without the packet tunnel extension:

```bash
npm run tauri build -- --bundles app,dmg        # macOS
npm run tauri build -- --bundles deb,appimage   # Linux
```

With the packet tunnel extension, which needs an Apple Developer team:

```bash
./scripts/build-app.sh
```

### Layout

```text
.
├── core.lock             the nunya-core release this client builds against
├── src/                  frontend (TypeScript, no framework)
├── design/               the style guide, rendered from src/ (dev only, never bundled)
├── docs/                 the README's logo and screenshots
├── src-tauri/
│   ├── src/
│   │   ├── lib.rs            Tauri commands and startup sequence (main.rs is a shim)
│   │   ├── config.rs         builds the sing-box config
│   │   ├── core_proc.rs      spawns and supervises the core
│   │   ├── subscription.rs   fetches subscriptions and reads their formats
│   │   ├── sysproxy.rs       sets the desktop's system proxy, and puts it back
│   │   ├── transport/        the TunnelTransport seam
│   │   └── rpc/              IPC client: codec, link, peer verification
│   └── build.rs          generates prost types from vendor/core/proto/nunya.proto
├── NunyaTunnel/          the macOS packet tunnel extension (Swift)
├── project.yml           XcodeGen spec for that extension
├── docker/               a Linux box for exercising the tunnel in its own netns
├── scripts/
│   ├── fetch-core.sh         install the pinned core (or build one from source)
│   ├── build-app.sh          the signed macOS build, with the packet tunnel extension
│   ├── build-extension.sh    the .appex, embedded into the bundle
│   ├── dev-linux.sh          core + tunnel tests in a container, no root on the host
│   └── dev-tunnel.sh         a tunnel that comes up without an Apple account
└── vendor/core/          installed by fetch-core.sh, gitignored
```

## Day-to-day development

### Working on the interface with mock data

Every screen is a list of things a provider gave you, so an empty store shows almost nothing. A
fixture in `src/mock.ts` stands in:

```bash
VITE_MOCK=1 npm run dev         # the UI in a browser, with a simulated core
VITE_MOCK=1 npm run tauri dev   # the app, with the list full
```

It covers the states that are otherwise awkward to reach by clicking: every latency grade, an
unreachable server, an untested one, a subscription whose last refresh failed, a quota past the
amber threshold, a collapsed group, a retired server (one a refresh dropped while the tunnel was
running on it), and weeks of usage history on a few configs.

In the browser, `src/mockcore.ts` answers what the Rust side would, so the whole flow can be worked
on without a Tauri window: connecting and disconnecting, live traffic, the exit on the map, server
checks and Quick Connect. The README's screenshots are taken this way.

Two things keep all of this away from real data. Writes are discarded, so clicking through the
fixture cannot overwrite the data file that holds your actual credentials. And `VITE_MOCK` is
substituted at build time, so an ordinary `npm run build` drops both modules entirely rather than
shipping a list of plausible-looking servers. Every host in the fixture is under `example.net`,
which RFC 2606 reserves so it can never be registered.

### The style guide

```bash
npm run design
```

A page that renders the design system out of the app rather than describing it: the tokens are
read from `src/styles.css` through the browser's own CSSOM, the icons are enumerated from
`views/icons.ts`, and the status card is the real component mounted with a plain model. Both
themes are shown side by side, because the app follows the system setting. It is dev-only, and
nothing under `design/` is bundled. See [design/README.md](design/README.md).

### Working on the core at the same time

```bash
./scripts/fetch-core.sh --source ../nunya-core
```

This builds the core from a local checkout instead of downloading it, and needs Go with
`protoc-gen-go` and `protoc-gen-go-grpc` on your `PATH` (`$(go env GOPATH)/bin`). The result is a
**development** core: its parent-process check is compiled out, because the parent of a dev build
is `cargo` rather than `Nunya`. `build-app.sh` refuses to put one in a bundle, and no such build is
ever published: a downloadable core with its parent check off is a root-capable binary anything on
the machine could drive.

### Moving to a new core release

```bash
./scripts/fetch-core.sh --update v0.2.0    # rewrites core.lock; commit it
```

## Tests

```bash
cargo test --manifest-path src-tauri/Cargo.toml   # the Rust side
npm test                                          # frontend logic, on Node's built-in runner
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
```

`npm test` needs no dependency: Node runs the TypeScript directly. Tests that need a live core or a
real TUN are `#[ignore]`d, so the default run is the pure ones. CI runs all of the above on macOS
and Linux for every pull request.

### Testing the tunnel without root

```bash
brew install colima docker && colima start --cpu 4 --memory 8   # once
./scripts/dev-linux.sh
```

Bringing a TUN up on your own machine means `sudo`, and it means rewriting the routing table you
are currently using in order to assert something about routing. A container avoids both: the tunnel
gets its own network namespace, so the table it takes over is the container's, and it disappears
when the container does. The privilege budget is `CAP_NET_ADMIN` plus `/dev/net/tun`: not
`--privileged`, not root on the host, and nothing that outlives the process.

The boundary is deliberately placed so that **the core stays a child process of the Rust side**,
both inside the container. That keeps the unix socket local and means `rpc/peer.rs` verification is
exercised unchanged on every run. Splitting GUI-here, core-there would break it: peer identity is a
pid over a unix socket, and nothing survives being forwarded across a VM boundary.

`src-tauri/tests/tunnel_linux.rs` holds what this makes possible: that the tunnel takes the default
route *and gives it back*, that a private range is not swallowed, and that stopping a tunnel that
never started leaves nothing behind. This does not run the UI; work on the interface with
`VITE_MOCK=1`.

## How the pieces fit

### The transport seam

`src-tauri/src/transport/` holds one trait, `TunnelTransport`, with a platform implementation behind
it. Everything above it (config generation, share-link parsing, the UI) is shared.

| Platform | Mechanism | Core runs as | Privilege |
| --- | --- | --- | --- |
| macOS | `NEPacketTunnelProvider` in a bundled `.appex` | a linked library | none; the system owns the utun |
| Windows | a service with WinTun | a subprocess | the service |
| Linux | systemd unit or `CAP_NET_ADMIN` | a subprocess | capability on the core |

The subprocess transport is implemented and tested; the NetworkExtension one is a skeleton, and
Windows is not built yet ([#34](https://github.com/nunyavpn/nunya/issues/34)).

### Replacing the core

The core is reached through exactly two seams, both narrow on purpose:

- `rpc/` speaks the framing below, generated from the pinned `proto/nunya.proto`. Any core that
  implements `Start`, `Stop`, `CheckConfig` and `QueryStats` over that protocol is a drop-in.
- `config.rs` generates sing-box JSON, which is the one place that assumes *which* core is running.

Nothing in the UI, the share-link parser or the bypass rules depends on the core being this one.

## How the client talks to the core

Despite the `service NunyaCoreService` block in the core's `proto/nunya.proto`, **nothing on the
wire is gRPC**. The core reads two little-endian frames over a unix socket and dispatches through a
map of handlers:

```text
request   [u32 id][u16 method_len][method][u32 payload_len][protobuf]
response  [u32 id][u8  status    ][u32 payload_len][protobuf or error text]
```

The proto exists to generate message types for both sides. `src-tauri/src/rpc/codec.rs` implements
the framing and is unit-tested against the layout above.

### The link is backwards from what you would expect

The GUI **listens** and the core **dials in**, reading `NUNYA_CORE_SOCKET`. So the startup order
is bind, spawn, accept, and the core is a child process of the app.

### Both ends verify each other

The core refuses to talk to a listener whose owning process is not its own parent, by reading
`LOCAL_PEERPID` off the socket. It also refuses to run at all unless its parent executable is named
`Nunya` (`Nunya.exe` on Windows) and sits in the same directory. That is why the app's
`mainBinaryName` is `Nunya`: the bundles put the core beside it.

This client adds the symmetric check in `src-tauri/src/rpc/peer.rs`: on accept, the peer's uid must
be ours or root, and its pid must be the core we spawned. Without it, any local process that won the
race to the socket could impersonate the core and report a healthy tunnel that did not exist. The
socket is created inside a `0700` directory and set to `0600`.

A core built with `fetch-core.sh --source` carries the `noparentcheck` build tag, because during
development the parent is `cargo`. Every published core enforces the check.

## Privilege: the macOS tunnel is a real system VPN

VPN mode on macOS uses Apple's NetworkExtension framework, the same mechanism WireGuard, Mullvad,
NordVPN and Tailscale use. The system creates the utun and launches a bundled packet tunnel
extension. Nothing runs as root, nothing is setuid, and the VPN appears in System Settings → VPN
beside every other VPN; the user approves it once, through the standard system prompt.

```text
Nunya.app/Contents/
├── MacOS/Nunya                     UI only, unprivileged
└── PlugIns/NunyaTunnel.appex/      system-launched, holds the utun
    └── NunyaCore.xcframework       the core's mobile package, linked in
```

### How the core takes a TUN it did not create

`PlatformInterface.OpenTun` in the core's `mobile/platform.go` returns a *file descriptor*, which
its `mobile/service.go` dups into `options.FileDescriptor`. That is exactly the
NEPacketTunnelProvider contract, and it is already how the Android build works.

Finding the descriptor is indirect: `NEPacketTunnelFlow` exposes no fd, so the provider scans its own
open descriptors for the one that is a `utunN` kernel-control socket, the approach WireGuard's Apple
clients use. The core independently validates what it is handed, checking that the socket is a
kernel control and the name really begins with `utun`. That check matters: `getsockopt` on an
unrelated descriptor can *succeed* and return garbage (an `AF_UNIX` socket yields `"Sc"`), which
without validation would be passed to sing-box as a real interface name.

### Building the Apple side

```bash
./scripts/fetch-core.sh                # installs NunyaCore.xcframework into vendor/core/apple/
npm run tauri build -- --bundles app --config src-tauri/tauri.networkextension.conf.json
./scripts/build-extension.sh           # the .appex, embedded into the bundle
```

`./scripts/build-app.sh` runs all three in order.

The NetworkExtension entitlements (`Nunya.entitlements`) are merged in only here, from
`src-tauri/tauri.networkextension.conf.json`. They need an Apple Developer team's signature, and
macOS kills an app that claims them without one, so the default build (and the CI release) is
ad-hoc signed without them and runs its core as a child process. The xcframework is fetched only
when the pinned core release publishes one.

`build-extension.sh` generates the Xcode project from `project.yml` via XcodeGen (do not edit the
`.xcodeproj`: it is generated and gitignored), builds the extension, and copies it into
`Contents/PlugIns`. Two things that are easy to lose an afternoon to:

- **gomobile generates a class *and* a protocol for every Go interface.** Swift resolves the bare
  name to the class, so the protocol needs the `Protocol` suffix: `MobilePlatformInterfaceProtocol`,
  `MobileTunOptionsProtocol`, and so on. Swift also renames some members on import
  (`usePlatformAutoDetectInterfaceControl` becomes `usePlatformAutoDetectControl`).
- **Go's `net` package resolves through cgo on Darwin**, which needs `res_9_ninit`, `res_9_nsearch`
  and `res_9_nclose` from libresolv. `project.yml` adds `-lresolv`; without it the link fails with
  three undefined symbols and no hint as to why.

Signing is what remains: `NUNYA_TEAM_ID=<your team> ./scripts/build-extension.sh` signs the
extension, and the team must have the NetworkExtension capability enabled for both bundle
identifiers. That capability comes only with a paid Apple Developer Program membership, and an
unsigned Network Extension will not load. Unsigned, everything still compiles, and
`NetworkExtensionTransport` reports the system's own error rather than pretending.

## Developing without an Apple Developer account

You do not need one to work on this. `TunnelTransport` has two implementations, and the choice is an
environment variable:

```bash
./scripts/dev-linux.sh                               # preferred: a real tunnel, no root on your machine
./scripts/dev-tunnel.sh                              # last resort: runs the whole app under sudo
NUNYA_TRANSPORT=networkextension npm run tauri dev   # once the extension can be signed
```

Reach for `dev-linux.sh` first. `dev-tunnel.sh` elevates the *entire app* (the webview, the
frontend and all) when only the core needs to create a utun, and because it uses `sudo -E` it
inherits your `HOME`: the moment that root instance saves, `data.json` becomes root-owned, and your
normal unprivileged instance silently fails every write afterwards. It leaves nothing privileged
behind (no setuid bit, no installed daemon), which is why it uses `sudo` rather than `chmod u+s` on
the core.

The transport default is deliberately `subprocess` (see `transport/select.rs`). Defaulting to
NetworkExtension before it can be signed would leave anyone without a membership looking at a
tunnel that silently never starts. When the membership arrives: sign the extension, flip
`NUNYA_TRANSPORT`, and nothing above the trait changes.

## Subscriptions, protocols and transports

### Subscriptions

A subscription address goes into the same box as share links: an `https://` line is taken as a
subscription and gets its own group, which can then be refreshed. So is a panel's import link
(`sing-box://import-remote-profile?url=…` or `clash://install-config?url=…`), which wraps the real
address and is unwrapped each time it is fetched. Several body formats are in circulation, and
nothing in the headers tells them apart, so they are distinguished by content:

| | |
| --- | --- |
| A list of share links | one per line, plain or base64-encoded. The common case. |
| An Xray configuration, or an array of them | what a BPB panel serves to `?app=xray`: each entry is a whole client config wrapped around a single `proxy` outbound. |
| A sing-box configuration | what BPB serves to `?app=sing-box`: every server is an outbound (or, for WireGuard, an endpoint), beside groups that are skipped. |
| A Clash configuration in JSON | what BPB serves to `?app=clash`: every server is in `proxies`. Clash YAML is refused by name. |

The configurations are rewritten back into share links in `subscription.rs`, all through one
writer, so the three forms of one subscription import identically. Two decisions there are easy to
get wrong. A configuration with no outbound tagged `proxy` is a load balancer, and is skipped rather
than flattened, because the servers behind it are already listed individually. And protocols this
build cannot run are emitted anyway, under their own scheme, so they come back named rather than
missing: a subscription that quietly returned four of its eight servers, with nothing to say why,
would be worse than one that explains itself.

### Protocols and transports

| | |
| --- | --- |
| Protocols | VLESS, VMess (both share-link forms, including the base64 blob), Trojan, WireGuard |
| Transports | TCP, WebSocket, gRPC, HTTP/2, HTTPUpgrade, QUIC |
| Security | none, TLS, Reality, with uTLS fingerprints and ALPN |
| Rejected by name | mKCP, XHTTP, SplitHTTP, meek: the core has no implementation, and silently downgrading one to TCP would connect to the wrong thing |
| Also rejected by name | multi-hop chains, until proxy chains exist ([#18](https://github.com/nunyavpn/nunya/issues/18)) |
| Not yet | Shadowsocks, Hysteria2, TUIC, SSH and more: separate outbound types rather than another transport |

WireGuard is the one that is not an outbound at all. sing-box moved it to `endpoints`, because it is
an interface with its own addresses rather than a dialer, and the core rejects the old outbound form
outright. `config::proxy_node` is where that fork lives. Cloudflare WARP works through this path,
its `reserved` client id included: a wrong `reserved` is dropped by the server without an error, so
it is carried rather than treated as optional.

Each transport is validated against a real core in
`src-tauri/tests/core_link.rs::every_transport_is_accepted_by_the_core`, because the shape this
client emits and the shape sing-box accepts differ in ways unit tests cannot see: `host` is a
string for HTTPUpgrade and a list for HTTP/2, and plain TCP means *no* `transport` key rather than
an empty one.

## Releasing

A release is a tag. Bump the version in `package.json`, `src-tauri/tauri.conf.json` and
`src-tauri/Cargo.toml`, merge that to `main`, then:

```bash
git tag v0.2.0 && git push origin v0.2.0
```

[`release.yml`](.github/workflows/release.yml) refuses a tag that isn't on `main` or doesn't match
the version. It builds a **macOS arm64** `.dmg` (and a zipped `.app`) plus **Linux x86-64 and
arm64** `.deb` and `.AppImage`, each against the core pinned in `core.lock`, and publishes them
with a `SHA256SUMS` as a GitHub release. Versions below 1.0, and tags with a suffix
(`v0.2.0-beta.1`), are marked pre-release.

The macOS build is ad-hoc signed, not notarised: the first time it's opened, macOS asks the user to
allow it in **System Settings → Privacy & Security → Open Anyway**. A signed, notarised build with
the packet tunnel extension (VPN mode on macOS) needs an Apple Developer account; that's
`build-app.sh`.

## Conventions

- **Comments explain why, not what.** Every module opens with a rationale header covering the
  decision and the alternative that was rejected. A change that arrives without it reads as foreign.
- **Reject by name; never silently downgrade.** A share link with an unrunnable transport is
  refused with a reason. Quietly treating it as TCP produces a config the core accepts and a tunnel
  that never passes traffic.
- **Test names are sentences**: `the_tunnel_takes_the_default_route_and_gives_it_back`.
- **No framework in the frontend.** Every shipped byte is something a user has to trust;
  [`src/dom.ts`](src/dom.ts) is the whole abstraction.
- **Do not edit `.xcodeproj`**: it is generated from `project.yml` by XcodeGen, and gitignored.
- There is no JS linter and **no `cargo fmt` gate**: the tree is not rustfmt-clean, so do not
  reformat files you are not otherwise editing.

`CLAUDE.md` holds the longer notes on each subsystem: the modes and their honesty rules, the
system proxy, server locations, latency testing, usage, Quick Connect, sharing, and support.
