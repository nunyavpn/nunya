/**
 * A fixture that fills the UI with servers, subscriptions and rules.
 *
 * Every screen in this app is a list of things a provider gave you, so an empty store shows almost
 * nothing: no rows, no flags, no pins, no quota bars, and a Quick Connect button that is disabled
 * because nothing has been tested. That makes the interface impossible to work on without a real
 * subscription to point it at.
 *
 * This is opt-in and never reaches a build:
 *
 *     VITE_MOCK=1 npm run tauri dev     the app, with the list full
 *     VITE_MOCK=1 npm run dev           the same UI in a browser, no Rust side at all
 *
 * `import.meta.env.VITE_MOCK` is substituted at build time, so with the flag unset `MOCK_ENABLED`
 * is the literal `false` and the bundler drops this module's contents entirely. That matters for
 * more than size: a shipped VPN client carrying a list of plausible-looking server credentials is
 * something a user could mistake for real endpoints.
 *
 * Two deliberate properties of the data itself:
 *
 * - **Nothing here resolves.** Every host is under `example.net`, which RFC 2606 reserves precisely
 *   so it can never be registered. If a mock server is selected and connect is pressed, the dial
 *   fails at DNS rather than reaching a stranger's machine.
 * - **Nothing here is saved.** `persist.ts` pairs this with a backend that discards writes, so a
 *   session spent clicking through mock rows cannot overwrite the real data file — which holds
 *   every credential the user actually has.
 */

import {
  DEFAULT_SETTINGS,
  MANUAL_GROUP_ID,
  type AppData,
  type BypassRule,
  type Group,
  type Server,
} from "./store";
import type { Profile, Protocol, TlsOptions, Transport, TransportKind } from "./share";
import { dayKey, type Usage } from "./usage";

/**
 * Whether the fixture is in use.
 *
 * Compared against a string because Vite substitutes the raw value: the flag arrives as `"1"`, not
 * as a boolean, and `Boolean("0")` is `true`.
 */
export const MOCK_ENABLED = import.meta.env.VITE_MOCK === "1";

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;
const GB = 1024 ** 3;

/** Obviously not a keypair. Spelled out so it cannot be mistaken for one in a screenshot. */
const FAKE_REALITY_KEY = "MOCK0PUBLIC0KEY0NOT0A0REAL0REALITY0KEYPAIR0";

/** Valid UUID shape, visibly counted, so rows stay tellable apart in devtools. */
function fakeUuid(n: number): string {
  return `00000000-0000-4000-8000-${n.toString().padStart(12, "0")}`;
}

type Security = "reality" | "tls" | "none";

function transportFor(kind: TransportKind, host: string): Transport {
  const base: Transport = {
    kind,
    path: "",
    host: "",
    serviceName: "",
    method: "",
    maxEarlyData: 0,
    earlyDataHeader: "",
  };
  switch (kind) {
    case "ws":
      return { ...base, path: "/mock-ws", host, maxEarlyData: 2048, earlyDataHeader: "Sec-WebSocket-Protocol" };
    case "grpc":
      return { ...base, serviceName: "MockService" };
    case "httpupgrade":
      return { ...base, path: "/mock-upgrade", host };
    case "http":
      return { ...base, path: "/mock-h2", host };
    default:
      return base;
  }
}

function tlsFor(kind: Security, sni: string): TlsOptions {
  if (kind === "none") {
    return { enabled: false, sni: "", insecure: false, alpn: [], fingerprint: "", reality: null };
  }
  return {
    enabled: true,
    sni,
    insecure: false,
    // Reality does its own handshake mimicry, so a Reality link carries no ALPN of its own.
    alpn: kind === "reality" ? [] : ["h2", "http/1.1"],
    fingerprint: "chrome",
    reality: kind === "reality" ? { publicKey: FAKE_REALITY_KEY, shortId: "0123abcd" } : null,
  };
}

interface Spec {
  country: string;
  city: string;
  /** The share-link name, in the shape providers actually write them. */
  name: string;
  host: string;
  /** Milliseconds, `-1` for unreachable, `null` for never tested — the three states a row renders. */
  latency: number | null;
  security: Security;
  port?: number;
  retired?: boolean;
  /** Defaults to plain TCP, which is what most links are. */
  transport?: TransportKind;
  /** Defaults to VLESS. */
  protocol?: Protocol;
  /**
   * Where a sweep measured this one actually exits, when that differs from its label.
   *
   * The common case with real subscriptions and impossible to reach by clicking, since it needs
   * a completed test against live servers.
   */
  exitCountry?: string;
  /** The measured exit city, when it is not the labelled one. Defaults to `city`. */
  exitCity?: string;
  /** Its address is a CDN edge, so the row carries a CDN tag. */
  cdn?: "cloudflare" | "fastly";
}

/**
 * Coordinates a sweep would have measured, so the map shows city dots rather than country centres.
 *
 * Only servers that answered a latency test get them, because only those are located: an
 * untested or unreachable server stays at its country's centre, which is the other state to see.
 */
const CITY_AT: Record<string, [number, number]> = {
  Frankfurt: [50.11, 8.68],
  Amsterdam: [52.37, 4.9],
  Tehran: [35.69, 51.42],
  Sydney: [-33.87, 151.21],
  Dubai: [25.2, 55.27],
  Toronto: [43.65, -79.38],
  Ashburn: [39.04, -77.49],
  Helsinki: [60.17, 24.94],
  Stockholm: [59.33, 18.07],
  London: [51.51, -0.13],
  Paris: [48.86, 2.35],
  Zurich: [47.37, 8.54],
  "New York": [40.71, -74.01],
  "Los Angeles": [34.05, -118.24],
  Tokyo: [35.68, 139.69],
  Singapore: [1.35, 103.82],
  "Hong Kong": [22.32, 114.17],
  Istanbul: [41.01, 28.98],
};

let uuidCounter = 0;

function serverFrom(groupId: string, spec: Spec): Server {
  uuidCounter += 1;
  const host = `${spec.host}.example.net`;
  const protocol = spec.protocol ?? "vless";
  const profile: Profile = {
    protocol,
    name: spec.name,
    server: host,
    port: spec.port ?? 443,
    uuid: fakeUuid(uuidCounter),
    // Vision is a VLESS flow and needs the TLS-like handshake Reality provides.
    flow: protocol === "vless" && spec.security === "reality" ? "xtls-rprx-vision" : "",
    security: protocol === "vmess" ? "auto" : "",
    alterId: 0,
    tls: tlsFor(spec.security, host),
    transport: transportFor(spec.transport ?? "tcp", host),
    password: "",
    wireguard: null,
  };

  // Every server has an entry — it needs only DNS — while only one that answered a test has an
  // exit. Addresses are from RFC 5737's documentation ranges, so none of them is anyone's.
  // Hosting names are placeholders; the CDN ones are the real networks, since what a CDN entry
  // looks like on the card is the thing worth seeing.
  const spot = (ip: string, country: string, city: string, org = "Example Hosting", asn = 64500) => {
    const at = CITY_AT[city];
    return {
      ip,
      country,
      city,
      lat: at?.[0] ?? null,
      lon: at?.[1] ?? null,
      org,
      asn,
      checkedAt: Date.now() - 6 * MINUTE,
    };
  };
  const cdnNetwork: Record<"cloudflare" | "fastly", [string, number]> = {
    cloudflare: ["Cloudflare, Inc.", 13335],
    fastly: ["Fastly, Inc.", 54113],
  };
  // Entry and exit on different documentation ranges (RFC 5737), because they are different
  // machines whenever a server relays — the case the card has to show.
  const entryIp = `192.0.2.${uuidCounter}`;
  const exitIp = spec.exitCountry || spec.cdn ? `198.51.100.${uuidCounter}` : entryIp;
  const answered = spec.latency !== null && spec.latency > 0;

  return {
    entry: spec.cdn
      ? { ...spot(entryIp, spec.country, spec.city, ...cdnNetwork[spec.cdn]), cdn: spec.cdn }
      : spot(entryIp, spec.country, spec.city),
    exit: answered
      ? spot(exitIp, spec.exitCountry ?? spec.country, spec.exitCity ?? spec.city)
      : undefined,
    // Readable rather than random: a fixture is easier to reason about when the ids mean something.
    id: `${groupId}-${spec.host}`,
    groupId,
    profile,
    country: spec.country,
    city: spec.city,
    latency: spec.latency,
    // An untested server has never been tested; anything else was, a few minutes ago.
    testedAt: spec.latency === null ? null : Date.now() - 6 * MINUTE,
    ...(spec.retired ? { retired: true } : {}),
  };
}

const AURORA_ID = "mock-aurora";
const BACKUP_ID = "mock-backup";

/**
 * The hand-added group, the one the store guarantees always exists.
 *
 * Kept small, because that is what it looks like in practice: a couple of links someone pasted.
 */
const MANUAL: Spec[] = [
  { country: "DE", city: "Frankfurt", name: "DE-1 Frankfurt", host: "fra-01", latency: 24, security: "reality" },
  // Labelled Amsterdam, measured exiting in the United States — the ordinary case with a
  // Cloudflare-fronted provider, and the one that proves a sweep changes the flag and not the name.
  { country: "NL", city: "Amsterdam", name: "NL-2 Amsterdam", host: "ams-02", latency: 138, security: "tls", transport: "ws", exitCountry: "US", exitCity: "Ashburn", cdn: "cloudflare" },
  { country: "IR", city: "Tehran", name: "IR-1 Tehran", host: "thr-01", latency: null, security: "none", port: 8080 },
];

/**
 * A healthy subscription, spread across the latency grades on purpose.
 *
 * `format.ts` grades under 100ms good, under 300 mid, and anything above bad; `-1` is unreachable
 * and `null` untested. All five appear here, so the colours and the signal bars can be checked
 * against each other in one screenful rather than by editing numbers by hand.
 */
const AURORA: Spec[] = [
  { country: "FI", city: "Helsinki", name: "FI-1 Helsinki", host: "hel-01", latency: 31, security: "reality" },
  { country: "SE", city: "Stockholm", name: "SE-1 Stockholm", host: "sto-01", latency: 44, security: "reality" },
  { country: "GB", city: "London", name: "GB-3 London", host: "lon-03", latency: 71, security: "tls", transport: "ws", cdn: "cloudflare" },
  { country: "FR", city: "Paris", name: "FR-2 Paris", host: "par-02", latency: 96, security: "tls", transport: "grpc", protocol: "vmess" },
  { country: "CH", city: "Zurich", name: "CH-1 Zurich", host: "zrh-01", latency: 112, security: "reality" },
  { country: "US", city: "New York", name: "US-5 New York", host: "nyc-05", latency: 184, security: "reality" },
  // A second New York exit, so a map dot holds more than one server and opens the picker.
  { country: "US", city: "New York", name: "US-6 New York", host: "nyc-06", latency: 142, security: "tls", transport: "ws" },
  { country: "US", city: "Los Angeles", name: "US-9 Los Angeles", host: "lax-09", latency: 212, security: "tls", transport: "httpupgrade", protocol: "vmess", cdn: "fastly" },
  { country: "JP", city: "Tokyo", name: "JP-2 Tokyo", host: "nrt-02", latency: 288, security: "reality" },
  { country: "SG", city: "Singapore", name: "SG-1 Singapore", host: "sin-01", latency: 341, security: "reality" },
  { country: "AU", city: "Sydney", name: "AU-1 Sydney", host: "syd-01", latency: -1, security: "reality" },
  // The one state that cannot be reached by clicking: a server a refresh dropped, kept because the
  // tunnel is running on it. It renders dimmed, with "retired" appended to the subtitle.
  { country: "HK", city: "Hong Kong", name: "HK-4 Hong Kong", host: "hkg-04", latency: 203, security: "tls", retired: true },
];

/** A subscription in the two states the group header treats specially: failed, and nearly spent. */
const BACKUP: Spec[] = [
  { country: "TR", city: "Istanbul", name: "TR-1 Istanbul", host: "ist-01", latency: 156, security: "tls", transport: "ws", protocol: "vmess" },
  { country: "AE", city: "Dubai", name: "AE-2 Dubai", host: "dxb-02", latency: -1, security: "reality" },
  { country: "CA", city: "Toronto", name: "CA-1 Toronto", host: "yyz-01", latency: null, security: "reality" },
];

function groups(now: number): Group[] {
  return [
    {
      id: MANUAL_GROUP_ID,
      kind: "manual",
      name: "Personal",
      url: null,
      updatedAt: null,
      lastError: null,
      refreshing: false,
      quota: null,
      collapsed: false,
    },
    {
      id: AURORA_ID,
      kind: "subscription",
      name: "Aurora Networks",
      url: "https://sub.example.net/aurora/nunya",
      updatedAt: now - 2 * HOUR,
      lastError: null,
      refreshing: false,
      // Comfortably inside the allowance, so the bar renders in its normal colour.
      quota: { usedBytes: Math.round(0.62 * 500 * GB), totalBytes: 500 * GB, resetsAt: now + 18 * DAY },
      collapsed: false,
    },
    {
      id: BACKUP_ID,
      kind: "subscription",
      name: "Backup Provider",
      url: "https://sub.example.net/backup/nunya",
      updatedAt: now - 9 * DAY,
      // Puts the group's meta line into its error styling instead of a timestamp.
      lastError: "The subscription host did not respond.",
      refreshing: false,
      // Past 85%, which is where the quota bar turns amber.
      quota: { usedBytes: Math.round(0.91 * 100 * GB), totalBytes: 100 * GB, resetsAt: now + 3 * DAY },
      // Starts folded, so the collapsed header is visible without clicking one shut first.
      collapsed: true,
    },
  ];
}

/**
 * Usage history, so the usage sheet has bars to draw.
 *
 * Deterministic rather than random, for the same reason the ids are readable: a chart that changes
 * on every reload cannot be compared with the last one. `days` is how far back it goes, `mb` a
 * typical day's download; a share of days are quiet, as real use is. The Backup group has none,
 * so its sheet shows the empty state.
 */
/*
 * Keyed by host, not id: `store.ts` imports this module, so a top-level constant built from its
 * `MANUAL_GROUP_ID` would read it before it exists.
 */
const USAGE: Record<string, { days: number; mb: number }> = {
  "hel-01": { days: 45, mb: 900 },
  "sto-01": { days: 20, mb: 300 },
  "lon-03": { days: 9, mb: 120 },
  "hkg-04": { days: 30, mb: 60 },
  "fra-01": { days: 14, mb: 450 },
};

/**
 * When the tunnel last ran on a config, by host, in hours ago: Quick Connect's "Most recent". One
 * config, and not the fastest or the most used, so the prompt's three choices name three configs.
 */
const LAST_CONNECTED: Record<string, number> = { "sto-01": 2 };

function usageFor(spec: { days: number; mb: number }, now: number): Usage {
  const usage: Usage = {};
  const today = new Date(now);
  for (let back = 0; back < spec.days; back++) {
    const wave = (Math.sin(back * 1.7 + spec.mb) + 1) / 2;
    if (wave < 0.25) continue;
    const down = Math.round(spec.mb * (0.3 + wave) * 1024 * 1024);
    const day = new Date(today.getFullYear(), today.getMonth(), today.getDate() - back, 12);
    usage[dayKey(day.getTime())] = { up: Math.round(down * 0.07), down };
  }
  return usage;
}

const BYPASS: BypassRule[] = [
  { id: "mock-bypass-1", kind: "domain", value: "*.example.net" },
  { id: "mock-bypass-2", kind: "domain", value: "intranet.example.org" },
  { id: "mock-bypass-3", kind: "address", value: "192.168.10.1" },
  { id: "mock-bypass-4", kind: "range", value: "10.0.0.0/8" },
];

/**
 * Builds the fixture.
 *
 * A function rather than a constant because the timestamps are relative: a subscription "updated 2
 * hours ago" has to stay 2 hours ago across reloads, not drift into last week because the module
 * happened to be evaluated when the tab first opened.
 */
export function mockData(): AppData {
  uuidCounter = 0;
  const now = Date.now();

  return {
    groups: groups(now),
    servers: [
      ...MANUAL.map((s) => serverFrom(MANUAL_GROUP_ID, s)),
      ...AURORA.map((s) => serverFrom(AURORA_ID, s)),
      ...BACKUP.map((s) => serverFrom(BACKUP_ID, s)),
    ].map((server) => {
      const host = server.id.slice(server.groupId.length + 1);
      const plan = USAGE[host];
      const hours = LAST_CONNECTED[host];
      return {
        ...server,
        ...(plan ? { usage: usageFor(plan, now) } : {}),
        ...(hours !== undefined ? { lastConnectedAt: now - hours * HOUR } : {}),
      };
    }),
    bypass: BYPASS,
    settings: { ...DEFAULT_SETTINGS },
    // Selected inside a subscription rather than the manual group, so the active row and the
    // collapsed-group behaviour are both visible on first paint.
    selectedServerId: `${AURORA_ID}-hel-01`,
  };
}
