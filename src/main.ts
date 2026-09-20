import { inTauri, invoke, listen } from "./bridge";

import { h, qs, render } from "./dom";
import { guessCity, guessCountry, place } from "./geo";
import { store, type Group, type Server } from "./store";
import { parseVless } from "./share";
import { icon } from "./views/icons";
import { BypassPanel } from "./views/bypass";
import { DiagnosticsPanel } from "./views/diagnostics";
import { LocationsPanel } from "./views/locations";
import { WorldMap, type Pin } from "./views/map";
import { SettingsPanel } from "./views/settings";
import { StatusCard, type ConnectionState } from "./views/status";

/** Mirrors the Rust `Readiness` struct. */
interface Readiness {
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
let exitIp: string | null = null;
let tunnelDevice: string | null = null;

/** Cumulative counters from the previous poll, so the UI can show a rate rather than a total. */
let lastCounters = { uplink: 0, downlink: 0, at: 0 };
let shownRate = { uplink: 0, downlink: 0 };

const logLines: string[] = [];
const MAX_LOG_LINES = 500;

// ---------------------------------------------------------------- views

const status = new StatusCard(qs("#status"), { onToggle: () => void toggleConnection() });
const map = new WorldMap(qs<HTMLCanvasElement>("#worldmap"), qs("#pins"));

const panelHost = qs<HTMLElement>("#panel");
const bypass = new BypassPanel(panelHost);
const settings = new SettingsPanel(panelHost);
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

  if (screen === "rules") bypass.render();
  else if (screen === "settings") settings.render();
  else if (screen === "diagnostics") diagnostics.render();
}

const locations = new LocationsPanel(qs("#locations"), {
  onSelect: (server) => {
    store.select(server.id);
    // Switching server while connected would silently leave you on the old one.
    if (connection === "on") void reconnect();
  },
  onRefresh: (group) => void refreshSubscription(group),
  onAdd: () => openAddServers(),
  onQuickConnect: () => {
    const fastest = store.fastest();
    if (fastest) {
      store.select(fastest.id);
      void connect();
    }
  },
  onTestAll: () => void testAll(),
});

interface LatencyResult {
  index: number;
  latencyMs: number;
  error: string | null;
}

/**
 * Measures every server.
 *
 * Runs in its own short-lived core instance with no TUN, so it never disturbs a running tunnel.
 * That is why it is safe to offer while connected.
 */
async function testAll() {
  // Snapshotted: the results come back by index, and a list that changed underneath would attribute
  // measurements to the wrong servers.
  const servers = [...store.get().servers];
  if (!servers.length || !coreReady) return;

  locations.setTesting(true);
  log(`[ui] testing ${servers.length} servers`);

  try {
    const results = await invoke<LatencyResult[]>("test_servers", {
      profiles: servers.map((s) => s.profile),
    });

    store.applyLatencies(
      results
        .filter((r) => r.index >= 0 && r.index < servers.length)
        .map((r) => ({ id: servers[r.index].id, latency: r.latencyMs })),
    );

    const reachable = results.filter((r) => r.latencyMs >= 0).length;
    log(`[ui] ${reachable} of ${results.length} servers answered`);
  } catch (e) {
    log(`[ui] test failed: ${String(e)}`);
  } finally {
    locations.setTesting(false);
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

function refresh() {
  syncDiagnostics();
  const server = store.selected();

  status.render({
    state: connection,
    server,
    connectedAt,
    uplink: shownRate.uplink,
    downlink: shownRate.downlink,
    exitIp,
    tunnelDevice,
    blockedReason: blockedReason(),
  });

  map.setPins(buildPins(server));
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

function buildPins(selected: Server | undefined): Pin[] {
  const pins: Pin[] = [];
  const seen = new Set<string>();

  for (const server of store.get().servers) {
    if (!server.country || seen.has(server.country)) continue;
    seen.add(server.country);

    const where = place(server.country);
    const active = connection === "on" && selected?.country === server.country;
    pins.push({
      lon: where.lon,
      lat: where.lat,
      label: active ? where.name : undefined,
      active,
    });
  }
  return pins;
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
  refresh();

  try {
    await invoke("start_tunnel", buildRequest());
    connectedAt = Date.now();
    lastCounters = { uplink: 0, downlink: 0, at: 0 };
    connection = "on";
    log("[ui] tunnel started");
  } catch (e) {
    connection = "off";
    log(`[ui] start failed: ${String(e)}`);
    // Surface the core's own words; it knows more about the failure than we do.
    readiness = readiness ? { ...readiness, ready: false, detail: String(e) } : readiness;
  }
  refresh();
}

async function disconnect() {
  try {
    await invoke("stop_tunnel");
    log("[ui] tunnel stopped");
  } catch (e) {
    log(`[ui] stop failed: ${String(e)}`);
  }
  connection = "off";
  exitIp = null;
  shownRate = { uplink: 0, downlink: 0 };
  refresh();
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
    readiness = await invoke<Readiness>("tunnel_readiness");
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
    let rejected = 0;

    for (const link of fetched.links) {
      try {
        const profile = parseVless(link);
        parsed.push({
          profile,
          country: guessCountry(profile.name),
          city: guessCity(profile.name),
          latency: null,
          testedAt: null,
        });
      } catch {
        // Protocols this build does not run. Counted rather than thrown: one unsupported entry
        // should not discard a subscription's other forty.
        rejected += 1;
      }
    }

    if (!parsed.length) {
      throw new Error(
        rejected
          ? `none of the ${rejected} servers use a protocol this build supports`
          : "the subscription had no servers",
      );
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
      rejected ? `${rejected} unsupported` : null,
    ].filter(Boolean);

    log(`[ui] ${group.name}: ${changes.length ? changes.join(", ") : "no changes"}`);
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

function openAddServers() {
  const scrim = qs<HTMLElement>("#scrim");
  let parsed: { server: Omit<Server, "id" | "groupId">; ok: true }[] = [];
  let rejected: string[] = [];

  const preview = h("div", { class: "parsed" });
  const addButton = h(
    "button",
    {
      class: "btn brand",
      disabled: true,
      onclick: () => {
        store.addServers(store.get().groups[0].id, parsed.map((p) => p.server));
        close();
      },
    },
    "Add servers",
  );

  const input = h("textarea", {
    class: "paste",
    spellcheck: false,
    placeholder: "vless://uuid@host:443?security=reality&sni=…&pbk=…#Name",
    oninput: (e: Event) => reparse((e.target as HTMLTextAreaElement).value),
  }) as HTMLTextAreaElement;

  function reparse(raw: string) {
    parsed = [];
    rejected = [];

    for (const line of raw.split(/\s+/).filter(Boolean)) {
      try {
        const profile = parseVless(line);
        const country = guessCountry(profile.name);
        parsed.push({
          ok: true,
          server: {
            profile,
            country,
            city: guessCity(profile.name),
            latency: null,
            testedAt: null,
          },
        });
      } catch (e) {
        rejected.push(e instanceof Error ? e.message : String(e));
      }
    }

    addButton.disabled = parsed.length === 0;
    addButton.textContent = parsed.length
      ? `Add ${parsed.length} server${parsed.length === 1 ? "" : "s"}`
      : "Add servers";

    render(
      preview,
      ...(parsed.length || rejected.length
        ? [
            h(
              "p",
              { class: "plabel" },
              "Found",
              h(
                "span",
                {},
                `${parsed.length + rejected.length} link${
                  parsed.length + rejected.length === 1 ? "" : "s"
                } · ${parsed.length} supported`,
              ),
            ),
            ...parsed.map((p) =>
              h(
                "div",
                { class: "prow" },
                h("span", { class: "pmark ok" }, icon("check", 11)),
                h("span", { class: "flag", style: `background:${place(p.server.country).flag}` }),
                h(
                  "span",
                  { class: "pmain" },
                  h("b", {}, p.server.profile.name),
                  h(
                    "span",
                    {},
                    `${p.server.profile.server}:${p.server.profile.port}`,
                  ),
                ),
              ),
            ),
            // Rejected links are named, never silently dropped: this build runs six protocols, so
            // users will paste things it cannot.
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

  function close() {
    scrim.hidden = true;
    scrim.replaceChildren();
  }

  const sheet = h(
    "div",
    { class: "app sheet", role: "dialog", "aria-label": "Add servers" },
    h(
      "div",
      { class: "sheet-head" },
      h("span", { class: "t" }, "Add servers"),
      h("button", { class: "x", "aria-label": "Close", onclick: close }, icon("close", 15)),
    ),
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
      h("span", { class: "kbd" }, "⌘V"),
    ),
    input,
    preview,
    h(
      "div",
      { class: "sheet-foot" },
      h("span", { class: "gpick" }, "Add to ", h("span", { class: "g" }, store.get().groups[0].name)),
      h("button", { class: "ghost", onclick: close }, "Cancel"),
      addButton,
    ),
  );

  scrim.hidden = false;
  render(scrim, sheet);
  input.focus();

  scrim.addEventListener("click", (e) => {
    if (e.target === scrim) close();
  });
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

void listen<boolean>("core-connection", async (connected) => {
  coreReady = connected;
  log(`[ui] core ${coreReady ? "connected" : "disconnected"}`);

  if (coreReady) {
    await refreshReadiness();
  } else if (connection !== "off") {
    // The core went away with the tunnel up, so the TUN went with it.
    connection = "off";
    refresh();
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
  coreReady = await invoke<boolean>("core_connected").catch(() => false);
  if (coreReady) await refreshReadiness();
  refresh();
})();

setInterval(() => void poll(), 1000);
