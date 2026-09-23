#!/usr/bin/env bash
#
# Refreshes what `src-tauri/src/cloudflare.rs` knows about Cloudflare's network, from Cloudflare.
#
#   ./scripts/update-cloudflare-data.sh
#
# Two facts, each from Cloudflare's own machine-readable source and nobody's copy of it:
#
#   ips-v4.txt, ips-v6.txt  the address ranges Cloudflare publishes as its own
#                           (https://www.cloudflare.com/ips/) — what "this address is Cloudflare's"
#                           is decided against
#   colos.json              its data centers by IATA-style code, with city, country and coordinates
#                           (https://speed.cloudflare.com/locations) — what turns the "FRA" read off
#                           a CF-Ray header into Frankfurt
#
# The files are compiled in, so a refresh is this script, a look at the diff, and a commit. Nothing
# is fetched at run time: the networks this client is for are the ones where that fetch would fail.
#
# Each download is checked before it replaces anything: a range list must be all CIDRs of its
# family, and the colo list must parse and be the size of Cloudflare's network, not an error page.

set -euo pipefail

cd "$(dirname "$0")/../src-tauri/data/cloudflare"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

fetch() { # fetch <url> <out> [curl args...]
  local url="$1" out="$2"
  shift 2
  curl --fail --silent --show-error --location --max-time 30 "$@" "$url" -o "$out"
}

fetch https://www.cloudflare.com/ips-v4 "$tmp/ips-v4.txt"
fetch https://www.cloudflare.com/ips-v6 "$tmp/ips-v6.txt"
# Without a Referer from its own page the endpoint answers `{}`.
fetch https://speed.cloudflare.com/locations "$tmp/colos.raw.json" \
  -H "Referer: https://speed.cloudflare.com/" -A "Mozilla/5.0"

python3 - "$tmp" <<'PY'
import ipaddress, json, sys
tmp = sys.argv[1]

for family, version in (("v4", 4), ("v6", 6)):
    path = f"{tmp}/ips-{family}.txt"
    lines = [l.strip() for l in open(path) if l.strip()]
    if not lines:
        sys.exit(f"ips-{family}: empty")
    for l in lines:
        net = ipaddress.ip_network(l)  # raises on anything that is not a CIDR
        if net.version != version:
            sys.exit(f"ips-{family}: {l} is not IPv{version}")
    open(path, "w").write("\n".join(lines) + "\n")

colos = json.load(open(f"{tmp}/colos.raw.json"))
if not isinstance(colos, list) or len(colos) < 100:
    sys.exit(f"colos: expected Cloudflare's list of data centers, got {str(colos)[:80]!r}")
keep = ("iata", "city", "cca2", "region", "lat", "lon")
out = []
for c in sorted(colos, key=lambda c: c["iata"]):
    if len(c.get("iata", "")) != 3 or len(c.get("cca2", "")) != 2:
        sys.exit(f"colos: malformed entry {c!r}")
    out.append({k: c[k] for k in keep if k in c})
with open(f"{tmp}/colos.json", "w") as f:
    # One data center per line, so a refresh reads as a diff of what changed.
    f.write("[\n" + ",\n".join(json.dumps(c, ensure_ascii=False) for c in out) + "\n]\n")
print(f"{len(out)} data centers")
PY

mv "$tmp/ips-v4.txt" ips-v4.txt
mv "$tmp/ips-v6.txt" ips-v6.txt
mv "$tmp/colos.json" colos.json
echo "updated $(pwd)"
