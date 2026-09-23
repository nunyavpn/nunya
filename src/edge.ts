/**
 * Where a config's traffic enters, when its address is a Cloudflare anycast edge.
 *
 * Such an address has a GeoIP place — `104.21.77.84` is "Los Angeles" everywhere — and it is not
 * where anything is: the address is answered by whichever Cloudflare data center this network
 * reaches, Newark from one and Frankfurt from another. `cloudflare.rs` asks the edge which one it
 * is (`observe_edge`); this module decides what the views make of the answer, by one rule:
 *
 * **For an anycast entry, the observed edge is the place, and nothing else is.** Observed, the
 * status card names the data center and the map draws the route through it. Not observed — not
 * yet, or the edge would not say — the card says so with no city, and the map draws no entry hop,
 * rather than falling back to GeoIP's city and drawing a detour through California.
 *
 * Any other entry is a real machine, and its GeoIP place stands as before.
 *
 * Imports nothing with a runtime value, so `npm test` covers it (`edge.test.ts`).
 */

import type { Spot } from "./store";
import type { PlaceLine } from "./views/status";

/** A Cloudflare data center, from Cloudflare's own list. Mirrors `cloudflare::Colo`. */
export interface Colo {
  iata: string;
  city: string;
  country: string;
  region?: string | null;
  lat: number;
  lon: number;
}

/** What asking about one edge came to. Mirrors `cloudflare::Edge`. */
export type Edge =
  | { status: "invalidIp"; reason: string }
  | { status: "notCloudflare" }
  | { status: "noHostname" }
  | {
      status: "observed";
      colo: string;
      /** Null for a code Cloudflare's list does not have yet; the code is still the answer. */
      place: Colo | null;
      source: "cf-ray" | "trace";
      /** The TLS handshake with the edge: one round trip. */
      rttMs: number;
      observedAt: number;
    }
  | { status: "notObservable"; reason: string }
  | { status: "tlsFailed"; reason: string }
  | { status: "timeout" }
  | { status: "probeFailed"; reason: string };

/** One observation. Mirrors `cloudflare::Report`. */
export interface EdgeReport {
  ip: string;
  host: string | null;
  provider: string | null;
  asn: number | null;
  edge: Edge;
  sourceNetwork: { local: string | null; public: string | null };
}

type Observed = Extract<Edge, { status: "observed" }>;

/** An entry whose GeoIP place is not a place: an anycast CDN edge. */
export function isAnycast(entry: Spot | null | undefined): boolean {
  return entry?.cdn === "cloudflare";
}

/** The observation, when there is one and it is about this entry's address. */
function observed(entry: Spot, report: EdgeReport | null | undefined): Observed | null {
  if (!report || report.ip !== entry.ip) return null;
  return report.edge.status === "observed" ? report.edge : null;
}

/** Why an anycast entry has no place yet, in the card's few words. */
export function edgeUnknown(report: EdgeReport | null | undefined): string {
  switch (report?.edge.status) {
    case undefined:
      return "edge not checked yet";
    case "noHostname":
      return "no host name to ask the edge with";
    case "notObservable":
      return "the edge did not say which it is";
    case "tlsFailed":
      return "edge check failed (TLS)";
    case "timeout":
      return "the edge did not answer";
    default:
      return "edge not known";
  }
}

/**
 * The status card's Entry line.
 *
 * "Frankfurt-am-Main, DE · Cloudflare edge FRA · AS13335" once observed; "Cloudflare anycast ·
 * edge not checked yet" before — with no city and no flag, because the only city on hand is
 * GeoIP's, which is the wrong one.
 */
export function entryLine(entry: Spot, report?: EdgeReport | null): PlaceLine {
  if (!isAnycast(entry)) return entry;
  const seen = observed(entry, report);
  if (seen) {
    return {
      city: seen.place?.city ?? null,
      country: seen.place?.country ?? null,
      org: `Cloudflare edge ${seen.colo}`,
      asn: entry.asn ?? null,
    };
  }
  return { city: null, country: null, org: `Cloudflare anycast · ${edgeUnknown(report)}`, asn: entry.asn ?? null };
}

/** Where traffic enters, for the map's hop: null when that is not known. */
export function entryPlace(
  entry: Spot | null | undefined,
  report?: EdgeReport | null,
): { lat: number; lon: number; city: string | null; country: string } | null {
  if (!entry) return null;
  if (isAnycast(entry)) {
    const place = observed(entry, report)?.place;
    return place ? { lat: place.lat, lon: place.lon, city: place.city, country: place.country } : null;
  }
  return entry.lat !== null && entry.lon !== null
    ? { lat: entry.lat, lon: entry.lon, city: entry.city, country: entry.country }
    : null;
}

/** The country traffic enters in, for "via XX": null for an anycast edge not observed. */
export function entryCountry(entry: Spot | null | undefined, report?: EdgeReport | null): string | null {
  if (!entry) return null;
  if (isAnycast(entry)) return observed(entry, report)?.place?.country ?? null;
  return entry.country;
}
