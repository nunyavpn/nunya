/**
 * The ad blocker's and the anti-tracker's switches: when their lists are fetched, and what the
 * switches say about them.
 *
 * Run with `npm test`; see `usage.test.ts` for why imports name the `.ts` file.
 */

import assert from "node:assert/strict";
import { test } from "node:test";

import { listLine, listsToFetch, STALE_MS, type ListState } from "./blocking.ts";

const NOW = 1_800_000_000_000;
const none: ListState = { updatedAt: null, busy: false, error: null };
const fresh: ListState = { updatedAt: NOW - 60 * 60 * 1000, busy: false, error: null };
const stale: ListState = { updatedAt: NOW - STALE_MS - 1, busy: false, error: null };

test("a switched-on list that is missing or stale is fetched, and nothing else is", () => {
  const on = { blockAds: true, blockTrackers: true };
  assert.deepEqual(listsToFetch(on, { ads: none, trackers: fresh }, NOW), ["ads"]);
  assert.deepEqual(listsToFetch(on, { ads: fresh, trackers: stale }, NOW), ["trackers"]);
  assert.deepEqual(listsToFetch({ blockAds: false, blockTrackers: false }, { ads: none, trackers: stale }, NOW), []);
});

test("a list already being fetched is not fetched twice", () => {
  const on = { blockAds: true, blockTrackers: false };
  assert.deepEqual(listsToFetch(on, { ads: { ...none, busy: true }, trackers: none }, NOW), []);
});

test("a switch that is on with no list says it is not blocking yet", () => {
  assert.match(listLine(true, none, NOW), /^not blocking yet/);
  assert.match(listLine(true, { ...none, error: "timed out" }, NOW), /not blocking yet.*timed out.*through the tunnel/);
});

test("a list on disk says how old it is, and a failed refresh does not hide that it still blocks", () => {
  assert.equal(listLine(true, fresh, NOW), "list updated 1 h ago");
  assert.match(listLine(true, { ...stale, error: "403" }, NOW), /^list updated .*; the last update failed \(403\)$/);
});
