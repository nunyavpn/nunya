#!/usr/bin/env bash
#
# Builds NunyaTunnel.appex, the packet tunnel extension, and copies it into the app bundle.
#
# Order matters: the extension links NunyaCore.xcframework, so scripts/fetch-core.sh has to
# have run first, and the Tauri bundle has to exist before there is anywhere to copy the extension.
#
#   ./scripts/fetch-core.sh
#   npm run tauri build -- --bundles app
#   ./scripts/build-extension.sh
#
# Signing: an unsigned extension will not load. Set NUNYA_TEAM_ID to sign with your Apple
# Developer team, which must have the NetworkExtension capability enabled for both bundle IDs.
# Without it this produces an unsigned bundle that compiles but cannot run — useful for catching
# build errors, useless for actually tunnelling.

set -euo pipefail

cd "$(dirname "$0")/.."
APP_DIR="$PWD"

CONFIG="${CONFIG:-Release}"
DERIVED="${DERIVED:-$APP_DIR/vendor/core/xcode}"
FRAMEWORK="$APP_DIR/vendor/core/apple/NunyaCore.xcframework"
BUNDLE="$APP_DIR/src-tauri/target/release/bundle/macos/Nunya.app"

if [[ ! -d "$FRAMEWORK" ]]; then
  echo "error: $FRAMEWORK is missing. Run ./scripts/fetch-core.sh first." >&2
  exit 1
fi

if ! command -v xcodegen >/dev/null 2>&1; then
  echo "error: xcodegen is not installed (brew install xcodegen)." >&2
  exit 1
fi

echo "==> generating the Xcode project from project.yml"
xcodegen generate --quiet

SIGN_ARGS=()
if [[ -n "${NUNYA_TEAM_ID:-}" ]]; then
  echo "==> signing with team $NUNYA_TEAM_ID"
  SIGN_ARGS+=("DEVELOPMENT_TEAM=$NUNYA_TEAM_ID")
else
  echo "==> WARNING: no NUNYA_TEAM_ID set, building unsigned"
  echo "    The extension will compile but the system will refuse to load it, because"
  echo "    com.apple.developer.networking.networkextension requires a signed bundle."
  SIGN_ARGS+=("CODE_SIGNING_ALLOWED=NO")
fi

echo "==> building NunyaTunnel.appex ($CONFIG)"
xcodebuild build \
  -project NunyaTunnel.xcodeproj \
  -scheme NunyaTunnel \
  -configuration "$CONFIG" \
  -derivedDataPath "$DERIVED" \
  "${SIGN_ARGS[@]}"

APPEX="$DERIVED/Build/Products/$CONFIG/NunyaTunnel.appex"
if [[ ! -d "$APPEX" ]]; then
  echo "error: expected $APPEX, but the build produced nothing" >&2
  exit 1
fi

echo "==> built $APPEX"
du -sh "$APPEX"

if [[ -d "$BUNDLE" ]]; then
  # Extensions live in Contents/PlugIns; the system looks nowhere else.
  echo "==> embedding into $BUNDLE"
  mkdir -p "$BUNDLE/Contents/PlugIns"
  rm -rf "$BUNDLE/Contents/PlugIns/NunyaTunnel.appex"
  cp -R "$APPEX" "$BUNDLE/Contents/PlugIns/"
  echo "==> embedded"
else
  echo "==> no app bundle at $BUNDLE yet; build it with 'npm run tauri build -- --bundles app'"
  echo "    then re-run this script to embed the extension."
fi
