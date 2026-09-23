/**
 * An anycast entry is placed where it was observed to answer, and nowhere else.
 *
 * Run with `npm test`; see `usage.test.ts` for why imports name the `.ts` file.
 */

import assert from "node:assert/strict";
import { test } from "node:test";

import { entryCountry, entryLine, entryPlace, isAnycast, type Edge, type EdgeReport } from "./edge.ts";
import type { Spot } from "./store.ts";

/** What GeoIP says of the anycast address from the issue: Los Angeles. */
const anycast: Spot = {
  ip: "104.21.77.84",
  country: "US",
  city: "Los Angeles",
  lat: 34.05,
  lon: -118.24,
  asn: 13335,
  org: "Cloudflare, Inc.",
  cdn: "cloudflare",
  checkedAt: 0,
};

/** A rented server: its GeoIP place is where it is. */
const direct: Spot = {
  ip: "67.220.82.10",
  country: "DE",
  city: "Frankfurt am Main",
  lat: 50.11,
  lon: 8.68,
  asn: 63023,
  org: "GTHost",
  cdn: null,
  checkedAt: 0,
};

const FRA = { iata: "FRA", city: "Frankfurt-am-Main", country: "DE", region: "Europe", lat: 50.03, lon: 8.54 };
const EWR = { iata: "EWR", city: "Newark", country: "US", region: "North America", lat: 40.69, lon: -74.17 };

function report(edge: Edge, ip = anycast.ip, publicIp = "178.252.132.98"): EdgeReport {
  return { ip, host: "network.alinaderiparizi.com", provider: "Cloudflare", asn: 13335, edge, sourceNetwork: { local: null, public: publicIp } };
}

const seen = (place: typeof FRA | null, colo = place?.iata ?? "QQQ"): Edge => ({
  status: "observed",
  colo,
  place,
  source: "cf-ray",
  rttMs: 40,
  observedAt: 0,
});

test("an observed edge is the entry's place, not GeoIP's city", () => {
  const line = entryLine(anycast, report(seen(FRA)));
  assert.equal(line.city, "Frankfurt-am-Main");
  assert.equal(line.country, "DE");
  assert.equal(line.org, "Cloudflare edge FRA");
  assert.deepEqual(entryPlace(anycast, report(seen(FRA))), { lat: 50.03, lon: 8.54, city: "Frankfurt-am-Main", country: "DE" });
  assert.equal(entryCountry(anycast, report(seen(FRA))), "DE");
});

test("one address observed from two networks is placed by whichever observation is given", () => {
  assert.equal(entryLine(anycast, report(seen(EWR), anycast.ip, "187.14.56.70")).city, "Newark");
  assert.equal(entryLine(anycast, report(seen(FRA), anycast.ip, "178.252.132.98")).city, "Frankfurt-am-Main");
});

test("an anycast entry not observed has no place at all, never GeoIP's", () => {
  const unknowns: (EdgeReport | undefined)[] = [
    undefined,
    report({ status: "noHostname" }),
    report({ status: "notObservable", reason: "answered 200 with no CF-Ray header and no trace" }),
    report({ status: "tlsFailed", reason: "invalid peer certificate" }),
    report({ status: "timeout" }),
    report({ status: "probeFailed", reason: "refused" }),
  ];
  for (const r of unknowns) {
    const line = entryLine(anycast, r);
    assert.equal(line.city, null, r?.edge.status);
    assert.equal(line.country, null, r?.edge.status);
    assert.match(line.org ?? "", /^Cloudflare anycast · /);
    assert.equal(entryPlace(anycast, r), null);
    assert.equal(entryCountry(anycast, r), null);
  }
});

test("a colo missing from Cloudflare's list is named by its code, with no city put in its place", () => {
  const line = entryLine(anycast, report(seen(null, "QQQ")));
  assert.equal(line.org, "Cloudflare edge QQQ");
  assert.equal(line.city, null);
  assert.equal(line.country, null);
  assert.equal(entryPlace(anycast, report(seen(null, "QQQ"))), null);
});

test("an observation about another address is not taken for this one", () => {
  const stale = report(seen(FRA), "104.21.1.1");
  assert.equal(entryLine(anycast, stale).city, null);
  assert.equal(entryPlace(anycast, stale), null);
});

test("an entry that is a real machine keeps its GeoIP place", () => {
  assert.equal(isAnycast(direct), false);
  assert.equal(entryLine(direct), direct);
  assert.deepEqual(entryPlace(direct), { lat: 50.11, lon: 8.68, city: "Frankfurt am Main", country: "DE" });
  assert.equal(entryCountry(direct), "DE");
});
