#!/usr/bin/env bash
#
# Runs the core and its tests inside a Linux container, where a real tunnel can be brought up
# without root on your machine.
#
#   ./scripts/dev-linux.sh              build the core, run every test including the tunnel ones
#   ./scripts/dev-linux.sh shell        a prompt inside the container
#   ./scripts/dev-linux.sh test <name>  one test by name
#
# Why this exists: bringing up a TUN on the host means `sudo`, and it means rewriting the machine's
# actual default route to run an assertion about routing. In a container the tunnel gets its own
# network namespace — the route table it takes over is the container's, and it disappears when the
# container does. The only privilege involved is CAP_NET_ADMIN plus /dev/net/tun, scoped to that
# container.
#
# What this does NOT do is run the UI. Tauri on Linux renders through WebKitGTK; forwarding that to
# macOS over X11 is a worse loop than the one you already have. Work on the interface with
# `VITE_MOCK=1 npm run tauri dev` on the host, and use this for the layers underneath it.

set -euo pipefail

cd "$(dirname "$0")/.."
APP_DIR="$PWD"
CORE_SRC="${NUNYA_CORE_SRC:-$APP_DIR/../nunya-core}"
IMAGE="${NUNYA_LINUX_IMAGE:-nunya-dev:linux}"

[[ -d "$CORE_SRC" ]] || {
  echo "error: no core checkout at $CORE_SRC (override with NUNYA_CORE_SRC)" >&2
  exit 1
}
CORE_SRC="$(cd "$CORE_SRC" && pwd)"

command -v docker >/dev/null || {
  cat >&2 <<'MSG'
error: docker is not on PATH.

  Colima is the lighter option on Apple Silicon — a Linux VM and nothing else:

    brew install colima docker
    colima start --cpu 4 --memory 8

  Docker Desktop works too.
MSG
  exit 1
}

docker info >/dev/null 2>&1 || {
  echo "error: docker is installed but no daemon is running. Try: colima start" >&2
  exit 1
}

echo "==> building the image (cached after the first run)"
docker build -q -t "$IMAGE" "$APP_DIR/docker" >/dev/null

# A named volume for build caches. Without it every run recompiles Tauri and its GTK bindings from
# scratch, which is minutes rather than seconds.
docker volume create nunya-linux-cache >/dev/null

# --cap-add NET_ADMIN and /dev/net/tun are what let the core open a TUN. That is the whole privilege
# budget: no --privileged, no root on the host, nothing that outlives the container.
#
# Both repositories are mounted because core.lock pins a release that has not been cut, so the core
# has to be built from the sibling checkout.
#
# GOPROXY defaults to direct rather than the public proxy. proxy.golang.org serves module zips by
# redirecting to Google storage, and where that host is unreachable the redirect comes back 403 —
# which Go does not treat as a reason to try the next proxy, since only 404 and 410 fall through.
# The result is a build that dies partway through resolving dependencies. Fetching from the source
# repositories avoids the redirect entirely; go.sum still verifies every module. Override by
# exporting GOPROXY if the proxy works for you.
# `-it` only when there is a terminal to attach to. Without the guard this fails outright under
# CI, a pipe, or any non-interactive caller with "the input device is not a TTY".
# A plain string rather than an array: macOS ships bash 3.2, where expanding an empty array under
# `set -u` is itself an unbound-variable error. Unquoted below so an empty value disappears instead
# of becoming an empty argument.
TTY_FLAGS=""
[[ -t 0 && -t 1 ]] && TTY_FLAGS="-it"

run_in_container() {
  # shellcheck disable=SC2086
  docker run --rm $TTY_FLAGS \
    --cap-add NET_ADMIN \
    --device /dev/net/tun \
    -v "$APP_DIR:/work/nunya" \
    -v "$CORE_SRC:/work/nunya-core" \
    -v nunya-linux-cache:/cache \
    -e NUNYA_CORE_SRC=/work/nunya-core \
    -e GOPROXY="${GOPROXY:-direct}" \
    -w /work/nunya \
    "$IMAGE" bash -c "$1"
}

BUILD_CORE='
set -euo pipefail
if [[ ! -x /cache/core/nunya-core ]]; then
  echo "==> building the core for linux from /work/nunya-core"
  mkdir -p /cache/core
  cd /work/nunya-core
  # Into the cache volume rather than the bind mount, so the Linux binary never lands in the host
  # tree next to the macOS one that fetch-core.sh put there.
  DEST=/cache/core ./scripts/build.sh
fi
export NUNYA_CORE_PATH=/cache/core/nunya-core

# Tauri resolves sidecars by <name>-<target-triple> and refuses to build — even `cargo check` —
# while the file named in externalBin is missing. The host tree only carries the macOS triple, so
# the Linux one has to be staged here or every cargo invocation below fails before it starts.
# Gitignored, like the macOS one fetch-core.sh stages.
# sed rather than awk here: BUILD_CORE is a single-quoted string, so a single quote anywhere
# inside it (including in awk syntax, or in an apostrophe) closes the string early and hands the
# rest to the outer shell.
TRIPLE="$(rustc -vV | sed -n "s/^host: //p")"
if [[ ! -x "/work/nunya/src-tauri/binaries/nunya-core-$TRIPLE" ]]; then
  echo "==> staging the sidecar for $TRIPLE"
  mkdir -p /work/nunya/src-tauri/binaries
  cp /cache/core/nunya-core "/work/nunya/src-tauri/binaries/nunya-core-$TRIPLE"
  chmod +x "/work/nunya/src-tauri/binaries/nunya-core-$TRIPLE"
fi

cd /work/nunya/src-tauri
'

case "${1:-test-all}" in
  shell)
    run_in_container "$BUILD_CORE"'exec bash'
    ;;
  test)
    NAME="${2:?usage: dev-linux.sh test <name>}"
    # A name is either a test target (tests/<name>.rs) or a filter on test names. Passing a target
    # name as a filter matches nothing and reports success with everything filtered out, which
    # reads exactly like a passing run — so the two are told apart here rather than by the caller.
    if [[ -f "$APP_DIR/src-tauri/tests/$NAME.rs" ]]; then
      SELECT="--test $NAME"
    else
      SELECT="$NAME"
    fi
    # --test-threads=1 because each test binds its own core socket and spawns a core; running them
    # together races on the socket directory.
    run_in_container "$BUILD_CORE"'cargo test '"$SELECT"' -- --ignored --test-threads=1 --nocapture'
    ;;
  test-all)
    run_in_container "$BUILD_CORE"'
      echo "==> unit tests"
      cargo test --lib
      echo "==> integration tests (core + tunnel)"
      cargo test --tests -- --ignored --test-threads=1
    '
    ;;
  *)
    echo "usage: dev-linux.sh [test-all|test <name>|shell]" >&2
    exit 1
    ;;
esac
