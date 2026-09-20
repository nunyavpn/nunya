# A Linux box for exercising the tunnel.
#
# The point of this image is not Linux. It is the network namespace: inside a container the core can
# create a real TUN, take over the default route and be checked for DNS leaks, all against a routing
# table that is thrown away when the container exits. On the host, the same test rewrites the
# machine's actual default route and needs root to do it.
#
# It is also where the privilege story ends cleanly. The container is given CAP_NET_ADMIN and
# /dev/net/tun and nothing else — not root on your machine, and no Apple Developer account.
#
# This image builds and tests. It does not run the GUI: Tauri on Linux renders through WebKitGTK,
# and forwarding that to a Mac over X11 is a worse development loop than the browser one you already
# have. UI work stays on the host with `VITE_MOCK=1 npm run tauri dev`.

# Go comes from the official image rather than a tarball URL.
#
# dl.google.com — which go.dev/dl redirects to — served 404 for every current linux/arm64 release
# while the JSON index still listed the same filenames. A build that depends on that URL is a build
# that breaks for reasons nothing in this repository can see or fix. The bookworm variant is chosen
# so the toolchain's glibc matches the stage it is copied into.
ARG GO_VERSION=1.27.1
FROM golang:${GO_VERSION}-bookworm AS go

FROM debian:bookworm-slim

ARG GO_VERSION=1.27.1
ARG RUST_VERSION=stable
ARG TARGETARCH=arm64

ENV DEBIAN_FRONTEND=noninteractive

# Two groups of packages, for two different reasons.
#
# The build group includes libwebkit2gtk because `nunya_lib` depends on `tauri`, and tauri's build
# script needs the GTK/WebKit development headers on Linux even for a target that never opens a
# window. Nothing here launches a browser engine; it is a link-time dependency only.
#
# The network group is what the tunnel assertions actually use: `ip` to read interfaces and routes,
# `dig` to prove a query went where it was supposed to, `curl` to push traffic through.
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential pkg-config curl wget file ca-certificates git unzip \
      libssl-dev libgtk-3-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev \
      libwebkit2gtk-4.1-dev librsvg2-dev libayatana-appindicator3-dev \
      protobuf-compiler \
      iproute2 iputils-ping dnsutils procps \
 && rm -rf /var/lib/apt/lists/*

# Go, for building the core from source. The pinned release in core.lock is not cut yet, so
# scripts/fetch-core.sh --source is the only route to a core, and that needs a Go toolchain.
COPY --from=go /usr/local/go /usr/local/go
ENV PATH="/usr/local/go/bin:/root/.cargo/bin:${PATH}"
ENV GOPATH=/go
ENV GOCACHE=/cache/go-build
ENV GOMODCACHE=/cache/go-mod

RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
  | sh -s -- -y --profile minimal --default-toolchain "${RUST_VERSION}" --no-modify-path

# Kept outside the bind-mounted source tree so a rebuild does not fight the host's target/ directory,
# which holds macOS objects that are useless here and whose absolute paths would not resolve anyway.
ENV CARGO_TARGET_DIR=/cache/target
ENV CARGO_HOME=/cache/cargo

# The core's scripts/gen-proto.sh runs before every build and requires both plugins on PATH, not
# just protoc. Appended last so editing this never invalidates the expensive apt layer above.
#
# The module cache is redirected away from /cache for this one step: /cache is a volume at runtime,
# so anything written there during the build is shadowed and wasted.
ENV PATH="/go/bin:${PATH}"
RUN GOMODCACHE=/go/pkg/mod GOCACHE=/root/.cache/go-build \
      go install google.golang.org/protobuf/cmd/protoc-gen-go@latest \
 && GOMODCACHE=/go/pkg/mod GOCACHE=/root/.cache/go-build \
      go install google.golang.org/grpc/cmd/protoc-gen-go-grpc@latest \
 && protoc-gen-go --version && protoc-gen-go-grpc --version

WORKDIR /work/nunya
