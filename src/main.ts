import { inTauri, invoke, listen } from "./bridge";

import { h, qs, render } from "./dom";
import { guessCity, guessCountry, place } from "./geo";
import { MANUAL_GROUP_ID, store, type Group, type Server } from "./store";
import { describe, parseShareLink, type Profile } from "./share";
import { icon } from "./views/icons";
import { BypassPanel } from "./views/bypass";
import { DiagnosticsPanel } from "./views/diagnostics";
import { LocationsPanel } from "./views/locations";
import { WorldMap, type Pin } from "./views/map";
import { SettingsPanel } from "./views/settings";
import { StatusCard, type ConnectionState } from "./views/status";
import { ProfileEditor } from "./views/editor";

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
        .map((r) => ({ id: servers[r.index].id, latency: r.latencyMs, error: r.error })),
    );

    const reachable = results.filter((r) => r.latencyMs >= 0).length;
    log(`[ui] ${reachable} of ${results.length} servers answered`);

    // Only servers that answered, and only after the latencies are on screen: locating them
    // starts a second core and makes a request through each one, which is far slower than the
    // measurement and worth nothing for a server that is down.
    const live = results.filter((r) => r.latencyMs >= 0).map((r) => servers[r.index]);
    if (live.length) await locateServers(live);
  } catch (e) {
    log(`[ui] test failed: ${String(e)}`);
  } finally {
    locations.setTesting(false);
  }
}

/**
 * Replaces guessed flags with measured ones.
 *
 * The flag beside a server was inferred from its name, which is whatever the provider typed —
 * often the country they want you to believe rather than the one the traffic leaves from. This
 * asks each server where it actually exits.
 *
 * Failures are silent by design: a server that cannot reach the lookup keeps the guess, which is
 * no worse than before the sweep.
 */
async function locateServers(servers: Server[]) {
  try {
    const located = await invoke<{ index: number; country: string; ip: string }[]>(
      "locate_servers",
      { profiles: servers.map((s) => s.profile) },
    );

    const updates = located
      .filter((l) => l.index >= 0 && l.index < servers.length && l.country)
      .map((l) => ({ id: servers[l.index].id, country: l.country }));

    if (updates.length) store.applyCountries(updates);
    log(`[ui] located ${updates.length} of ${servers.length} servers`);
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

function refresh() {
  syncDiagnostics();
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

function buildAddServers(close: () => void) {
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
        if (servers.length) store.addServers(MANUAL_GROUP_ID, servers);
        close();
      },
    },
    "Add servers",
  );

  const input = h("textarea", {
    class: "paste",
    spellcheck: false,
    placeholder: "vless://uuid@host:443?security=reality&sni=…&pbk=…#Name\nhttps://example.com/sub",
    oninput: (e: Event) => reparse((e.target as HTMLTextAreaElement).value),
  }) as HTMLTextAreaElement;

  function reparse(raw: string) {
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

    // Only a subscription-only paste needs the destination explained; anything with servers in it
    // still lands in the hand-added group.
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
            // A subscription is listed before its servers exist, because they only arrive with the
            // first fetch. All it can promise at this point is a name and a URL.
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

  const sheet = h(
    "div",
    { class: "app sheet", role: "dialog", "aria-label": "Add servers" },
    sheetHead("Add servers", close),
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
      footNote,
      h("button", { class: "ghost", onclick: close }, "Cancel"),
      addButton,
    ),
  );

  // Fills the footer before anything is typed, which is where the destination is stated.
  reparse("");

  return sheet;
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
 * different server. The profile is already structured data on disk, so the form edits that and
 * the link, shown at the bottom of the sheet, becomes an export rather than the interface.
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
