#!/usr/bin/env bash
#
# Puts the core artefacts under vendor/core/, where the build expects them.
#
# This client does not build the core. It pins a nunya-core release in core.lock, downloads that
# release's assets and verifies them before unpacking — so the RPC contract between the two repos
# is a published, versioned interface rather than a relative path into a sibling checkout.
#
#   ./scripts/fetch-core.sh                    download the pinned release (default)
#   ./scripts/fetch-core.sh --update v0.2.0    repin core.lock to a tag, then download it
#   ./scripts/fetch-core.sh --source ../nunya-core   build from a local core checkout instead
#
# Layout it produces (all gitignored):
#
#   vendor/core/proto/nunya.proto              what build.rs generates the Rust bindings from
#   vendor/core/bin/nunya-core                 the core for this host, for bundling
#   vendor/core/apple/NunyaCore.xcframework    linked into the packet tunnel extension
#   vendor/core/SHA256SUMS                     what everything above was checked against

set -euo pipefail
cd "$(dirname "$0")/.."

REPO="${NUNYA_CORE_REPO:-nunyavpn/nunya-core}"
LOCK="core.lock"
DEST="vendor/core"

read_lock() { sed -n "s/^$1[[:space:]]*=[[:space:]]*//p" "$LOCK" | head -1; }

MODE=download
SOURCE_DIR="${NUNYA_CORE_SRC:-}"
[[ -n "$SOURCE_DIR" ]] && MODE=source

while [[ $# -gt 0 ]]; do
  case "$1" in
    --update) MODE=update; NEW_TAG="${2:?--update needs a tag}"; shift 2 ;;
    --source) MODE=source; SOURCE_DIR="${2:?--source needs a path}"; shift 2 ;;
    -h|--help) sed -n '2,20p' "$0"; exit 0 ;;
    *) echo "error: unknown argument $1" >&2; exit 1 ;;
  esac
done

host_triple_os() { case "$(uname -s)" in Darwin) echo darwin ;; Linux) echo linux ;; *) echo windows ;; esac; }
host_arch()      { case "$(uname -m)" in arm64|aarch64) echo arm64 ;; *) echo amd64 ;; esac; }

# Tauri resolves sidecars by <name>-<target-triple> and refuses to build — even `cargo check` —
# while the file named in externalBin is missing. Staging it here is what makes the tree compile
# straight after a fetch. It also lands the core in Contents/MacOS/nunya-core beside the app binary
# when bundled, which is exactly where a release core's parent check expects to find itself.
#
# A source-built core is staged too, so a dev tree compiles; scripts/build-app.sh is what refuses to
# put one in a bundle.
stage_sidecar() {
  command -v rustc >/dev/null 2>&1 || { echo "==> rustc not found, skipping sidecar staging"; return; }
  local triple src ext=""
  triple="$(rustc -vV | awk '/^host:/ {print $2}')"
  [[ "$(host_triple_os)" == "windows" ]] && ext=".exe"
  src="$DEST/bin/nunya-core$ext"
  [[ -f "$src" ]] || return
  mkdir -p src-tauri/binaries
  cp "$src" "src-tauri/binaries/nunya-core-$triple$ext"
  chmod +x "src-tauri/binaries/nunya-core-$triple$ext"
  echo "==> sidecar staged at src-tauri/binaries/nunya-core-$triple$ext"
}

# ---------------------------------------------------------------------------- build from source
#
# For working on the core itself, and for a development tunnel: released cores are release builds,
# which refuse to run unless their parent is a binary named `Nunya` in the same directory. A dev
# client's parent is cargo, so it needs a core built with the noparentcheck tag.
#
# That tag is deliberately never published. A downloadable core with its parent check disabled is a
# root-capable binary anything on the machine could drive, and no convenience is worth shipping one.
if [[ "$MODE" == "source" ]]; then
  [[ -d "$SOURCE_DIR" ]] || { echo "error: $SOURCE_DIR is not a directory" >&2; exit 1; }
  SOURCE_DIR="$(cd "$SOURCE_DIR" && pwd)"
  echo "==> building the core from $SOURCE_DIR"

  mkdir -p "$DEST/bin" "$DEST/proto"
  ( cd "$SOURCE_DIR" && DEST="$PWD/build/dev/host" ./scripts/build.sh )
  cp "$SOURCE_DIR"/build/dev/host/nunya-core* "$DEST/bin/"
  cp "$SOURCE_DIR/proto/nunya.proto" "$DEST/proto/nunya.proto"

  if [[ "$(uname -s)" == "Darwin" ]]; then
    if [[ "${NUNYA_SKIP_APPLE:-0}" != "1" ]]; then
      echo "==> building NunyaCore.xcframework (skip with NUNYA_SKIP_APPLE=1)"
      mkdir -p "$DEST/apple"
      ( cd "$SOURCE_DIR" && DEST="$PWD/build/apple" ./scripts/build-apple.sh )
      rm -rf "$DEST/apple/NunyaCore.xcframework"
      cp -R "$SOURCE_DIR/build/apple/NunyaCore.xcframework" "$DEST/apple/"
    fi
  fi

  echo "source" > "$DEST/.origin"
  stage_sidecar
  echo "==> core built from source into $DEST"
  echo "    this is a DEVELOPMENT core: its parent check is off, and it must not be bundled."
  exit 0
fi

# -------------------------------------------------------------------------------- download
command -v curl >/dev/null || { echo "error: curl is required" >&2; exit 1; }

if [[ "$MODE" == "update" ]]; then
  TAG="$NEW_TAG"
else
  TAG="$(read_lock tag)"
  EXPECTED="$(read_lock sha256_sums)"
fi

BASE="https://github.com/$REPO/releases/download/$TAG"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

echo "==> fetching $REPO $TAG"
if ! curl -fsSL "$BASE/SHA256SUMS" -o "$TMP/SHA256SUMS"; then
  cat >&2 <<MSG
error: could not download $BASE/SHA256SUMS

  If nunya-core has not cut its first release yet, build from a local checkout instead:

    ./scripts/fetch-core.sh --source ../nunya-core
MSG
  exit 1
fi

ACTUAL="$(shasum -a 256 "$TMP/SHA256SUMS" | awk '{print $1}')"

if [[ "$MODE" == "update" ]]; then
  echo "==> repinning core.lock to $TAG ($ACTUAL)"
  perl -pi -e "s|^tag = .*|tag = $TAG|; s|^sha256_sums = .*|sha256_sums = $ACTUAL|" "$LOCK"
  EXPECTED="$ACTUAL"
elif [[ "$ACTUAL" != "$EXPECTED" ]]; then
  cat >&2 <<MSG
error: SHA256SUMS for $TAG does not match core.lock

  expected  $EXPECTED
  got       $ACTUAL

  Either the release was re-cut under the same tag, or the download was tampered with. Nothing has
  been unpacked. Repin deliberately with:  ./scripts/fetch-core.sh --update $TAG
MSG
  exit 1
fi

ASSET_CORE="nunya-core-$(host_triple_os)-$(host_arch)"
[[ "$(host_triple_os)" == "windows" ]] && ASSET_CORE="$ASSET_CORE.exe"
ASSETS=("$ASSET_CORE" "nunya.proto")
# The xcframework is only for the packet tunnel extension, which needs an Apple Developer account to
# build at all. A release that does not publish one is still a complete release for everything else
# — the app, its proxy mode, and a subprocess tunnel — so it is fetched when listed and skipped when not.
if [[ "$(uname -s)" == "Darwin" ]]; then
  if grep -qF "  NunyaCore.xcframework.zip" "$TMP/SHA256SUMS"; then
    ASSETS+=("NunyaCore.xcframework.zip")
  else
    echo "==> $TAG publishes no NunyaCore.xcframework.zip; the packet tunnel extension cannot be built from it"
  fi
fi

for asset in "${ASSETS[@]}"; do
  echo "==> $asset"
  curl -fsSL "$BASE/$asset" -o "$TMP/$asset"
done

# Each asset is verified against the SHA256SUMS whose own digest core.lock pinned, before anything
# is unpacked or made executable. Checked one at a time so a name containing regex metacharacters
# cannot quietly widen the match.
echo "==> verifying"
for asset in "${ASSETS[@]}"; do
  line="$(grep -F "  $asset" "$TMP/SHA256SUMS" || true)"
  [[ -n "$line" ]] || { echo "error: $asset is not listed in SHA256SUMS" >&2; exit 1; }
  ( cd "$TMP" && printf '%s\n' "$line" | shasum -a 256 -c - >/dev/null ) \
    || { echo "error: $asset failed its checksum" >&2; exit 1; }
  echo "    ok  $asset"
done

rm -rf "$DEST"
mkdir -p "$DEST/bin" "$DEST/proto"

CORE_BIN="nunya-core"
[[ "$(host_triple_os)" == "windows" ]] && CORE_BIN="nunya-core.exe"
cp "$TMP/$ASSET_CORE" "$DEST/bin/$CORE_BIN"
chmod +x "$DEST/bin/$CORE_BIN"

cp "$TMP/nunya.proto" "$DEST/proto/nunya.proto"
cp "$TMP/SHA256SUMS" "$DEST/SHA256SUMS"

if [[ -f "$TMP/NunyaCore.xcframework.zip" ]]; then
  mkdir -p "$DEST/apple"
  unzip -q "$TMP/NunyaCore.xcframework.zip" -d "$DEST/apple"
fi

echo "$TAG" > "$DEST/.origin"
stage_sidecar
echo "==> core $TAG installed into $DEST"
