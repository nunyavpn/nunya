/** Import/export must keep XHTTP settings, including values that need URL escaping. */
import assert from "node:assert/strict";
import { test } from "node:test";
import { parseShareLink, toShareLink } from "../src/share.ts";

const uuid = "8f3c9d2e-4a17-4b8e-9c21-7de5f0a63b14";
const extra = { headers: { "X-Token": "a&b=سلام" }, xmux: { maxConcurrency: "8-16" }, noSSEHeader: true };
const query = new URLSearchParams({ type: "xhttp", path: "/upload?token=a&b", host: "cdn.example.net",
  mode: "packet-up", security: "tls", sni: "cdn.example.net", extra: JSON.stringify(extra) });

test("XHTTP and SplitHTTP links round-trip through storage and sharing without losing settings", () => {
  for (const protocol of ["vless", "vmess", "trojan"]) {
    for (const type of ["xhttp", "splithttp"]) {
      query.set("type", type);
      const p = parseShareLink(`${protocol}://${uuid}@example.net:443?${query}#XHTTP`);
      assert.equal(p.transport.kind, "xhttp");
      assert.deepEqual(p.transport.extra, extra);
      assert.equal(p.transport.path, "/upload?token=a&b");
      assert.deepEqual(parseShareLink(toShareLink(JSON.parse(JSON.stringify(p)))), p);
    }
  }
});

test("VMess JSON carries XHTTP mode and extra too", () => {
  const raw = { add: "example.net", port: 443, id: uuid, net: "xhttp", path: "/x", mode: "stream-up", extra };
  const p = parseShareLink(`vmess://${Buffer.from(JSON.stringify(raw)).toString("base64")}`);
  assert.equal(p.transport.mode, "stream-up");
  assert.deepEqual(p.transport.extra, extra);
});

test("invalid modes, malformed extra and unsupported options are refused by name", () => {
  for (const [suffix, message] of [
    ["mode=made-up", /mode/], ["flow=xtls-rprx-vision", /flow/], ["encryption=unsupported", /encryption/], ["extra=%5B%5D", /JSON object/], ["extra=oops", /JSON object/],
    [`extra=${encodeURIComponent('{"downloadSettings":{}}')}`, /downloadSettings/],
    [`extra=${encodeURIComponent('{"xmux":{"typo":1}}')}`, /typo/],
  ] as const) {
    assert.throws(() => parseShareLink(`vless://${uuid}@example.net:443?type=xhttp&${suffix}`), message);
  }
  assert.throws(() => parseShareLink(`vless://${uuid}@example.net:443?type=kcp`), /mKCP/);
});

// The reported provider shape, with credentials and endpoint replaced by test values.
test("provider links can repeat mode inside extra while preserving padding, Firefox and h2/h3", () => {
  const extra = { mode: "auto", xPaddingBytes: "100-1000" };
  const q = new URLSearchParams({ type: "xhttp", security: "tls", insecure: "0", allowInsecure: "0",
    alpn: "h2,h3", host: "cdn.example.net", path: "/", sni: "cdn.example.net",
    extra: JSON.stringify(extra, null, 2), fp: "firefox", encryption: "none", mode: "auto" });
  const p = parseShareLink(`vless://${uuid}@192.0.2.10:443?${q}#Sample`);
  assert.deepEqual(p.transport.extra, extra);
  assert.equal(p.tls.insecure, false);
  assert.equal(p.tls.fingerprint, "firefox");
  assert.deepEqual(p.tls.alpn, ["h2", "h3"]);
  assert.equal(p.transport.host, "cdn.example.net");
  assert.equal(p.tls.sni, "cdn.example.net");
  assert.deepEqual(parseShareLink(toShareLink(p)), p);
});

test("XHTTP refuses legacy VMess before saving an unusable server", () => {
  assert.throws(() => parseShareLink(`vmess://${uuid}@example.net:443?type=xhttp&alterId=4`), /alterId/);
  const raw = { add: "example.net", port: 443, id: uuid, net: "xhttp", aid: 4 };
  assert.throws(() => parseShareLink(`vmess://${Buffer.from(JSON.stringify(raw)).toString("base64")}`), /alterId/);
});
