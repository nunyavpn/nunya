/**
 * Connecting and disconnecting the tunnel, and the connection-state fields every screen renders
 * from — extracted out of `main.ts`'s own "tunnel" section per `ENGINEERING_STANDARDS.md`.
 *
 * Connect, disconnect and reconnect run through one `Serial` queue, one at a time: a Disconnect
 * that ran while Connect was still setting the system proxy once put the user's settings back and
 * then had ours land on top, pointing every browser at a port nothing listened on (see
 * `serial.ts`). `main.ts` and the popover both need to agree on what "connected" means right now,
 * so the state lives here, exported, rather than duplicated in each.
 *
 * None of this state is in `store.ts`: it should never survive a restart, and the exit's address
 * would be actively wrong to persist and reuse on a network it no longer describes. What this
 * module cannot do on its own — render a screen, save usage, look up where the user is — is asked
 * of `main.ts` through `initTunnel`'s hooks, the same plain-callback shape `views/status.ts`'s
 * `StatusCard` uses, so this module never has to reach back into `main.ts` by name.
 */
import { invoke } from "../bridge";
import { Serial } from "../serial";
import { store } from "../store";
import type { ConnectionState } from "../views/status";

/** Mirrors the Rust `Readiness` struct. */
export interface Readiness {
  mode: string;
  transport: string;
  ready: boolean;
  state: "disconnected" | "connecting" | "connected" | "needsPermission";
  detail: string | null;
}

/** Mirrors `geo::Exit` without its place: what the status card shows. */
export interface ExitIps {
  ipv4: string | null;
  ipv6: string | null;
  /** Set when every attempt failed: why no public address could be found through the tunnel. */
  failed?: string;
  /** What Cloudflare-hosted sites see; see `geo::CloudflareExit`. */
  cloudflare: { ip: string; country: string | null } | null;
}

/** Mirrors `geo::Whereabouts`. */
export interface Whereabouts {
  ip: string;
  country: string;
  city: string | null;
  lat: number;
  lon: number;
  asn: number | null;
  org: string | null;
}

/** What `main.ts` supplies so this module never has to reach back into it directly. */
export interface TunnelHooks {
  log(line: string): void;
  refresh(): void;
  blockedReason(): string | null;
  /** Where this machine is, looked up with the tunnel down; null while unplaced or connected. */
  getHome(): Whereabouts | null;
  /** A connect just landed: reset the usage/throughput counters main.ts owns for the new session. */
  onConnected(serverId: string | null, connectedAt: number): void;
  /** One last reading of the counters before they go away with the tunnel. */
  sampleBeforeStop(): Promise<void>;
  /** Writes counted traffic into the store and clears which config it belonged to. */
  finishUsageSession(): void;
  /** Clears the shown throughput rate. */
  resetRate(): void;
  syncBlockLists(): void;
  /** Asked again from the start: an earlier failure may have been the tunnel's doing. */
  locateHomeNow(): void;
  /** Asked again as-is, without resetting a retry already under way. */
  locateHome(): void;
}

let hooks: TunnelHooks;

/** Must be called once, during boot, before anything can trigger a connect or disconnect. */
export function initTunnel(next: TunnelHooks): void {
  hooks = next;
}

export let connection: ConnectionState = "off";
export let connectedAt = 0;
/** The running tunnel's public addresses, per family, once measured; null while checking. */
export let exitIps: ExitIps | null = null;
/** Where the running tunnel comes out, once measured. */
export let exitPlace: Whereabouts | null = null;
/**
 * Why the last attempt to connect failed, or why a running tunnel stopped: the rail shield's red.
 * Cleared when a new attempt starts or the user disconnects, never by itself; see `shield.ts`.
 */
export let tunnelFault: string | null = null;
/**
 * Bumped on every connect and disconnect, so a lookup that was in flight across one can tell its
 * answer is about a network that no longer exists. Without it, a "where am I" asked just before
 * connecting in VPN mode could come back through the tunnel and put the user at their exit.
 */
export let tunnelEpoch = 0;
/** Whether this app currently has the system proxy pointed at its listener. */
export let systemProxyOn = false;
/** Why setting it failed, when it did; shown on the status card until the next connect. */
export let systemProxyError: string | null = null;
/** The step under way while connecting or disconnecting, for the status card and the popover. */
export let transitionStep: string | null = null;
export let readiness: Readiness | null = null;

/** `main.ts`'s `refreshReadiness` is the only other writer; this keeps the field exported-read-only. */
export function setReadiness(next: Readiness | null): void {
  readiness = next;
}

export function buildRequest() {
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
      // The switches only: which lists are on disk is the Rust side's to know.
      block: { ads: settings.blockAds, trackers: settings.blockTrackers },
    },
  };
}

/**
 * Connecting and disconnecting, one at a time; see `serial.ts` for why a queue and not only
 * disabled buttons. Each checks the state when its turn comes, so a Connect queued behind a
 * Connect is nothing, and a reconnect queued behind a Disconnect does not start the tunnel again.
 */
const queue = new Serial();

/** Connecting or disconnecting: nothing that would start another is offered until it is done. */
export function working(): boolean {
  return connection === "connecting" || connection === "disconnecting";
}

export function connect(): Promise<void> {
  return queue.run(connectNow);
}

export function disconnect(): Promise<void> {
  return queue.run(disconnectNow);
}

/**
 * Starts the tunnel again so a change applies — a server, a mode, a block list — if it is up when
 * its turn comes. Asked for several times while one waits, it is one reconnect: the settings it
 * reads are the latest.
 */
export function reconnect(): Promise<void> {
  return queue.once("reconnect", async () => {
    if (connection !== "on") return;
    await disconnectNow();
    await connectNow();
  });
}

export async function toggleConnection(): Promise<void> {
  if (working()) return;
  if (connection === "on") await disconnect();
  else await connect();
}

async function connectNow() {
  if (connection !== "off") return;
  const blocked = hooks.blockedReason();
  if (blocked) {
    hooks.log(`[ui] refusing to connect: ${blocked}`);
    hooks.refresh();
    return;
  }

  connection = "connecting";
  transitionStep = "Starting the tunnel…";
  tunnelFault = null;
  tunnelEpoch++;
  exitPlace = null;
  exitIps = null;
  hooks.refresh();

  try {
    const running = store.selected()?.id ?? null;
    await invoke("start_tunnel", buildRequest());
    connectedAt = Date.now();
    hooks.onConnected(running, connectedAt);
    // Quick Connect's "Latest". Only once the tunnel is really up: a connect that failed was not
    // a connection, and should not become the thing offered first next time.
    if (running) store.markConnected(running, connectedAt);
    hooks.log("[ui] tunnel started");
    // Part of connecting, not something after it. Until it is set, "on" would claim the browsers
    // are covered when they are not, and a Disconnect would have nothing it could safely undo.
    if (wantsSystemProxy()) {
      transitionStep = "Setting the system proxy…";
      hooks.refresh();
    }
    await applySystemProxy();
    connection = "on";
    void locateExit();
    // A list that could not be fetched directly is fetched now, through the tunnel.
    hooks.syncBlockLists();
  } catch (e) {
    connection = "off";
    tunnelFault = `the connection could not start (${String(e)})`;
    hooks.log(`[ui] start failed: ${String(e)}`);
    // Surface the core's own words; it knows more about the failure than we do.
    readiness = readiness ? { ...readiness, ready: false, detail: String(e) } : readiness;
  } finally {
    transitionStep = null;
  }
  hooks.refresh();
}

/** Proxy mode, with the user's say-so to point the desktop's proxy setting at the listener. */
function wantsSystemProxy(): boolean {
  const current = store.settings();
  return current.mode === "proxy" && current.systemProxy;
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
    hooks.log(`[ui] system proxy set to 127.0.0.1:${current.proxyPort}`);
  } catch (e) {
    systemProxyError = String(e);
    hooks.log(`[ui] could not set the system proxy: ${systemProxyError}`);
  }
}

async function disconnectNow() {
  if (connection !== "on") return;
  // Said at once, before anything else is awaited: a button still reading Disconnect is a button
  // clicked again.
  connection = "disconnecting";
  transitionStep = systemProxyOn ? "Restoring the system proxy…" : "Stopping the tunnel…";
  hooks.refresh();
  // One last reading before the counters go away with the tunnel, so the seconds since the last
  // poll are counted too.
  await hooks.sampleBeforeStop();

  // The system proxy first. Pointed at a listener that is about to close, every app that follows
  // it would lose the network for as long as stopping takes; put back, they go direct at once.
  if (systemProxyOn) {
    transitionStep = "Restoring the system proxy…";
    hooks.refresh();
    try {
      await invoke("clear_system_proxy");
    } catch (e) {
      hooks.log(`[ui] could not restore the system proxy: ${String(e)}`);
    }
  }
  transitionStep = "Stopping the tunnel…";
  hooks.refresh();
  try {
    // Restores the system proxy too, on the Rust side, if it is still ours: no way of stopping
    // may leave it pointing at a listener that is gone.
    await invoke("stop_tunnel");
    hooks.log("[ui] tunnel stopped");
  } catch (e) {
    hooks.log(`[ui] stop failed: ${String(e)}`);
  }
  hooks.finishUsageSession();
  // Asked for, so whatever went wrong before is no longer the state to report.
  tunnelFault = null;
  systemProxyOn = false;
  systemProxyError = null;
  connection = "off";
  transitionStep = null;
  tunnelEpoch++;
  exitIps = null;
  exitPlace = null;
  hooks.resetRate();
  hooks.refresh();
  // Asked again rather than remembered: the tunnel may have been up across a change of network.
  // From the start: an earlier failure may have been the tunnel's doing, not the network's.
  hooks.locateHomeNow();
}

/**
 * The core went away with the tunnel up, so the TUN went with it — and the listener the system
 * proxy points at, which has to be put back now rather than at the next disconnect.
 */
export function handleCoreDied(): void {
  if (connection === "off") return;
  connection = "off";
  tunnelEpoch++;
  // What was counted before the core went is real traffic; only the last poll's worth is lost.
  hooks.finishUsageSession();
  tunnelFault = "the core stopped while connected";
  if (systemProxyOn) void invoke("clear_system_proxy").catch(() => {});
  systemProxyOn = false;
  exitIps = null;
  exitPlace = null;
  hooks.refresh();
  hooks.locateHome();
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
      const home = hooks.getHome();
      if (found.place && server && !(home && found.place.ip === home.ip)) {
        store.applyLocations([{ id: server.id, exit: { ...found.place, checkedAt: Date.now() } }]);
      }
      const where = found.place ? ` in ${found.place.city ?? found.place.country}` : "";
      hooks.log(`[ui] exiting from ${[found.ipv4, found.ipv6].filter(Boolean).join(" and ")}${where}`);
      hooks.refresh();
      return;
    } catch (e) {
      lastError = String(e);
      hooks.log(`[ui] could not find the exit yet: ${lastError}`);
    }
  }
  // Every attempt failed: nothing came out through the server. Said on the card, rather than
  // leaving "checking public IP…" up for the rest of the session as if an answer were coming.
  if (epoch !== tunnelEpoch || connection !== "on") return;
  exitIps = { ipv4: null, ipv6: null, cloudflare: null, failed: lastError || "no answer" };
  hooks.refresh();
}
