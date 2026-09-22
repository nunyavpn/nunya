#!/usr/bin/env bash
#
# Builds the shippable macOS app: pinned core, Tauri bundle, packet tunnel extension.
#
#   ./scripts/build-app.sh
#
# The steps have a strict order. The core has to be on disk before cargo runs, because build.rs
# generates the RPC bindings from its proto. The bundle has to exist before the extension can be
# embedded, because there is nowhere to put it otherwise.
#
# Signing: set NUNYA_TEAM_ID to an Apple Developer team with the NetworkExtension capability enabled
# for both bundle IDs. Without it the extension compiles but the system refuses to load it, which is
# useful for catching build errors and useless for actually tunnelling.

set -euo pipefail
cd "$(dirname "$0")/.."
APP_DIR="$PWD"

# 1. The pinned core. Refuses to proceed on a core built from source: those have their parent check
#    disabled, and a bundle must never ship one.
if [[ ! -f vendor/core/proto/nunya.proto ]]; then
  echo "==> fetching the pinned core"
  ./scripts/fetch-core.sh
fi

if [[ "$(cat vendor/core/.origin 2>/dev/null)" == "source" ]]; then
  cat >&2 <<'MSG'
error: vendor/core holds a development core, built from source with its parent check disabled.

  A release bundle must not ship one. Install the pinned release instead:

    ./scripts/fetch-core.sh
MSG
  exit 1
fi

# 2. Frontend and bundle. scripts/fetch-core.sh already staged the core as a Tauri sidecar, which
#    is what puts it in Contents/MacOS/nunya-core beside the app binary.
#
#    With the NetworkExtension entitlements merged in, which the default build leaves out: they need
#    an Apple Developer team's signature, and macOS kills an app that claims them without one. That
#    is why they live in tauri.networkextension.conf.json and not tauri.conf.json — the CI release,
#    with no team, is ad-hoc signed without them and runs its core as a child process.
echo "==> building the app bundle"
npm run tauri build -- --bundles app --config src-tauri/tauri.networkextension.conf.json

# 3. The packet tunnel extension, embedded into the bundle just built.
if [[ "$(uname -s)" == "Darwin" ]]; then
  echo "==> building and embedding the packet tunnel extension"
  ./scripts/build-extension.sh
fi

echo
echo "==> done: $APP_DIR/src-tauri/target/release/bundle/macos/Nunya.app"
