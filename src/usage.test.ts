/**
 * Usage history and config identity: the arithmetic that decides whose traffic is whose.
 *
 * Run with `npm test`, which is Node's own runner — no dependency. Node strips the types itself,
 * so imports name the `.ts` file; for the same reason this file is left out of `tsc`, which would
 * want Node's type definitions to check it.
 */

// Before anything reads a date: a zone with a daylight-saving change, for the test that crosses one.
process.env.TZ = "Europe/Berlin";

import assert from "node:assert/strict";
import { test } from "node:test";

import { configKey, matchExisting } from "./identity.ts";
import type { Profile } from "./share.ts";
import { addUsage, advance, dayKey, firstDay, lastDays, niceCeiling, total, type Usage } from "./usage.ts";

const MB = 1024 ** 2;
const GB = 1024 ** 3;

/** Noon on a local calendar day, month counted from 1. */
const at = (y: number, m: number, d: number, hour = 12, minute = 0) => new Date(y, m - 1, d, hour, minute).getTime();

test("a day key is the local calendar day, zero-padded", () => {
  assert.equal(dayKey(at(2026, 9, 1)), "2026-09-01");
  // 00:30 local is still that day locally, whatever the UTC date says.
  assert.equal(dayKey(at(2026, 9, 21, 0, 30)), "2026-09-21");
});

test("what moved between two readings is their difference", () => {
  assert.deepEqual(advance({ uplink: 100, downlink: 1000 }, { uplink: 150, downlink: 4000 }), { up: 50, down: 3000 });
});

test("the first reading after connecting counts in full", () => {
  assert.deepEqual(advance({ uplink: 0, downlink: 0 }, { uplink: 70, downlink: 900 }), { up: 70, down: 900 });
});

test("counters that went backwards mean the core restarted, and the new reading is all new", () => {
  assert.deepEqual(advance({ uplink: 5000, downlink: 90000 }, { uplink: 20, downlink: 300 }), { up: 20, down: 300 });
});

test("traffic on one day adds up, and another day gets its own entry", () => {
  let usage = addUsage(undefined, at(2026, 9, 20), { up: 10, down: 100 });
  usage = addUsage(usage, at(2026, 9, 20, 23, 59), { up: 5, down: 50 });
  usage = addUsage(usage, at(2026, 9, 21, 0, 1), { up: 1, down: 2 });
  assert.deepEqual(usage, { "2026-09-20": { up: 15, down: 150 }, "2026-09-21": { up: 1, down: 2 } });
});

test("nothing moving writes nothing, so an idle session leaves no empty days", () => {
  assert.deepEqual(addUsage(undefined, at(2026, 9, 21), { up: 0, down: 0 }), {});
});

test("totals can be taken over everything or from a day on, across configs", () => {
  const a: Usage = { "2026-08-01": { up: 1, down: 10 }, "2026-09-20": { up: 2, down: 20 } };
  const b: Usage = { "2026-09-21": { up: 3, down: 30 } };
  assert.deepEqual(total([a, b, undefined]), { up: 6, down: 60 });
  assert.deepEqual(total([a, b], "2026-09-01"), { up: 5, down: 50 });
  assert.equal(firstDay([b, a, undefined]), "2026-08-01");
  assert.equal(firstDay([undefined]), null);
});

test("the chart has every day up to today, quiet ones as zero, summed across configs", () => {
  const a: Usage = { "2026-09-19": { up: 1, down: 10 }, "2026-09-21": { up: 2, down: 20 } };
  const b: Usage = { "2026-09-21": { up: 3, down: 30 }, "2026-09-01": { up: 9, down: 90 } };
  assert.deepEqual(lastDays([a, b], 3, at(2026, 9, 21)), [
    { day: "2026-09-19", up: 1, down: 10 },
    { day: "2026-09-20", up: 0, down: 0 },
    { day: "2026-09-21", up: 5, down: 50 },
  ]);
});

/**
 * Europe/Berlin moved its clocks forward on 29 March 2026, a 23-hour day. Stepping back from just
 * after midnight in 24-hour strides lands on the 28th twice and never on the 29th.
 */
test("the chart's days survive a daylight-saving change", () => {
  const days = lastDays([], 3, at(2026, 3, 30, 0, 30)).map((d) => d.day);
  assert.deepEqual(days, ["2026-03-28", "2026-03-29", "2026-03-30"]);
});

test("the chart's scale tops out at a round size", () => {
  // A thousand megabytes is a gigabyte on this scale, and says so.
  assert.equal(niceCeiling(700 * MB), GB);
  assert.equal(niceCeiling(300 * MB), 500 * MB);
  assert.equal(niceCeiling(3.2 * GB), 5 * GB);
  assert.equal(niceCeiling(2 * GB), 2 * GB);
  assert.equal(niceCeiling(0), MB);
});

// ------------------------------------------------------------------------------------ identity

function profile(change: Partial<Profile>): Profile {
  return {
    protocol: "trojan",
    name: "One",
    server: "edge.example.net",
    port: 443,
    uuid: "",
    flow: "",
    security: "",
    alterId: 0,
    password: "first",
    tls: { enabled: true, sni: "edge.example.net", insecure: false, alpn: [], fingerprint: "chrome", reality: null },
    transport: { kind: "ws", path: "/tr/a", host: "", serviceName: "", method: "", maxEarlyData: 0, earlyDataHeader: "" },
    wireguard: null,
    ...change,
  };
}

/** The bug this replaced: every Trojan config on one host and port had the same key. */
test("two Trojan configs on one host and port are different configs", () => {
  assert.notEqual(configKey(profile({ password: "first" })), configKey(profile({ password: "second" })));
});

test("a renamed config with a new random path and SNI case is still the same config", () => {
  const before = profile({});
  const after = profile({
    name: "Renamed",
    transport: { ...before.transport, path: "/tr/zzz" },
    tls: { ...before.tls, sni: "EdGe.Example.NET" },
  });
  assert.equal(configKey(before), configKey(after));
});

test("each existing config is claimed once, so two entries never share one id", () => {
  const existing = [{ id: "a", profile: profile({}) }];
  const matched = matchExisting(existing, [{ profile: profile({}) }, { profile: profile({}) }]);
  assert.equal(matched[0]?.id, "a");
  assert.equal(matched[1], undefined);
});

test("a refresh that reorders the list still finds each config", () => {
  const existing = [
    { id: "a", profile: profile({ password: "first" }) },
    { id: "b", profile: profile({ password: "second" }) },
  ];
  const matched = matchExisting(existing, [{ profile: profile({ password: "second" }) }, { profile: profile({ password: "first" }) }]);
  assert.deepEqual(matched.map((m) => m?.id), ["b", "a"]);
});
