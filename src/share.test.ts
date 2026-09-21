/**
 * wg-quick configs: writing one the official WireGuard apps can scan, and reading one back.
 *
 * Run with `npm test`; see `usage.test.ts` for why imports name the `.ts` file.
 */

import assert from "node:assert/strict";
import { test } from "node:test";

import {
  dnsAddressOf,
  extractWgQuick,
  parseShareLink,
  parseWgQuick,
  toShareLink,
  toWgQuick,
  wgQuickRefusal,
  type Profile,
} from "./share.ts";

/** A plain WireGuard server, parsed from the link form the app already reads. */
const plain = (): Profile =>
  parseShareLink(
    "wireguard://yAnz5TF%2BlXXJte14tji3zlMNq%2BhdyA64XPuR2YmHMWg%3D@vpn.example.net:51820" +
      "?publickey=xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg%3D&address=10.0.0.2%2F32%2Cfd00%3A%3A2%2F128" +
      "&mtu=1420&keepalive=25#Home%20server",
  );

test("a WireGuard server is written as the config the official apps scan", () => {
  assert.equal(
    toWgQuick(plain(), ["1.1.1.1"]),
    [
      "# Name = Home server",
      "[Interface]",
      "PrivateKey = yAnz5TF+lXXJte14tji3zlMNq+hdyA64XPuR2YmHMWg=",
      "Address = 10.0.0.2/32, fd00::2/128",
      "DNS = 1.1.1.1",
      "MTU = 1420",
      "",
      "[Peer]",
      "PublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=",
      "AllowedIPs = 0.0.0.0/0, ::/0",
      "Endpoint = vpn.example.net:51820",
      "PersistentKeepalive = 25",
      "",
    ].join("\n"),
  );
});

/** The contract, as for links: what is shared from here imports back as the same server. */
test("a config written here reads back as the same profile", () => {
  const profile = plain();
  assert.deepEqual(parseWgQuick(toWgQuick(profile)), profile);
});

test("an IPv6 endpoint is bracketed and read back", () => {
  const profile = { ...plain(), server: "2001:db8::7" };
  const config = toWgQuick(profile);
  assert.match(config, /^Endpoint = \[2001:db8::7\]:51820$/m);
  assert.equal(parseWgQuick(config).server, "2001:db8::7");
});

/** CLAUDE.md: WARP's reserved client id must survive, and wg-quick cannot carry it. */
test("a WARP config is refused by name, not written without its client id", () => {
  const warp = parseShareLink(
    "wireguard://c2VjcmV0@engage.cloudflareclient.com:2408?publickey=bmXOC%2BF1FxEMF9dyiK2H5%2F1SUtzH0JuVo51h2wPfgyo%3D" +
      "&address=172.16.0.2%2F32&reserved=216%2C253%2C3#Warp",
  );
  assert.match(wgQuickRefusal(warp) ?? "", /WARP/);
  assert.throws(() => toWgQuick(warp), /WARP/);
  assert.equal(wgQuickRefusal(plain()), null);
});

test("a config exported by the official apps imports", () => {
  const profile = parseShareLink(`[Interface]
PrivateKey = yAnz5TF+lXXJte14tji3zlMNq+hdyA64XPuR2YmHMWg=
ListenPort = 21841
Address = 10.0.0.2/32
Address = fd00::2/128
DNS = 10.0.0.1

[Peer]
# the office
PublicKey = xTIBA5rboUvnH4htodjb6e697QjLERt1NAB4mZqp8Dg=
AllowedIPs = 10.0.0.0/24
Endpoint = [2001:db8::1]:51820
`);
  assert.equal(profile.protocol, "wireguard");
  assert.equal(profile.server, "2001:db8::1");
  assert.equal(profile.port, 51820);
  assert.deepEqual(profile.wireguard?.localAddress, ["10.0.0.2/32", "fd00::2/128"]);
  // No name comment: named after the server, as a link without a fragment is.
  assert.equal(profile.name, "2001:db8::1");
});

test("keys are read case-insensitively, as wg-quick reads them", () => {
  const profile = parseWgQuick(
    "[interface]\nprivatekey = k\naddress = 10.0.0.2/32\n[peer]\npublickey = p\nendpoint = vpn.example.net:51820\n",
  );
  assert.equal(profile.wireguard?.privateKey, "k");
});

/** Dropping either would give a tunnel that comes up and carries nothing. */
test("a preshared key or AmneziaWG obfuscation is refused by name", () => {
  const base = "[Interface]\nPrivateKey = k\nAddress = 10.0.0.2/32\n";
  const peer = "[Peer]\nPublicKey = p\nEndpoint = vpn.example.net:51820\n";
  assert.throws(() => parseWgQuick(`${base}${peer}PresharedKey = s\n`), /preshared key/);
  assert.throws(() => parseWgQuick(`${base}Jc = 4\nJmin = 40\n${peer}`), /AmneziaWG/);
});

test("a config with more than one peer is refused", () => {
  const peer = "[Peer]\nPublicKey = p\nEndpoint = vpn.example.net:51820\n";
  assert.throws(() => parseWgQuick(`[Interface]\nPrivateKey = k\nAddress = 10.0.0.2/32\n${peer}${peer}`), /more than one peer/);
});

test("a config missing what a connection needs says which part", () => {
  assert.throws(() => parseWgQuick("[Interface]\nAddress = 10.0.0.2/32\n[Peer]\nPublicKey = p\nEndpoint = a:1\n"), /PrivateKey/);
  assert.throws(() => parseWgQuick("[Interface]\nPrivateKey = k\nAddress = 10.0.0.2/32\n[Peer]\nPublicKey = p\n"), /Endpoint/);
});

/** The paste box reads links a word at a time; a config has to come out whole first. */
test("a config pasted among links is lifted out whole, its name comment with it", () => {
  const config = toWgQuick(plain());
  const { configs, rest } = extractWgQuick(`vless://a@b.example.net:443#One\n${config}\ntrojan://c@d.example.net:443#Two\n`);
  assert.equal(configs.length, 1);
  assert.deepEqual(parseWgQuick(configs[0]), plain());
  assert.deepEqual(rest.split(/\s+/).filter(Boolean), ["vless://a@b.example.net:443#One", "trojan://c@d.example.net:443#Two"]);
});

test("links alone are left alone", () => {
  assert.deepEqual(extractWgQuick("vless://a@b.example.net:443\n"), { configs: [], rest: "vless://a@b.example.net:443\n" });
});

test("the DNS line comes from the app's setting when it names an address", () => {
  assert.equal(dnsAddressOf("https://1.1.1.1/dns-query"), "1.1.1.1");
  assert.equal(dnsAddressOf("tls://8.8.8.8"), "8.8.8.8");
  assert.equal(dnsAddressOf("9.9.9.9"), "9.9.9.9");
  assert.equal(dnsAddressOf("https://[2606:4700:4700::1111]/dns-query"), "2606:4700:4700::1111");
  // A host name would need resolving before anything else could be.
  assert.equal(dnsAddressOf("https://dns.google/dns-query"), null);
  assert.equal(dnsAddressOf("https://999.1.1.1/dns-query"), null);
});

/** The link form is unchanged by any of this. */
test("the wireguard:// link still round-trips", () => {
  assert.deepEqual(parseShareLink(toShareLink(plain())), plain());
});
