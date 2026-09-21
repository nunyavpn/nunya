/**
 * Quick Connect's choice of rows: latest, most used and fastest, each config once.
 *
 * Run with `npm test`; see `usage.test.ts` for why imports name the `.ts` file.
 */

import assert from "node:assert/strict";
import { test } from "node:test";

import { fastestFirst, quickTargets, type Candidate } from "./quick.ts";

const HOUR = 3_600_000;
const GB = 1024 ** 3;

function candidate(name: string, change: Partial<Candidate<string>> = {}): Candidate<string> {
  return { item: name, lastConnectedAt: null, recentBytes: 0, latency: null, ...change };
}

/** The rows as `name: kinds`, which is what a user reads off the card. */
const rows = (candidates: Candidate<string>[]) =>
  quickTargets(candidates).map((t) => `${t.candidate.item}: ${t.kinds.join("+")}`);

test("each of the three picks its own config, in the card's order", () => {
  assert.deepEqual(
    rows([
      candidate("fast", { latency: 24 }),
      candidate("daily", { recentBytes: 30 * GB, latency: 90 }),
      candidate("yesterday", { lastConnectedAt: 1000 * HOUR, latency: 150 }),
    ]),
    ["yesterday: latest", "daily: mostUsed", "fast: fastest"],
  );
});

test("one config that is two of them is one row with both reasons", () => {
  assert.deepEqual(
    rows([
      candidate("home", { lastConnectedAt: 5 * HOUR, recentBytes: 12 * GB, latency: 60 }),
      candidate("quick", { latency: 20 }),
    ]),
    ["home: latest+mostUsed", "quick: fastest"],
  );
});

test("one config that is all three is the whole card", () => {
  assert.deepEqual(rows([candidate("only", { lastConnectedAt: 1, recentBytes: 1, latency: 30 })]), [
    "only: latest+mostUsed+fastest",
  ]);
});

test("latest is the most recent connection, not the first in the list", () => {
  assert.deepEqual(
    rows([candidate("older", { lastConnectedAt: 1 * HOUR }), candidate("newer", { lastConnectedAt: 9 * HOUR })]),
    ["newer: latest"],
  );
});

/** A new install has no history and nothing tested; the card says so rather than offering nothing. */
test("a target with nothing behind it is left out", () => {
  assert.deepEqual(rows([candidate("new"), candidate("untested")]), []);
  assert.deepEqual(rows([candidate("tested", { latency: 80 })]), ["tested: fastest"]);
});

test("an unreachable or untested config is never the fastest", () => {
  assert.deepEqual(
    fastestFirst([
      candidate("dead", { latency: -1 }),
      candidate("untested"),
      candidate("slow", { latency: 300 }),
      candidate("quick", { latency: 40 }),
    ]).map((c) => c.item),
    ["quick", "slow"],
  );
});

test("traffic of zero does not make a config the most used", () => {
  assert.deepEqual(rows([candidate("idle", { recentBytes: 0, latency: 50 })]), ["idle: fastest"]);
});
