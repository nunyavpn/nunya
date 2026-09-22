/**
 * The rail shield's four states.
 *
 * Run with `npm test`; see `usage.test.ts` for why imports name the `.ts` file.
 */

import assert from "node:assert/strict";
import { test } from "node:test";

import { shieldState, type ShieldInput } from "./shield.ts";

const input = (change: Partial<ShieldInput>): ShieldInput => ({
  connection: "off",
  mode: "vpn",
  fault: null,
  exit: null,
  ...change,
});

test("off is grey with a slash", () => {
  assert.deepEqual(shieldState(input({})), { tone: "off", glyph: "shield-off", label: "Not connected" });
});

test("connecting is its own state", () => {
  assert.equal(shieldState(input({ connection: "connecting" })).tone, "connecting");
});

test("connected with a public exit is green and says where it exits", () => {
  const shield = shieldState(input({ connection: "on", exit: { ipv4: "203.0.113.7", ipv6: null } }));
  assert.equal(shield.tone, "on");
  assert.equal(shield.glyph, "shield-check");
  assert.match(shield.label, /203\.0\.113\.7/);
});

/** Up, and checking — not yet known to be broken, so not red. */
test("connected while the exit is still being found is green", () => {
  assert.equal(shieldState(input({ connection: "on", exit: null })).tone, "on");
});

test("connected with no public exit is not working", () => {
  const shield = shieldState(input({ connection: "on", exit: { ipv4: null, ipv6: null, failed: "timed out" } }));
  assert.equal(shield.tone, "failed");
  assert.equal(shield.glyph, "shield-alert");
});

test("a failed attempt stays red while off", () => {
  assert.equal(shieldState(input({ fault: "the core refused the config" })).tone, "failed");
});

/** A new attempt replaces the old failure: the user is watching this one now. */
test("connecting again is not red", () => {
  assert.equal(shieldState(input({ connection: "connecting", fault: "earlier failure" })).tone, "connecting");
});

/** Stopping takes seconds too; the shield says it is under way rather than still green. */
test("disconnecting is amber and says so", () => {
  const shield = shieldState(input({ connection: "disconnecting" }));
  assert.equal(shield.tone, "connecting");
  assert.equal(shield.label, "Disconnecting…");
});

/** CLAUDE.md, Modes: only VPN mode speaks for the whole device. */
test("proxy mode never claims the device", () => {
  const label = shieldState(input({ connection: "on", mode: "proxy", exit: { ipv4: "203.0.113.7", ipv6: null } })).label;
  assert.match(label, /^Proxy on: apps set to use it/);
  assert.doesNotMatch(label, /device|protected/i);
});
