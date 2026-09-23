#!/usr/bin/env bash
#
# What answers from this network — run it on the one that is failing.
#
# Nunya is used where names are poisoned, addresses are blocked, or both, and the error the app
# reports does not say which: "Dns Failed" on every endpoint at once means the names never
# resolved, while a stall means the addresses are unreachable. The fix differs — a resolver of our
# own, endpoints reached by address, or something else — so this asks each question separately.
#
#   ./scripts/net-check.sh
#
# It sends nothing but these probes, reads nothing of yours, and prints one line each. Paste the
# output into the issue. Addresses it prints are the public ones a web server would see anyway.

set -u

TIMEOUT="${NUNYA_NET_CHECK_TIMEOUT:-4}"

# The names the app needs: where it asks what address it has (geo.rs) and where the block lists
# come from (blocklists.rs).
NAMES=(ipwho.is ipinfo.io api.ip.sb ip-api.com raw.githubusercontent.com)

# Resolvers to try by address, so the probe needs no working name resolution of its own. The last
# two are Iranian services widely used where the others are blocked.
RESOLVERS=(1.1.1.1 8.8.8.8 9.9.9.9 178.22.122.100 10.202.10.10)

say() { printf '%-52s %s\n' "$1" "$2"; }

timed() { # timed <label> <command...>
  local label="$1"
  shift
  local start out status
  start=$(date +%s)
  out=$("$@" 2>&1)
  status=$?
  local took=$(($(date +%s) - start))
  # One line per probe, however many the answer had.
  out=$(printf '%s' "$out" | tr '\n' ' ' | tr -s ' ')
  if [ "$status" -eq 0 ] && [ -n "$out" ]; then
    say "$label" "ok ${took}s  ${out:0:96}"
  else
    say "$label" "FAIL ${took}s  ${out:0:96}"
  fi
}

address_of() { # address_of <name> — from any resolver above that answers, plain DNS or HTTPS
  local name="$1" resolver address
  for resolver in "${RESOLVERS[@]}"; do
    address=$(dig +short +time=3 +tries=1 "@$resolver" "$name" A 2>/dev/null | grep -E '^[0-9.]+$' | head -1)
    if [ -n "$address" ]; then
      printf '%s' "$address"
      return
    fi
  done
  curl -sS --max-time "$TIMEOUT" -H "accept: application/dns-json" \
    "https://1.1.1.1/dns-query?name=$name&type=A" 2>/dev/null |
    python3 -c "import json,sys; print(next(a['data'] for a in json.load(sys.stdin).get('Answer', []) if a.get('type') == 1))" 2>/dev/null
}

echo "== the system's own resolver (what the app uses today) =="
for name in "${NAMES[@]}"; do
  timed "getaddrinfo $name" python3 -c "
import socket, sys
print(sorted({a[4][0] for a in socket.getaddrinfo(sys.argv[1], 443)})[0])" "$name"
done

echo
echo "== plain DNS, straight to a resolver by address =="
if command -v dig >/dev/null 2>&1; then
  for resolver in "${RESOLVERS[@]}"; do
    timed "dig @$resolver ipwho.is" dig +short +time=3 +tries=1 "@$resolver" ipwho.is A
  done
else
  say "dig" "missing — skipped"
fi

echo
echo "== DNS over HTTPS, to a resolver named by address (no name needed) =="
timed "1.1.1.1 DoH" curl -sS --max-time "$TIMEOUT" -H "accept: application/dns-json" \
  "https://1.1.1.1/dns-query?name=ipwho.is&type=A"
timed "8.8.8.8 DoH" curl -sS --max-time "$TIMEOUT" "https://8.8.8.8/resolve?name=ipwho.is&type=A"
timed "9.9.9.9 DoH" curl -sS --max-time "$TIMEOUT" -H "accept: application/dns-json" \
  "https://9.9.9.9:5053/dns-query?name=ipwho.is&type=A"

echo
echo "== reaching an address with no name at all =="
# Cloudflare's certificate carries this address, so TLS verifies without resolving anything. The
# answer holds the public address and the country, which is most of what the map needs.
timed "https://1.1.1.1/cdn-cgi/trace" curl -sS --max-time "$TIMEOUT" https://1.1.1.1/cdn-cgi/trace

echo
echo "== the endpoints the app asks, by name =="
timed "https://ipwho.is/" curl -sS --max-time "$TIMEOUT" https://ipwho.is/
timed "https://api.ip.sb/geoip" curl -sS --max-time "$TIMEOUT" -A Nunya https://api.ip.sb/geoip
timed "http://ip-api.com/json/" curl -sS --max-time "$TIMEOUT" "http://ip-api.com/json/?fields=status,query,countryCode,city"
timed "https://ipinfo.io/json" curl -sS --max-time "$TIMEOUT" https://ipinfo.io/json

echo
echo "== the same endpoints with an address supplied for the name =="
# If these answer while the ones above did not, the names are the problem and a resolver of our
# own fixes it. The addresses come from a resolver above, so this needs one of them to have worked.
for name in ipwho.is api.ip.sb; do
  address=$(address_of "$name")
  if [ -z "$address" ]; then
    say "https://$name via its address" "skipped — no resolver above gave one"
    continue
  fi
  path=$([ "$name" = "api.ip.sb" ] && echo "/geoip" || echo "/")
  timed "https://$name$path via $address" curl -sS --max-time "$TIMEOUT" -A Nunya \
    --resolve "$name:443:$address" "https://$name$path"
done

echo
echo "== block lists (blocklists.rs) =="
timed "raw.githubusercontent.com" curl -sS --max-time "$TIMEOUT" -o /dev/null -w "%{http_code} %{size_download} bytes" \
  https://raw.githubusercontent.com/SagerNet/sing-geosite/rule-set/geosite-category-ads-all.srs
