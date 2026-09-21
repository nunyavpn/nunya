import { inTauri, invoke, listen } from "./bridge";

import { h, qs, render } from "./dom";
import { guessCity, guessCountry, place } from "./geo";
import {
  isRelayed,
  located,
  MANUAL_GROUP_ID,
  store,
  type Group,
  type Server,
  type Spot,
} from "./store";
import { describe, parseShareLink, toShareLink, type Profile } from "./share";
import { icon } from "./views/icons";
import { BypassPanel } from "./views/bypass";
import { DiagnosticsPanel } from "./views/diagnostics";
import { LocationsPanel } from "./views/locations";
import { WorldMap, type Hop, type Pin, type PickPoint } from "./views/map";
import { MapPicker } from "./views/mappick";
import { SettingsPanel } from "./views/settings";
import { StatusCard, type ConnectionState, type PlaceLine } from "./views/status";
import { ProfileEditor } from "./views/editor";
import { qrCode, readQrCode } from "./views/qr";

/** Mirrors the Rust `Readiness` struct. */
interface Readiness {
  mode: string;
  transport: string;
  ready: boolean;
  state: "disconnected" | "connecting" | "connected" | "needsPermission";
  detail: string | null;
}

interface Throughput {
  uplink: number;
  downlink: number;
}

// ---------------------------------------------------------------- app state

let connection: ConnectionState = "off";
let connectedAt = 0;
let readiness: Readiness | null = null;
let coreReady = false;
/** The running tunnel's public addresses, per family, once measured; null while checking. */
let exitIps: ExitIps | null = null;
/** Where this machine is, looked up with the tunnel down. */
let home: Whereabouts | null = null;
/** Where the running tunnel comes out, once measured. */
let exitPlace: Whereabouts | null = null;
let tunnelDevice: string | null = null;

/** Cumulative counters from the previous poll, so the UI can show a rate rather than a total. */
let lastCounters = { uplink: 0, downlink: 0, at: 0 };
let shownRate = { uplink: 0, downlink: 0 };

const logLines: string[] = [];
const MAX_LOG_LINES = 500;

// ---------------------------------------------------------------- views

const status = new StatusCard(qs("#status"), {
  onToggle: () => void toggleConnection(),
  onShare: () => {
    const server = store.selected();
    if (server) openShareServer(server);
  },
});
const map = new WorldMap(qs<HTMLCanvasElement>("#worldmap"), qs("#pins"), (key, at) =>
  pickFromMap(key, at),
);
const picker = new MapPicker(qs(".mappane"), (server) => selectServer(server));
// A card placed beside a dot is wrong the moment the dot moves.
map.onViewChange = () => picker.close();

const panelHost = qs<HTMLElement>("#panel");
const bypass = new BypassPanel(panelHost);
const settings = new SettingsPanel(panelHost, { onDisconnect: () => void disconnect() });
const diagnostics = new DiagnosticsPanel(panelHost, {
  onClear: () => {
    logLines.length = 0;
    diagnostics.render();
  },
  onPreviewConfig: () => invoke<string>("preview_config", buildRequest()),
});

/** Which panel the rail is showing. `vpn` means the locations list, which is the default. */
type Screen = "vpn" | "rules" | "settings" | "diagnostics";
let screen: Screen = "vpn";

const RAIL: Record<Screen, string> = {
  vpn: "#nav-vpn",
  rules: "#nav-rules",
  settings: "#nav-settings",
  diagnostics: "#nav-diagnostics",
};

function show(next: Screen) {
  // Pressing the active button returns to the list, so the map is never more than one click away.
  screen = screen === next && next !== "vpn" ? "vpn" : next;

  for (const [name, selector] of Object.entries(RAIL)) {
    qs(selector).classList.toggle("on", name === screen);
  }

  const locationsVisible = screen === "vpn";
  qs<HTMLElement>("#locations").hidden = !locationsVisible;
  panelHost.hidden = locationsVisible;

  bypass.active = screen === "rules";
  settings.active = screen === "settings";
  diagnostics.active = screen === "diagnostics";

  if (screen === "rules") bypass.render();
  else if (screen === "settings") settings.render();
  else if (screen === "diagnostics") diagnostics.render();
}

const locations = new LocationsPanel(qs("#locations"), {
  onSelect: (server) => selectServer(server),
  onRefresh: (group) => void refreshSubscription(group),
  onCheck: (server) => void checkOne(server),
  onShare: (server) => openShareServer(server),
  onEdit: (server) => openEditServer(server),
  onDelete: (server) => confirmDeleteServer(server),
  onRemoveGroup: (group) => confirmDeleteGroup(group),
  onAdd: () => openAddServers(),
  onQuickConnect: () => {
    const fastest = store.fastest();
    if (fastest) {
      store.select(fastest.id);
      void connect();
    }
  },
});

/**
 * Servers checked automatically — on add and after a subscription update — at most. A public list
 * of twenty thousand is not something to start testing unasked: it would run for hours and spend
 * every geo lookup the free tiers allow. Larger additions are checked a row at a time.
 *
 * There is no Test all. A server is measured when it arrives and when its subscription is
 * updated, and a row's Check re-measures one on demand — so a button that re-measured
 * everything would only repeat those, at the cost of the whole list's probes and geo lookups.
 */
const AUTO_CHECK_MAX = 100;

function autoCheck(servers: Server[]) {
  if (servers.length <= AUTO_CHECK_MAX) {
    void checkServers(servers);
  } else {
    log(`[ui] ${servers.length.toLocaleString()} servers added; not testing them all unasked — use a row's Check`);
  }
}

/** The ⋯ menu's Check: the same test a server gets on arrival, for one server. */
async function checkOne(server: Server) {
  await checkServers([server]);
}

/** Mirrors `geo::Located`. */
interface Located {
  index: number;
  entry: Omit<Spot, "checkedAt"> | null;
  exit: Omit<Spot, "checkedAt"> | null;
}

/** Mirrors the Rust `Checked`: one server's full result, sent the moment it is known. */
interface Checked {
  run: number;
  index: number;
  latencyMs: number;
  error: string | null;
  exitIp: string | null;
  exit: Omit<Spot, "checkedAt"> | null;
}

/** Runs in flight, by id, so each `server-checked` event reaches the call that asked for it. */
const checkRuns = new Map<number, (checked: Checked) => void>();
let lastRun = 0;
void listen<Checked>("server-checked", (checked) => checkRuns.get(checked.run)?.(checked));

/**
 * Servers per `check_servers` call. Each call runs its own small probe core with a port per
 * server, so a batch bounds the ports, the listeners and the lookups in flight however long the
 * list is.
 */
const CHECK_BATCH = 50;

/**
 * Everything a server can be asked, in the order the answers become useful.
 *
 * 1. **Entry**, from DNS alone — seconds, and it works for a server that is down, so a freshly
 *    added row shows where its address is before anything else has answered.
 * 2. **The test itself**, end to end and per server: latency, then a real request *through* the
 *    server to see which public address it comes out of, then where that is. A server whose
 *    traffic never comes out does not work, whatever its latency said. Each row updates the
 *    moment its own result lands; `check_servers` runs a few at a time on one scratch core.
 *
 * Used by each row's Check, on servers just added, and after a subscription reload, so a
 * server's two locations are refreshed whenever it is measured at all.
 */
async function checkServers(snapshot: Server[]) {
  const all = [...snapshot];
  for (let start = 0; start < all.length; start += CHECK_BATCH) {
    await checkBatch(all.slice(start, start + CHECK_BATCH));
  }
}

/** One batch of `checkServers`: entries, then the end-to-end test, streamed per server. */
async function checkBatch(snapshot: Server[]) {
  // Snapshotted: results come back by position, and a list that changed underneath would pin
  // measurements on the wrong servers.
  const servers = [...snapshot];
  if (!servers.length || !inTauri) return;
  const ids = servers.map((s) => s.id);
  let done = 0;
  locations.setChecking(ids, true);

  try {
    await locate(servers, { entries: true, exits: false });

    if (!coreReady) {
      log("[ui] the core is not running, so only entries were checked");
      return;
    }

    log(`[ui] testing ${servers.length} server${servers.length === 1 ? "" : "s"}`);
    const run = ++lastRun;
    let working = 0;
    let allIn: () => void = () => {};
    const finished = new Promise<void>((resolve) => (allIn = resolve));

    checkRuns.set(run, (c) => {
      const server = servers[c.index];
      if (!server) return;
      let { latencyMs, error } = c;
      // Out of this machine's own address means the request never went through the server.
      if (c.exitIp && home && c.exitIp === home.ip) {
        latencyMs = -1;
        error = "traffic came out of this machine's own address, not through the server";
      }
      store.applyLatencies([{ id: server.id, latency: latencyMs, error }]);
      if (latencyMs >= 0 && c.exit) {
        store.applyLocations([{ id: server.id, exit: { ...c.exit, checkedAt: Date.now() } }]);
      }
      if (latencyMs >= 0) working += 1;
      done += 1;
      locations.setChecking([server.id], false);
      if (done === servers.length) allIn();
    });

    try {
      await invoke("check_servers", { run, profiles: servers.map((s) => s.profile) });
      // Events can trail the call's own reply; give the last of them a moment to land.
      await Promise.race([finished, new Promise((r) => setTimeout(r, 2000))]);
    } catch (e) {
      log(`[ui] test failed: ${String(e)}`);
    } finally {
      checkRuns.delete(run);
    }
    if (working < servers.length) log(`[ui] ${working} of ${servers.length} servers in this batch work`);
  } finally {
    locations.setChecking(ids, false);
  }
}

async function locate(servers: Server[], want: { entries: boolean; exits: boolean }) {
  try {
    const located = await invoke<Located[]>("locate_servers", {
      profiles: servers.map((s) => s.profile),
      entries: want.entries,
      exits: want.exits,
    });
    const now = Date.now();
    const stamp = (spot: Omit<Spot, "checkedAt"> | null): Spot | undefined =>
      spot ? { ...spot, checkedAt: now } : undefined;
    // A server cannot exit from the user's own address. If a probe says it does, the request
    // never went through the server, and filing it would put the user's location on the flag.
    for (const l of located) {
      if (l.exit && home && l.exit.ip === home.ip) {
        log(`[ui] discarded an exit for ${servers[l.index]?.profile.name}: it was this machine's own address`);
        l.exit = null;
      }
    }

    const updates = located
      .filter((l) => l.index >= 0 && l.index < servers.length)
      .map((l) => ({ id: servers[l.index].id, entry: stamp(l.entry), exit: stamp(l.exit) }));
    if (updates.length) store.applyLocations(updates);

    const what = want.exits ? "exits" : "entries";
    const found = updates.filter((u) => (want.exits ? u.exit : u.entry)).length;
    log(`[ui] placed ${found} of ${servers.length} ${what}`);
  } catch (e) {
    log(`[ui] could not locate servers: ${String(e)}`);
  }
}

function log(line: string) {
  logLines.push(line);
  if (logLines.length > MAX_LOG_LINES) logLines.shift();
  syncDiagnostics();
}

function syncDiagnostics() {
  diagnostics.update({
    storage: store.storageName,
    storageError: store.storageError,
    lines: logLines,
    transport: readiness?.transport ?? "unknown",
    transportState: readiness?.state ?? "unknown",
    transportDetail: readiness?.detail ?? null,
    coreConnected: coreReady,
  });
}

// ---------------------------------------------------------------- rendering

/** Whether this app currently has the system proxy pointed at its listener. */
let systemProxyOn = false;
/** Why setting it failed, when it did; shown on the status card until the next connect. */
let systemProxyError: string | null = null;

/** Settings are locked while the tunnel is up or coming up; see `SettingsPanel.setLocked`. */
function lockSettings() {
  settings.setLocked(connection !== "off");
}

function refresh() {
  syncDiagnostics();
  lockSettings();
  const server = store.selected();

  const settings = store.settings();

  status.render({
    state: connection,
    mode: settings.mode,
    proxyAddress:
      settings.mode === "proxy"
        ? `${settings.allowLan ? "0.0.0.0" : "127.0.0.1"}:${settings.proxyPort}`
        : null,
    server,
    connectedAt,
    uplink: shownRate.uplink,
    downlink: shownRate.downlink,
    exitIps,
    tunnelDevice,
    systemProxy: systemProxyOn,
    systemProxyError,
    ...placeLines(server),
    blockedReason: blockedReason(),
  });

  const { pins, route } = buildMap(server);
  map.setPins(pins, route);
  syncTray(server, blockedReason() === null);
}

/** What was last sent to the tray, so the once-a-second uptime refresh does not resend it. */
let trayShown = "";

/**
 * Mirrors the connection into the Linux top-bar menu (`tray.rs`), which owns no state of its own.
 *
 * The same honesty rule as the status card applies: proxy mode carries only what is pointed at
 * the listener, so its line names the listener instead of claiming the machine is connected.
 */
function syncTray(server: Server | undefined, canConnect: boolean) {
  const settings = store.settings();
  const detail =
    connection === "on" && settings.mode === "proxy"
      ? `proxy on ${settings.allowLan ? "0.0.0.0" : "127.0.0.1"}:${settings.proxyPort}`
      : null;
  const args = {
    state: connection,
    server: server ? serverName(server) : null,
    detail,
    canConnect,
  };

  const key = JSON.stringify(args);
  if (key === trayShown) return;
  trayShown = key;
  invoke("set_tray_status", args).catch(() => {
    // Outside Tauri, or not on Linux: there is no tray to update.
  });
}

/** The name the server list shows for a row: the config's own; see `LocationsPanel`. */
function serverName(server: Server): string {
  return server.profile.name;
}

/**
 * Where the selected config is, for the status card: its exit — measured live through the running
 * tunnel when connected, else the one saved by its last test — and its entry when that is
 * somewhere else (a CDN edge, a relay's front). Before any test, the entry is all there is.
 */
function placeLines(server: Server | undefined): { exitAt: PlaceLine | null; entryAt: PlaceLine | null } {
  if (!server) return { exitAt: null, entryAt: null };
  const exit: (PlaceLine & { ip: string }) | null =
    connection === "on" && exitPlace ? exitPlace : (server.exit ?? null);
  const entry = server.entry ?? null;
  return {
    exitAt: exit,
    entryAt: entry && (!exit || entry.ip !== exit.ip) ? entry : null,
  };
}

/** Why Connect will not work, phrased for someone who did not write the app. */
function blockedReason(): string | null {
  // A storage failure comes first: it means the server list on screen is not what is on disk.
  if (store.storageError) return `Could not read saved data: ${store.storageError}`;
  if (!coreReady) return "Waiting for the core to start";
  if (!store.get().servers.length) return "Add a server to get started";
  if (readiness && !readiness.ready) {
    return readiness.detail ?? `The ${readiness.transport} transport is not ready`;
  }
  return null;
}

/**
 * The map's dots and the path between them.
 *
 * With the tunnel down, the user's own dot is the one that matters — it is where their traffic
 * comes from — so it is the prominent one, labelled. With it up, the route is drawn from that dot
 * to the exit: measured coordinates when the exit has been looked up, the country's centre until
 * then. A proxy chain's servers will go between the two; the map already draws any number of legs.
 */
/** A spot's coordinates, when the lookup that placed it knew them. */
function spotCoords(spot: Spot | undefined): { lat: number; lon: number; city: string | null } | null {
  return spot && spot.lat !== null && spot.lon !== null
    ? { lat: spot.lat, lon: spot.lon, city: spot.city }
    : null;
}

/** One dot on the map: every server that exits in the same place. */
interface MapPlace {
  lat: number;
  lon: number;
  /** City when measured, else the country's name. */
  title: string;
  country: string;
  servers: Server[];
}

/** The dots as last drawn, by key, so a click can find the servers behind the dot it hit. */
let mapPlaces = new Map<string, MapPlace>();

/**
 * Groups servers into the dots the map draws.
 *
 * Keyed on coordinates rounded to about 50 km, so two lookups that put one city a few streets
 * apart are one dot, and on the country for servers not yet measured — those stand at the
 * country's centre until a sweep places them.
 */
function groupPlaces(): Map<string, MapPlace> {
  const places = new Map<string, MapPlace>();
  for (const server of store.get().servers) {
    // The same rule as the flag and the label (`located`): the exit once tested, the address
    // until then, the name only before the address has been looked up. The dot follows whichever
    // of those the row is showing, so a row and its dot never disagree.
    const where0 = located(server);
    const country = where0.country;
    const at = spotCoords(where0.source === "exit" ? server.exit : where0.source === "entry" ? server.entry : undefined);
    if (!at && !country) continue;
    const where = at ?? place(country);
    const key = at
      ? `${Math.round(where.lat * 2)},${Math.round(where.lon * 2)}`
      : `country:${country}`;

    const existing = places.get(key);
    if (existing) {
      existing.servers.push(server);
    } else {
      places.set(key, {
        lat: where.lat,
        lon: where.lon,
        title: where0.city || place(country).name,
        country,
        servers: [server],
      });
    }
  }
  return places;
}

function buildMap(selected: Server | undefined): { pins: Pin[]; route: Hop[] } {
  const on = connection === "on";
  const pins: Pin[] = [];
  mapPlaces = groupPlaces();

  // The dot the selected server belongs to, so the exit pin can stand in for it — and stay
  // clickable, since it is still that city's dot.
  const selectedKey = selected
    ? [...mapPlaces].find(([, p]) => p.servers.some((s) => s.id === selected.id))?.[0]
    : undefined;

  // The exit: measured through the running tunnel if that has answered, else where the last
  // sweep placed this server, else the centre of the country its name suggests.
  let exit: Pin | null = null;
  if (on && selected) {
    const at = exitPlace ?? spotCoords(selected.exit) ?? spotCoords(selected.entry);
    const country = exitPlace?.country ?? located(selected).country;
    const where = at ?? place(country);
    exit = {
      lon: where.lon,
      lat: where.lat,
      label: at?.city ?? place(country).name,
      active: true,
      key: selectedKey,
      name: selectedKey ? placeName(mapPlaces.get(selectedKey)!) : undefined,
    };
  }

  for (const [key, where] of mapPlaces) {
    if (exit && key === selectedKey) continue;
    pins.push({ lon: where.lon, lat: where.lat, key, name: placeName(where) });
  }

  if (home) {
    pins.push({
      lon: home.lon,
      lat: home.lat,
      home: true,
      here: !on,
      label: on ? undefined : `You · ${home.city ?? place(home.country).name}`,
      // Clickable, for the details: public address, provider, place.
      key: HOME_KEY,
      name: "Your location",
    });
  }
  if (exit) pins.push(exit);

  // A relayed server enters somewhere else first; the route shows the detour, which is the whole
  // reason to know the entry. Chains will add their hops the same way.
  const entry = on && selected && isRelayed(selected) ? spotCoords(selected.entry) : null;
  if (entry) pins.push({ lon: entry.lon, lat: entry.lat, label: `via ${entry.city ?? place(selected!.entry!.country).name}`, labelBelow: true, hop: true });
  const route: Hop[] = home && exit ? [home, ...(entry ? [entry] : []), exit] : [];

  return { pins, route };
}

function placeName(p: MapPlace): string {
  return p.servers.length > 1 ? `${p.title} · ${p.servers.length} servers` : p.title;
}

/**
 * A dot was clicked. One server there is simply selected; several open the picker, because
 * choosing among them by clicking the dot would be a coin toss.
 */
/** The map key of the user's own pin; no server place can have it (see `groupPlaces`). */
const HOME_KEY = "home:you";

function pickFromMap(key: string, at: PickPoint) {
  if (key === HOME_KEY) {
    if (!home) return;
    const network = [home.org, home.asn ? `AS${home.asn}` : null].filter(Boolean).join(" · ");
    picker.openDetails({
      title: "Your location",
      subtitle: [home.city, place(home.country).name].filter(Boolean).join(", "),
      country: home.country,
      rows: [
        ["Public IP", home.ip],
        ["ISP", network || "unknown"],
        ["City", home.city ?? "unknown"],
        ["Country", `${place(home.country).name} (${home.country})`],
      ],
      // Measured with the tunnel down, and kept while it is up: this is who the user is *without*
      // the tunnel — the thing it hides — not what websites see now.
      note:
        connection === "on"
          ? "Measured before connecting. While connected, websites see the exit instead."
          : "What websites see while you are not connected.",
      at,
    });
    return;
  }
  const where = mapPlaces.get(key);
  if (!where) return;
  if (where.servers.length === 1) {
    picker.close();
    selectServer(where.servers[0]);
    return;
  }
  picker.open({
    title: where.title,
    country: where.country,
    servers: where.servers,
    selectedId: store.get().selectedServerId,
    at,
  });
}

/** Selecting from the list and from the map are the same act, so they share one path. */
function selectServer(server: Server) {
  store.select(server.id);
  // Switching server while connected would silently leave you on the old one.
  if (connection === "on") void reconnect();
}

/** Mirrors `geo::Exit` without its place: what the status card shows. */
interface ExitIps {
  ipv4: string | null;
  ipv6: string | null;
  /** Set when every attempt failed: why no public address could be found through the tunnel. */
  failed?: string;
  /** What Cloudflare-hosted sites see; see `geo::CloudflareExit`. */
  cloudflare: { ip: string; country: string | null } | null;
}

/** Mirrors `geo::Whereabouts`. */
interface Whereabouts {
  ip: string;
  country: string;
  city: string | null;
  lat: number;
  lon: number;
  asn: number | null;
  org: string | null;
}

/**
 * Bumped on every connect and disconnect, so a lookup that was in flight across one can tell its
 * answer is about a network that no longer exists. Without it, a "where am I" asked just before
 * connecting in VPN mode could come back through the tunnel and put the user at their exit.
 */
let tunnelEpoch = 0;

/**
 * Finds the user's own public address and where it is, for the map's first dot.
 *
 * Only with the tunnel down: in VPN mode a direct request goes through the tunnel, and the answer
 * would be the exit dressed up as the user.
 */
async function locateHome() {
  if (!inTauri || connection !== "off") return;
  const epoch = tunnelEpoch;
  try {
    const found = await invoke<Whereabouts>("locate_me");
    if (epoch !== tunnelEpoch || connection !== "off") return;
    home = found;
    log(`[ui] this machine is in ${found.city ?? found.country}`);
    store.forgetExitsAt(found.ip);
    refresh();
  } catch (e) {
    log(`[ui] could not find this machine's location: ${String(e)}`);
  }
}

/**
 * Finds where the running tunnel comes out, for the exit chip and the end of the route.
 *
 * Asked a moment after connecting, because the first request through a fresh tunnel pays for its
 * handshake, and once more if that fails — a single slow start should not leave the chip saying
 * "checking" for the whole session.
 */
async function locateExit() {
  const epoch = tunnelEpoch;
  const settings = store.settings();
  const proxyPort = settings.mode === "proxy" ? settings.proxyPort : null;

  let lastError = "";
  for (const delay of [600, 3000, 8000]) {
    await new Promise((resolve) => setTimeout(resolve, delay));
    if (epoch !== tunnelEpoch || connection !== "on") return;
    try {
      const found = await invoke<ExitIps & { place: Whereabouts | null }>("locate_exit", {
        proxyPort,
      });
      if (epoch !== tunnelEpoch || connection !== "on") return;
      exitPlace = found.place;
      exitIps = { ipv4: found.ipv4, ipv6: found.ipv6, cloudflare: found.cloudflare };
      // A live end-to-end measurement is the freshest exit this config will have, so it is saved
      // as the config's exit — the row's flag and the card then agree.
      const server = store.selected();
      if (found.place && server && !(home && found.place.ip === home.ip)) {
        store.applyLocations([{ id: server.id, exit: { ...found.place, checkedAt: Date.now() } }]);
      }
      const where = found.place ? ` in ${found.place.city ?? found.place.country}` : "";
      log(`[ui] exiting from ${[found.ipv4, found.ipv6].filter(Boolean).join(" and ")}${where}`);
      refresh();
      return;
    } catch (e) {
      lastError = String(e);
      log(`[ui] could not find the exit yet: ${lastError}`);
    }
  }
  // Every attempt failed: nothing came out through the server. Said on the card, rather than
  // leaving "checking public IP…" up for the rest of the session as if an answer were coming.
  if (epoch !== tunnelEpoch || connection !== "on") return;
  exitIps = { ipv4: null, ipv6: null, cloudflare: null, failed: lastError || "no answer" };
  refresh();
}

// ---------------------------------------------------------------- tunnel

function buildRequest() {
  const server = store.selected();
  if (!server) throw new Error("no server selected");

  const bypass = store.get().bypass.map((rule) => ({ kind: rule.kind, value: rule.value }));
  const settings = store.settings();

  // Field names match the serde camelCase shape of config::BuildRequest.
  return {
    req: {
      profile: server.profile,
      mode: settings.mode,
      proxy: {
        port: settings.proxyPort,
        allowLan: settings.allowLan,
      },
      tun: {
        ipv4Cidr: settings.ipv4Cidr,
        mtu: settings.mtu,
        stack: settings.stack,
        strictRoute: settings.strictRoute,
        ipv6: settings.ipv6,
      },
      bypass,
      dns: settings.dns,
      logLevel: settings.logLevel,
    },
  };
}

async function connect() {
  const blocked = blockedReason();
  if (blocked) {
    log(`[ui] refusing to connect: ${blocked}`);
    refresh();
    return;
  }

  connection = "connecting";
  tunnelEpoch++;
  exitPlace = null;
  exitIps = null;
  refresh();

  try {
    await invoke("start_tunnel", buildRequest());
    connectedAt = Date.now();
    lastCounters = { uplink: 0, downlink: 0, at: 0 };
    connection = "on";
    log("[ui] tunnel started");
    void locateExit();
    await applySystemProxy();
  } catch (e) {
    connection = "off";
    log(`[ui] start failed: ${String(e)}`);
    // Surface the core's own words; it knows more about the failure than we do.
    readiness = readiness ? { ...readiness, ready: false, detail: String(e) } : readiness;
  }
  refresh();
}

/**
 * Points the system proxy at the listener, when proxy mode is running and the user asked for it.
 *
 * A failure does not fail the connection: the listener is up and anything pointed at it by hand
 * works. It is reported on the status card instead, because a user who asked for the system proxy
 * and silently did not get it would assume their browser is covered.
 */
async function applySystemProxy() {
  systemProxyOn = false;
  systemProxyError = null;
  const current = store.settings();
  if (current.mode !== "proxy" || !current.systemProxy) return;
  try {
    await invoke("set_system_proxy", { port: current.proxyPort });
    systemProxyOn = true;
    log(`[ui] system proxy set to 127.0.0.1:${current.proxyPort}`);
  } catch (e) {
    systemProxyError = String(e);
    log(`[ui] could not set the system proxy: ${systemProxyError}`);
  }
}

async function disconnect() {
  try {
    // Restores the system proxy too, on the Rust side, so it cannot outlive the listener.
    await invoke("stop_tunnel");
    log("[ui] tunnel stopped");
  } catch (e) {
    log(`[ui] stop failed: ${String(e)}`);
  }
  systemProxyOn = false;
  systemProxyError = null;
  connection = "off";
  tunnelEpoch++;
  exitIps = null;
  exitPlace = null;
  shownRate = { uplink: 0, downlink: 0 };
  refresh();
  // Asked again rather than remembered: the tunnel may have been up across a change of network.
  void locateHome();
}

async function reconnect() {
  await disconnect();
  await connect();
}

async function toggleConnection() {
  if (connection === "on") await disconnect();
  else if (connection === "off") await connect();
}

async function poll() {
  if (connection !== "on") return;

  // The uptime line ticks even when no bytes move.
  refresh();

  try {
    const counters = await invoke<Throughput>("query_stats");
    const now = Date.now();
    if (lastCounters.at > 0) {
      const seconds = (now - lastCounters.at) / 1000;
      if (seconds > 0) {
        shownRate = {
          uplink: Math.max(0, counters.uplink - lastCounters.uplink) / seconds,
          downlink: Math.max(0, counters.downlink - lastCounters.downlink) / seconds,
        };
      }
    }
    lastCounters = { ...counters, at: now };
  } catch (e) {
    // One failed poll is not worth tearing the UI down over; the connection event handles a real
    // disconnect.
    log(`[ui] stats: ${String(e)}`);
  }
}

async function refreshReadiness() {
  try {
    // Readiness is a different question per mode — privilege for a TUN, a free port for a
    // listener — so the answer is only meaningful alongside the mode it was asked about.
    readiness = await invoke<Readiness>("tunnel_readiness", { mode: store.settings().mode });
    if (!readiness.ready && readiness.detail) log(`[ui] ${readiness.detail}`);
  } catch (e) {
    readiness = null;
    log(`[ui] readiness: ${String(e)}`);
  }
  refresh();
}

interface FetchedSubscription {
  links: string[];
  quota: { usedBytes: number; totalBytes: number; resetsAt: number | null } | null;
  title: string | null;
}

/**
 * Refreshes a subscription.
 *
 * The fetch happens in Rust: a subscription URL is a credential, so it should never sit in the
 * webview's network log, and when a tunnel is up the request has to travel through it.
 *
 * Three rules this follows, all of them about not surprising the user:
 * - a server that is currently connected is kept even if the refresh drops it
 * - a failure keeps the previous servers rather than emptying the list
 * - what changed is reported, not applied silently
 */
async function refreshSubscription(group: Group) {
  if (!group.url) return;

  store.setRefreshing(group.id, true);
  log(`[ui] refreshing ${group.name}`);

  try {
    const fetched = await invoke<FetchedSubscription>("fetch_subscription", { url: group.url });

    const parsed: Omit<Server, "id" | "groupId">[] = [];
    const rejected: string[] = [];

    for (const link of fetched.links) {
      try {
        const profile = parseShareLink(link);
        parsed.push({
          profile,
          country: guessCountry(profile.name),
          city: guessCity(profile.name),
          latency: null,
          testedAt: null,
        });
      } catch (e) {
        // Protocols this build does not run. Collected rather than thrown: one unsupported entry
        // should not discard a subscription's other forty.
        rejected.push(e instanceof Error ? e.message : String(e));
      }
    }

    if (!parsed.length) {
      // Naming the reasons matters most when nothing survived. A WARP subscription is entirely
      // WireGuard, and "no supported servers" on its own leaves the user unable to tell a bad
      // link from a protocol this build simply cannot run.
      const reasons = [...new Set(rejected)];
      throw new Error(
        reasons.length
          ? `none of its ${rejected.length} servers can run here — ${reasons.join(" ")}`
          : "the subscription had no servers",
      );
    }

    // The new list is fetched and parsed before anything changes, so the old servers stay usable
    // for as long as the fetch takes. If the tunnel runs on one of them it is disconnected now,
    // right before the swap: the server it runs on may not survive the update, and keeping a
    // stale entry alive to protect it would leave the list out of step with the provider.
    const live = connectedServerId();
    if (live && store.serversIn(group.id).some((s) => s.id === live)) {
      log(`[ui] disconnecting to update ${group.name}, which the tunnel is running on`);
      await disconnect();
    }
    const summary = store.replaceSubscriptionServers(group.id, parsed, connectedServerId());
    store.update((data) => {
      const g = data.groups.find((x) => x.id === group.id);
      if (!g) return;
      g.refreshing = false;
      g.lastError = null;
      g.updatedAt = Date.now();
      if (fetched.quota) g.quota = fetched.quota;
      if (fetched.title) g.name = fetched.title;
    });

    const changes = [
      summary.added ? `${summary.added} added` : null,
      summary.removed ? `${summary.removed} removed` : null,
      summary.retired ? `${summary.retired} retired but still connected` : null,
      rejected.length ? `${rejected.length} unsupported` : null,
    ].filter(Boolean);

    log(`[ui] ${group.name}: ${changes.length ? changes.join(", ") : "no changes"}`);
    // A reload is a new list from the provider; where each server is and whether it works are
    // measured again rather than trusted from last time.
    autoCheck(store.serversIn(group.id));
  } catch (e) {
    // The previous servers stay exactly as they were; only the header says something went wrong.
    store.update((data) => {
      const g = data.groups.find((x) => x.id === group.id);
      if (!g) return;
      g.refreshing = false;
      g.lastError = String(e);
    });
    log(`[ui] ${group.name} refresh failed: ${String(e)}`);
  }
}

/** The server the tunnel is actually running on, which a refresh must never pull out from under it. */
function connectedServerId(): string | null {
  return connection === "on" ? (store.get().selectedServerId ?? null) : null;
}

// ---------------------------------------------------------------- add servers

/**
 * What a pasted line turned out to be.
 *
 * Subscriptions and share links arrive through the same box because that is how users receive
 * them — a provider hands you a page with both on it — and which kind is on the clipboard is a
 * question the app can answer for itself rather than ask.
 */
type Pasted =
  | { kind: "server"; server: Omit<Server, "id" | "groupId"> }
  | { kind: "subscription"; url: string; name: string }
  | { kind: "rejected"; reason: string };

/**
 * Names a subscription from its URL fragment.
 *
 * Providers put the display name there — `#%F0%9F%92%A6%20BPB%20Normal` is "💦 BPB Normal" — and
 * it is the only name available until the fetch returns, because `profile-title` is a header not
 * every panel sends. The host is a weak fallback but an honest one, and the first refresh replaces
 * either with whatever the subscription calls itself.
 */
function subscriptionName(url: string): string {
  try {
    const parsed = new URL(url);
    // A malformed percent-escape throws, which is why this sits inside the try rather than beside
    // it: a name is never worth failing an import over.
    const fragment = decodeURIComponent(parsed.hash.replace(/^#/, "")).trim();
    return fragment || parsed.hostname;
  } catch {
    return url;
  }
}

/**
 * Sorts one pasted line into a server, a subscription, or a rejection carrying its reason.
 *
 * `http://` is checked here as well as in Rust. Leaving it to the backend would mean creating a
 * group and failing it a moment later, when the reason can be given while the user is still
 * looking at what they pasted.
 */
function classify(line: string): Pasted {
  if (/^https:\/\//i.test(line)) {
    return { kind: "subscription", url: line, name: subscriptionName(line) };
  }

  if (/^http:\/\//i.test(line)) {
    return {
      kind: "rejected",
      reason:
        "A plain HTTP subscription would expose every server credential to the network. " +
        "Ask your provider for an https:// link.",
    };
  }

  try {
    const profile = parseShareLink(line);
    return {
      kind: "server",
      server: {
        profile,
        country: guessCountry(profile.name),
        city: guessCity(profile.name),
        latency: null,
        testedAt: null,
      },
    };
  } catch (e) {
    return { kind: "rejected", reason: e instanceof Error ? e.message : String(e) };
  }
}

/** The hand-added group, which is where a pasted share link goes. */
function manualGroupName(): string {
  return store.get().groups.find((g) => g.id === MANUAL_GROUP_ID)?.name ?? "Personal";
}

/**
 * Adds a subscription group and pulls it straight away.
 *
 * The group is created before the fetch rather than after it, so a slow or failing subscription
 * appears as a row that spins and then carries an error — exactly what a later refresh produces.
 * Fetching first would mean a dialog that hangs on a dead endpoint, and a second failure story to
 * write and keep in step with the first.
 *
 * Re-pasting a URL already on the list refreshes it instead of adding a second copy, because
 * pasting it again is the obvious way to ask for an update.
 */
function addSubscription(url: string, name: string) {
  const existing = store.get().groups.find((g) => g.url === url);
  if (existing) {
    log(`[ui] ${existing.name} is already on the list; refreshing it instead`);
    void refreshSubscription(existing);
    return;
  }

  const id = store.addSubscription(name, url);
  const group = store.get().groups.find((g) => g.id === id);
  if (group) void refreshSubscription(group);
}

function openAddServers() {
  openSheet((close) => buildAddServers(close));
}

/** The paste shortcut's modifier as this platform spells it. */
const MOD_KEY = /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘" : "Ctrl ";

/** The three ways in. A link is the common case, so it is where the sheet opens. */
type AddTab = "link" | "qr" | "manual";

const ADD_TABS: [AddTab, string][] = [
  ["link", "Link"],
  ["qr", "QR code"],
  ["manual", "Manual"],
];

/**
 * The Add servers sheet: paste a link, read a QR code, or fill in a form.
 *
 * All three end up in the same place. A QR code is only a link in another form, so reading one
 * hands its text to the Link tab, where it is parsed, previewed and rejected by name exactly like
 * a paste — there is no second import path to keep honest. Manual entry is the editor's form over
 * an empty profile, for a server someone was given as a list of settings rather than a link.
 *
 * What was pasted survives switching tabs, so looking at the QR tab does not cost a paste.
 */
function buildAddServers(close: () => void) {
  let tab: AddTab = "link";
  let pasted = "";

  const sheet = h("div", { class: "app sheet add", role: "dialog", "aria-label": "Add servers" });

  function show(next: AddTab) {
    tab = next;
    // The form needs the editor's height; a paste box and a drop zone do not.
    sheet.classList.toggle("editor", tab === "manual");
    const pane =
      tab === "link" ? linkPane() : tab === "qr" ? qrPane() : manualPane();
    render(
      sheet,
      sheetHead("Add servers", close),
      h(
        "div",
        { class: "addtabs" },
        h(
          "span",
          { class: "seg", role: "tablist", "aria-label": "How to add" },
          ...ADD_TABS.map(([key, text]) =>
            h(
              "button",
              {
                type: "button",
                role: "tab",
                class: key === tab ? "on" : "",
                "aria-selected": String(key === tab),
                onclick: () => key !== tab && show(key),
              },
              text,
            ),
          ),
        ),
      ),
      ...pane,
    );
    sheet.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }

  // ------------------------------------------------------------ link

  function linkPane(): Node[] {
    let servers: Omit<Server, "id" | "groupId">[] = [];
    let subscriptions: { url: string; name: string }[] = [];
    let rejected: string[] = [];

    const preview = h("div", { class: "parsed" });
    const footNote = h("span", { class: "gpick" });

    const addButton = h(
      "button",
      {
        class: "btn brand",
        disabled: true,
        onclick: () => {
          for (const sub of subscriptions) addSubscription(sub.url, sub.name);
          // MANUAL_GROUP_ID rather than groups[0]: the hand-added group is first only by insertion
          // order, and subscriptions append to the same array.
          if (servers.length) autoCheck(store.addServers(MANUAL_GROUP_ID, servers));
          close();
        },
      },
      "Add servers",
    );

    const input = h("textarea", {
      class: "paste",
      spellcheck: false,
      "data-autofocus": true,
      placeholder: "vless://uuid@host:443?security=reality&sni=…&pbk=…#Name\nhttps://example.com/sub",
      oninput: (e: Event) => reparse((e.target as HTMLTextAreaElement).value),
    }) as HTMLTextAreaElement;
    input.value = pasted;

    function reparse(raw: string) {
      pasted = raw;
      servers = [];
      subscriptions = [];
      rejected = [];

      for (const line of raw.split(/\s+/).filter(Boolean)) {
        const item = classify(line);
        if (item.kind === "server") servers.push(item.server);
        else if (item.kind === "subscription") subscriptions.push({ url: item.url, name: item.name });
        else rejected.push(item.reason);
      }

      const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

      const found = [
        subscriptions.length ? plural(subscriptions.length, "subscription") : null,
        servers.length ? plural(servers.length, "server") : null,
        rejected.length ? `${rejected.length} unsupported` : null,
      ].filter(Boolean);

      const willAdd = [
        subscriptions.length ? plural(subscriptions.length, "subscription") : null,
        servers.length ? plural(servers.length, "server") : null,
      ].filter(Boolean);

      addButton.disabled = willAdd.length === 0;
      addButton.textContent = willAdd.length ? `Add ${willAdd.join(" and ")}` : "Add servers";

      // Only a subscription-only paste needs the destination explained; anything with servers in
      // it still lands in the hand-added group.
      render(
        footNote,
        ...(subscriptions.length && !servers.length
          ? ["Each subscription brings its own group"]
          : ["Add to ", h("span", { class: "g" }, manualGroupName())]),
      );

      render(
        preview,
        ...(found.length
          ? [
              h("p", { class: "plabel" }, "Found", h("span", {}, found.join(" · "))),
              // A subscription is listed before its servers exist, because they only arrive with
              // the first fetch. All it can promise at this point is a name and a URL.
              ...subscriptions.map((sub) =>
                h(
                  "div",
                  { class: "prow" },
                  h("span", { class: "pmark ok" }, icon("check", 11)),
                  h("span", { class: "flag dim glyph" }, icon("globe", 11)),
                  h(
                    "span",
                    { class: "pmain" },
                    h("b", {}, sub.name),
                    h("span", {}, "Subscription · servers arrive on the first update"),
                  ),
                ),
              ),
              ...servers.map((server) =>
                h(
                  "div",
                  { class: "prow" },
                  h("span", { class: "pmark ok" }, icon("check", 11)),
                  h("span", { class: "flag", style: `background:${place(server.country).flag}` }),
                  h(
                    "span",
                    { class: "pmain" },
                    h("b", {}, server.profile.name),
                    h("span", {}, `${server.profile.server}:${server.profile.port}`),
                  ),
                ),
              ),
              // Rejected links are named, never silently dropped: this build runs six protocols,
              // so users will paste things it cannot.
              ...rejected.map((reason) =>
                h(
                  "div",
                  { class: "prow" },
                  h("span", { class: "pmark no" }, icon("close", 11)),
                  h("span", { class: "flag dim" }),
                  h("span", { class: "pmain dim" }, h("b", {}, "Not supported"), h("span", {}, reason)),
                ),
              ),
            ]
          : []),
      );
    }

    // Fills the footer and the preview before anything is typed, including after a tab switch.
    reparse(pasted);

    return [
      h(
        "button",
        {
          class: "clipbar",
          onclick: async () => {
            try {
              const text = await navigator.clipboard.readText();
              input.value = text;
              reparse(text);
            } catch {
              log("[ui] clipboard read was refused");
            }
          },
        },
        icon("clipboard", 16),
        h("span", { class: "ct" }, "Paste from clipboard"),
        h("span", { class: "kbd" }, `${MOD_KEY}V`),
      ),
      input,
      preview,
      h(
        "div",
        { class: "sheet-foot" },
        footNote,
        h("button", { class: "ghost", onclick: close }, "Cancel"),
        addButton,
      ),
    ];
  }

  // ------------------------------------------------------------ QR code

  function qrPane(): Node[] {
    const note = h("p", { class: "fnote qrnote" });
    const picker = h("input", {
      type: "file",
      accept: "image/*",
      hidden: true,
      onchange: () => {
        const file = picker.files?.[0];
        if (file) void read(file);
      },
    }) as HTMLInputElement;

    async function read(image: Blob) {
      render(note, "Reading…");
      note.classList.remove("bad");
      let text: string | null = null;
      try {
        text = await readQrCode(image);
      } catch (e) {
        log(`[ui] could not read that image: ${String(e)}`);
      }
      if (!text) {
        render(note, "No QR code found in that image. Try a sharper or larger one.");
        note.classList.add("bad");
        return;
      }
      // Appended rather than replacing, so a code can be read on top of links already pasted.
      pasted = pasted.trim() ? `${pasted.trim()}\n${text}` : text;
      show("link");
    }

    const zone = h(
      "div",
      {
        class: "dropzone",
        tabindex: 0,
        role: "button",
        "data-autofocus": true,
        "aria-label": "Choose an image with a QR code",
        onclick: () => picker.click(),
        onkeydown: (e: Event) => {
          const key = (e as KeyboardEvent).key;
          if (key === "Enter" || key === " ") {
            e.preventDefault();
            picker.click();
          }
        },
        // A screenshot on the clipboard is the usual source, so Ctrl+V works here directly.
        onpaste: (e: Event) => {
          const file = [...((e as ClipboardEvent).clipboardData?.files ?? [])].find((f) =>
            f.type.startsWith("image/"),
          );
          if (!file) return;
          e.preventDefault();
          void read(file);
        },
        ondragover: (e: Event) => {
          e.preventDefault();
          zone.classList.add("over");
        },
        ondragleave: () => zone.classList.remove("over"),
        ondrop: (e: Event) => {
          e.preventDefault();
          zone.classList.remove("over");
          const file = (e as DragEvent).dataTransfer?.files?.[0];
          if (file) void read(file);
        },
      },
      icon("scan", 30),
      h("span", { class: "dz-t" }, "Drop an image with a QR code"),
      h("span", { class: "dz-d" }, `or click to choose one · ${MOD_KEY}V pastes a screenshot`),
    );

    render(note, "The code is read on this machine; the image is not kept or sent anywhere.");

    return [
      zone,
      picker,
      note,
      h(
        "div",
        { class: "sheet-foot" },
        h("span", { class: "gpick" }),
        h("button", { class: "ghost", onclick: close }, "Cancel"),
      ),
    ];
  }

  // ------------------------------------------------------------ manual

  function manualPane(): Node[] {
    const body = h("div", { class: "set-scroll" });
    const problemLine = h("span", { class: "gpick" });

    const nameInput = h("input", {
      class: "field",
      type: "text",
      spellcheck: false,
      "aria-label": "Name",
      placeholder: "Name — optional",
      "data-autofocus": true,
    }) as HTMLInputElement;

    const addButton = h("button", { class: "btn brand" }, "Add server") as HTMLButtonElement;

    // The first problem only, as in the editor; with none left, the line says where it goes.
    const showProblems = (problems: { message: string }[]) => {
      addButton.disabled = problems.length > 0;
      problemLine.classList.toggle("bad", problems.length > 0);
      if (problems.length) render(problemLine, problems[0].message);
      else render(problemLine, "Add to ", h("span", { class: "g" }, manualGroupName()));
    };

    const editor = new ProfileEditor(body, blankProfile(), showProblems);

    addButton.onclick = () => {
      if (editor.problems().length) return;
      const profile = editor.value();
      const typed = nameInput.value.trim();
      // Something has to be in the list; the address is the one thing certain to be filled in.
      const name = typed || profile.server;
      const added = store.addServers(MANUAL_GROUP_ID, [
        {
          profile: { ...profile, name },
          country: guessCountry(name),
          city: guessCity(name),
          latency: null,
          testedAt: null,
          // A typed name is the user's, and the row should show it rather than a country guess.
          renamed: typed ? true : undefined,
        },
      ]);
      log(`[ui] added ${name} by hand`);
      void checkServers(added);
      close();
    };

    editor.render();
    // Shown from the start: an empty form is not yet addable, and saying why beats a disabled
    // button with no explanation.
    showProblems(editor.problems());

    return [
      h("label", { class: "flabel" }, "Name", nameInput),
      body,
      h(
        "div",
        { class: "sheet-foot" },
        problemLine,
        h("button", { class: "ghost", onclick: close }, "Cancel"),
        addButton,
      ),
    ];
  }

  show(tab);
  return sheet;
}

/**
 * What manual entry starts from: VLESS over TLS on 443, which is what most servers handed out as a
 * list of settings are. Everything else is empty, so nothing plausible-looking is invented.
 */
function blankProfile(): Profile {
  return {
    protocol: "vless",
    name: "",
    server: "",
    port: 443,
    uuid: "",
    flow: "",
    security: "",
    alterId: 0,
    password: "",
    tls: { enabled: true, sni: "", insecure: false, alpn: [], fingerprint: "", reality: null },
    transport: {
      kind: "tcp",
      path: "",
      host: "",
      serviceName: "",
      method: "",
      maxEarlyData: 0,
      earlyDataHeader: "",
    },
    wireguard: null,
  };
}

// ---------------------------------------------------------------- sheets

/**
 * Opens a modal over the scrim and hands the builder a way to close it.
 *
 * The dialogs differ only in their contents, so the plumbing — the scrim, click-outside, Escape —
 * lives here once. Escape in particular is why this exists rather than three copies: a modal that
 * traps you until you find the right button is a modal people learn to distrust.
 */
function openSheet(build: (close: () => void) => Node) {
  const scrim = qs<HTMLElement>("#scrim");

  const onKey = (event: KeyboardEvent) => {
    if (event.key === "Escape") close();
  };
  const onClick = (event: MouseEvent) => {
    if (event.target === scrim) close();
  };

  function close() {
    scrim.hidden = true;
    scrim.replaceChildren();
    // Both are removed, or every sheet ever opened keeps listening for the rest of the session.
    document.removeEventListener("keydown", onKey);
    scrim.removeEventListener("click", onClick);
  }

  scrim.hidden = false;
  render(scrim, build(close));
  // After insertion, not inside the builder: an element outside the document cannot take focus.
  scrim.querySelector<HTMLElement>("textarea, input")?.focus();
  document.addEventListener("keydown", onKey);
  scrim.addEventListener("click", onClick);
}

/** Head and close button, identical across the sheets. */
function sheetHead(title: string, close: () => void) {
  return h(
    "div",
    { class: "sheet-head" },
    h("span", { class: "t" }, title),
    h("button", { class: "x", "aria-label": "Close", onclick: close }, icon("close", 15)),
  );
}

/**
 * A yes/no sheet for something that cannot be undone.
 *
 * This app has no undo, and what is being deleted is usually a credential — a hand-added server or
 * a subscription URL exists nowhere else. So the sheet states what will be lost rather than asking
 * "are you sure?", which is a question nobody reads.
 */
function confirmSheet(options: {
  title: string;
  lines: (string | null)[];
  confirmLabel: string;
  onConfirm: () => void;
}) {
  openSheet((close) =>
    h(
      "div",
      { class: "app sheet confirm", role: "dialog", "aria-label": options.title },
      sheetHead(options.title, close),
      h(
        "div",
        { class: "confirm-body" },
        ...options.lines.filter((l): l is string => Boolean(l)).map((line) => h("p", {}, line)),
      ),
      h(
        "div",
        { class: "sheet-foot" },
        h("span", { class: "gpick" }),
        h("button", { class: "ghost", onclick: close }, "Cancel"),
        h(
          "button",
          {
            class: "btn danger",
            onclick: () => {
              options.onConfirm();
              close();
            },
          },
          options.confirmLabel,
        ),
      ),
    ),
  );
}

/** Whether the tunnel is currently running on this server. */
function isLive(server: Server): boolean {
  return connection !== "off" && store.get().selectedServerId === server.id;
}

function confirmDeleteServer(server: Server) {
  const group = store.group(server.groupId);
  const live = isLive(server);

  confirmSheet({
    title: "Delete this server?",
    lines: [
      `${server.profile.name} · ${describe(server.profile)}`,
      // A subscription server returns on the next update, so removing it is housekeeping. A
      // hand-added one is gone for good, and its credentials with it.
      group?.kind === "subscription"
        ? `It will come back the next time ${group.name} updates.`
        : "Its address and credentials are not saved anywhere else.",
      live ? "The tunnel is running on it and will be disconnected." : null,
    ],
    confirmLabel: "Delete",
    onConfirm: () => {
      store.removeServer(server.id);
      log(`[ui] deleted ${server.profile.name}`);
      if (live) void disconnect();
    },
  });
}

function confirmDeleteGroup(group: Group) {
  const servers = store.serversIn(group.id);
  const live = servers.some(isLive);

  confirmSheet({
    title: group.url ? "Delete this subscription?" : "Delete this group?",
    lines: [
      group.name,
      servers.length
        ? `Its ${servers.length} server${servers.length === 1 ? "" : "s"} go with it.`
        : "It has no servers.",
      // The URL is the credential: anyone holding it can read the whole server list, and this is
      // the only copy the app has.
      group.url ? "The subscription address is not saved anywhere else." : null,
      live ? "The tunnel is running on one of them and will be disconnected." : null,
    ],
    confirmLabel: "Delete",
    onConfirm: () => {
      store.removeGroup(group.id);
      log(`[ui] deleted ${group.name}`);
      if (live) void disconnect();
    },
  });
}

/**
 * Edits a server through a form over its profile.
 *
 * Not a share link in a text box: a link is a serialisation, and changing a port by finding it
 * between an `@` and a `?` makes the user the parser. A typo there does not fail — it produces a
 * different server. The profile is already structured data on disk, so the form edits that; a link
 * is generated from it only when the server is shared, by `openShareServer`.
 *
 * The name is separate from the rest because it is the one field that is not part of the
 * connection, and because renaming is the edit people make most often.
 */
function openEditServer(server: Server) {
  openSheet((close) => {
    const body = h("div", { class: "set-scroll" });
    const problemLine = h("span", { class: "gpick" });

    const nameInput = h("input", {
      class: "field",
      type: "text",
      spellcheck: false,
      "aria-label": "Name",
      placeholder: "Name",
    }) as HTMLInputElement;
    nameInput.value = server.profile.name;

    const saveButton = h("button", { class: "btn brand" }, "Save") as HTMLButtonElement;

    const editor = new ProfileEditor(body, server.profile, (problems) => {
      saveButton.disabled = problems.length > 0;
      // One at a time, and the first one: a list of five complaints about a half-filled form is
      // noise, and the top field is the one to fix first anyway.
      render(problemLine, problems.length ? problems[0].message : "Changes apply on save");
      problemLine.classList.toggle("bad", problems.length > 0);
    });

    saveButton.onclick = () => {
      if (editor.problems().length) return;

      const edited = editor.value();
      const name = nameInput.value.trim() || edited.name;
      const profile: Profile = { ...edited, name };
      const live = isLive(server);

      store.updateServer(server.id, profile, {
        country: guessCountry(name),
        city: guessCity(name),
        // Only a name that differs from the one the profile already carried counts as a rename;
        // re-saving an untouched sheet should not start overriding the country in the list.
        renamed: server.renamed === true || name !== server.profile.name,
      });
      log(`[ui] edited ${name}`);
      // A new address is a different place: the old entry and exit described the old one, so
      // they go, and the server is measured again — showing the new address's country at once
      // and the new exit after the test.
      if (profile.server !== server.profile.server || profile.port !== server.profile.port) {
        store.clearLocations(server.id);
        const updated = store.get().servers.find((s) => s.id === server.id);
        if (updated) void checkOne(updated);
      }
      // The tunnel is still running against the old settings, so it has to be rebuilt.
      if (live) void reconnect();
      close();
    };

    editor.render();

    return h(
      "div",
      { class: "app sheet editor", role: "dialog", "aria-label": "Edit server" },
      sheetHead("Edit server", close),
      h("label", { class: "flabel" }, "Name", nameInput),
      body,
      h(
        "div",
        { class: "sheet-foot" },
        problemLine,
        h("button", { class: "ghost", onclick: close }, "Cancel"),
        saveButton,
      ),
    );
  });
}

/**
 * Shows a server as a share link and a QR code, for a phone to scan or another client to import.
 *
 * The link is generated here, from the profile, rather than kept from whatever was pasted: the
 * profile is what the app actually connects with, so an edit made since import is what gets
 * shared. The standard `vless://` / `vmess://` / `trojan://` form is what v2rayNG, Hiddify and
 * Streisand all scan.
 *
 * The sheet says plainly that the link is the credential. A QR code on screen looks like a harmless
 * picture, and anyone who photographs it can use the server exactly as the user does.
 */
function openShareServer(server: Server) {
  let link: string;
  try {
    link = toShareLink(server.profile);
  } catch (e) {
    log(`[ui] could not write a share link for ${server.profile.name}: ${String(e)}`);
    return;
  }

  openSheet((close) => {
    const copyButton = h("button", { class: "btn brand" }, "Copy link") as HTMLButtonElement;
    let reset = 0;
    copyButton.onclick = async () => {
      const copied = await copyText(link);
      copyButton.textContent = copied ? "Copied" : "Copy failed";
      window.clearTimeout(reset);
      reset = window.setTimeout(() => (copyButton.textContent = "Copy link"), 1600);
    };

    const linkBox = h("textarea", {
      class: "val sharelink",
      readonly: true,
      rows: 4,
      spellcheck: false,
      "aria-label": "Share link",
      onclick: (e: Event) => (e.target as HTMLTextAreaElement).select(),
    }) as HTMLTextAreaElement;
    linkBox.value = link;

    return h(
      "div",
      { class: "app sheet share", role: "dialog", "aria-label": "Share server" },
      sheetHead("Share server", close),
      h(
        "div",
        { class: "share-body" },
        h("p", { class: "share-name" }, `${server.profile.name} · ${describe(server.profile)}`),
        h("div", { class: "share-qr" }, qrCode(link)),
        h("p", { class: "fnote" }, "Scan with a phone client such as v2rayNG, Hiddify or Streisand."),
        linkBox,
        h(
          "p",
          { class: "fnote warn" },
          "The link contains this server's credentials. Anyone who has it can use the server.",
        ),
      ),
      h(
        "div",
        { class: "sheet-foot" },
        h("span", { class: "gpick" }),
        h("button", { class: "ghost", onclick: close }, "Done"),
        copyButton,
      ),
    );
  });
}

/**
 * Puts text on the clipboard, and says whether it got there.
 *
 * The async Clipboard API is the right one, but WebKitGTK builds that predate it — or refuse it for
 * the app's origin — reject, so the old selection-and-`execCommand` route is kept as a fallback
 * rather than reporting a copy that did not happen.
 */
async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const scratch = h("textarea", { class: "offscreen", readonly: true }) as HTMLTextAreaElement;
    scratch.value = text;
    document.body.append(scratch);
    scratch.select();
    const ok = document.execCommand("copy");
    scratch.remove();
    return ok;
  }
}

// ---------------------------------------------------------------- rail icons

function paintRail() {
  const buttons: [Screen, string, number][] = [
    ["vpn", "shield", 20],
    ["rules", "globe", 20],
    ["settings", "sliders", 20],
    ["diagnostics", "activity", 18],
  ];

  for (const [name, glyph, size] of buttons) {
    const button = qs<HTMLButtonElement>(RAIL[name]);
    button.appendChild(icon(glyph, size));
    button.addEventListener("click", () => show(name));
  }
  qs(".logo").appendChild(icon("shield-check", 19));
}

// ---------------------------------------------------------------- boot

paintRail();

// The status card, the map and the tray all render from the store but, unlike the list, are not
// views that subscribe themselves. Without this they only caught up with a change — a server
// picked from the list or the map — on the next connect or the next once-a-second tick, and the
// tick only runs while connected, so a disconnected selection never reached the status card.
store.subscribe(() => refresh());

void (async () => {
  // Rendering before the data arrives would flash an empty list on every launch.
  await store.load();
  locations.render();
  refresh();
})();

void listen<string>("core-log", (line) => {
  log(`[core] ${line}`);
  // sing-box names the interface it opened; the status card shows it as proof the tunnel is real.
  const match = line.match(/\b(utun\d+|nunya-tun)\b/);
  if (match) {
    tunnelDevice = match[1];
    refresh();
  }
});

// The Linux top-bar menu's Connect/Disconnect; it runs exactly what the status card's button does.
void listen<null>("tray-toggle", () => void toggleConnection());

void listen<boolean>("core-connection", async (connected) => {
  coreReady = connected;
  log(`[ui] core ${coreReady ? "connected" : "disconnected"}`);

  if (coreReady) {
    await refreshReadiness();
  } else if (connection !== "off") {
    // The core went away with the tunnel up, so the TUN went with it — and the listener the
    // system proxy points at, which has to be put back now rather than at the next disconnect.
    connection = "off";
    tunnelEpoch++;
    if (systemProxyOn) void invoke("clear_system_proxy").catch(() => {});
    systemProxyOn = false;
    exitIps = null;
    exitPlace = null;
    refresh();
    void locateHome();
  }
  refresh();
});

void (async () => {
  if (!inTauri) {
    // Browser preview: no backend, so the UI renders in its "core not running" state.
    log("[ui] running outside Tauri; the tunnel backend is unavailable");
    refresh();
    return;
  }
  // Independent of the core: it is a direct request from the Rust side.
  void locateHome();
  coreReady = await invoke<boolean>("core_connected").catch(() => false);
  if (coreReady) await refreshReadiness();
  refresh();
})();

setInterval(() => void poll(), 1000);
