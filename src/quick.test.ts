/**
 * Quick Connect's three choices: fastest, most used and most recent.
 *
 * Run with `npm test`; see `usage.test.ts` for why imports name the `.ts` file.
 */

import assert from "node:assert/strict";
import { test } from "node:test";

import { fastestFirst, quickPicks, type Candidate } from "./quick.ts";

const HOUR = 3_600_000;
const GB = 1024 ** 3;

function candidate(name: string, change: Partial<Candidate<string>> = {}): Candidate<string> {
  return { item: name, lastConnectedAt: null, recentBytes: 0, latency: null, ...change };
}

/** What each option of the prompt would connect to. */
function picks(candidates: Candidate<string>[]) {
  const found = quickPicks(candidates);
  return { fastest: found.fastest?.item, mostUsed: found.mostUsed?.item, recent: found.recent?.item };
}

test("each choice picks by its own measure", () => {
  assert.deepEqual(
    picks([
      candidate("fast", { latency: 24 }),
      candidate("daily", { recentBytes: 30 * GB, latency: 90 }),
      candidate("yesterday", { lastConnectedAt: 1000 * HOUR, latency: 150 }),
    ]),
    { fastest: "fast", mostUsed: "daily", recent: "yesterday" },
  );
});

/** A choice of criterion: every option says what it would connect to, even when they agree. */
test("one config can be the answer to all three", () => {
  assert.deepEqual(picks([candidate("only", { lastConnectedAt: 1, recentBytes: 1, latency: 30 })]), {
    fastest: "only",
    mostUsed: "only",
    recent: "only",
  });
});

test("most recent is the newest connection, not the first in the list", () => {
  assert.equal(
    picks([candidate("older", { lastConnectedAt: 1 * HOUR }), candidate("newer", { lastConnectedAt: 9 * HOUR })]).recent,
    "newer",
  );
});

/** A new install has no history and nothing tested, and the prompt says why for each. */
test("a choice with nothing behind it has no answer", () => {
  assert.deepEqual(picks([candidate("new"), candidate("untested")]), {
    fastest: undefined,
    mostUsed: undefined,
    recent: undefined,
  });
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
  assert.equal(picks([candidate("idle", { recentBytes: 0, latency: 50 })]).mostUsed, undefined);
});
