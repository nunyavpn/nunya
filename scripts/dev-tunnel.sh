#!/usr/bin/env bash
#
# Runs the app with a tunnel that actually comes up, without an Apple Developer account.
#
# Creating a utun on macOS requires root, and there are exactly two ways to get there: let the
# system do it (NetworkExtension, which needs a signed extension), or be root yourself. This script
# takes the second route, deliberately and only for development.
#
#   ./scripts/dev-tunnel.sh
#
# What it does NOT do is leave anything privileged behind. The app is run under sudo for this one
# session; no setuid bit is set, no daemon is installed, and nothing persists after you quit. That
# matters because the Qt client's approach — chmod u+s on the core — leaves a root-owned binary any
# local process can exec, for as long as it is on disk.
#
# The shipping answer is NetworkExtension, where nothing is privileged at all. This is scaffolding
# until that can be signed; see README.md.

set -euo pipefail

cd "$(dirname "$0")/.."
APP_DIR="$PWD"

CORE="$APP_DIR/vendor/core/bin/nunya-core"
CORE_SRC="${NUNYA_CORE_SRC:-../nunya-core}"

# A *released* core is a release build, and a release build refuses to start unless its parent is a
# binary named `Nunya` in the same directory. Here the parent is a debug binary in target/, so this
# needs a core built with the noparentcheck tag — which only ./scripts/fetch-core.sh --source
# produces, and which is deliberately never published.
if [[ ! -x "$CORE" || "$(cat "$APP_DIR/vendor/core/.origin" 2>/dev/null)" != "source" ]]; then
  echo "==> building a development core from $CORE_SRC"
  NUNYA_SKIP_APPLE=1 ./scripts/fetch-core.sh --source "$CORE_SRC"
fi

echo "==> building the frontend"
npm run build

echo "==> building the app"
# Built unprivileged on purpose: only the run needs root, and a root-owned target/ directory would
# then need sudo for every later build.
( cd src-tauri && cargo build )

# `tauri build` renames the binary to mainBinaryName, but a plain `cargo build` leaves it named
# after the package. Accept either so this works whichever produced target/debug.
BINARY=""
for candidate in "$APP_DIR/src-tauri/target/debug/Nunya" "$APP_DIR/src-tauri/target/debug/nunya"; do
  [[ -x "$candidate" ]] && { BINARY="$candidate"; break; }
done

if [[ -z "$BINARY" ]]; then
  echo "error: no binary at src-tauri/target/debug/{Nunya,nunya} after the build" >&2
  exit 1
fi

cat <<EOF

  Running Nunya as root so the core can create a utun.

  This is a development shortcut, not how the app ships. The release path is the packet tunnel
  extension, where macOS owns the utun and nothing runs as root. Quit the app to drop the privilege;
  nothing is left installed.

EOF

# -E keeps NUNYA_CORE_PATH and RUST_LOG; without it root gets a clean environment and the app
# looks for the core next to the binary instead.
exec sudo -E \
  env NUNYA_CORE_PATH="$CORE" \
      NUNYA_TRANSPORT="${NUNYA_TRANSPORT:-subprocess}" \
      RUST_LOG="${RUST_LOG:-info}" \
  "$BINARY"
