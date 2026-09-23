/**
 * The tunnel's live throughput and the usage counters it feeds — extracted out of `main.ts`'s own
 * "blocking" section per `ENGINEERING_STANDARDS.md`, which named this exact code as the proof
 * that a comment banner is not a module boundary: it lived under "blocking" alongside code about
 * block lists, sharing nothing with it beyond proximity.
 *
 * Polled once a second while connected (`poll`, called from `main.ts`'s boot), and written into
 * the usage store every `USAGE_SAVE_MS` rather than on every poll, so a quit while connected can
 * lose at most that much of the session's count. Disconnecting, and the core going away, ask for
 * `finishSession` instead, which writes at once.
 */
import { invoke } from "../bridge";
import { store } from "../store";
import { advance, type Bytes } from "../usage";
import { connection } from "./tunnel";

interface Throughput {
  uplink: number;
  downlink: number;
}

/** What `main.ts` supplies so this module never has to reach back into it directly. */
export interface ThroughputHooks {
  log(line: string): void;
  refresh(): void;
}

let hooks: ThroughputHooks;

/** Must be called once, during boot, before `poll` is scheduled. */
export function initThroughput(next: ThroughputHooks): void {
  hooks = next;
}

/** Cumulative counters from the previous poll, so the UI can show a rate rather than a total. */
let lastCounters = { uplink: 0, downlink: 0, at: 0 };
export let shownRate = { uplink: 0, downlink: 0 };

/**
 * The config the running tunnel is carrying traffic for, fixed when it connected.
 *
 * Not the store's selection: choosing another server while connected changes the selection first
 * and reconnects after, and the last seconds of the old session belong to the old config.
 */
let usageServerId: string | null = null;
/** Traffic counted since it was last written into the store. */
let pendingUsage: Bytes = { up: 0, down: 0 };
let usageSavedAt = 0;

/**
 * How often counted traffic is written into the store while connected.
 *
 * Not every poll: each write re-renders the list and queues a save of the whole data file, once a
 * second for as long as the tunnel runs. The cost is that quitting while connected can lose up to
 * this much of the session's count. Disconnecting, and the core going away, write at once.
 */
const USAGE_SAVE_MS = 15_000;

/** Starts a new session's counters; called once a connect really lands. */
export function startSession(serverId: string | null, connectedAt: number): void {
  lastCounters = { uplink: 0, downlink: 0, at: 0 };
  usageServerId = serverId;
  pendingUsage = { up: 0, down: 0 };
  usageSavedAt = connectedAt;
}

export async function poll() {
  if (connection !== "on") return;

  // The uptime line ticks even when no bytes move.
  hooks.refresh();
  // A slow reply is not queued behind: the next tick reads again.
  if (!sampling) await sample();
}

/** The reading in flight, if any; see `sample`. */
let sampling: Promise<void> | null = null;

/**
 * Takes one reading of the counters, after any already in flight.
 *
 * One at a time because two could land out of order, and a reading older than the last is
 * indistinguishable from a core that restarted — whose traffic `advance` counts again in full.
 */
export async function sample(): Promise<void> {
  while (sampling) await sampling;
  sampling = readCounters().finally(() => (sampling = null));
  return sampling;
}

/** Reads the tunnel's counters: the rate the status card shows, and the traffic usage is made of. */
async function readCounters() {
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
    // The first reading counts in full: the counters start at zero when the tunnel comes up, and
    // `lastCounters` is reset to zero with them.
    const moved = advance(lastCounters, counters);
    pendingUsage = { up: pendingUsage.up + moved.up, down: pendingUsage.down + moved.down };
    lastCounters = { ...counters, at: now };
    if (now - usageSavedAt >= USAGE_SAVE_MS) saveUsage();
  } catch (e) {
    // One failed poll is not worth tearing the UI down over; the connection event handles a real
    // disconnect.
    hooks.log(`[ui] stats: ${String(e)}`);
  }
}

/** Writes counted traffic into the store, under the config the tunnel was running on. */
function saveUsage() {
  usageSavedAt = Date.now();
  if (!usageServerId) return;
  const bytes = pendingUsage;
  pendingUsage = { up: 0, down: 0 };
  store.recordUsage(usageServerId, usageSavedAt, bytes);
}

/** Ends a session: writes whatever was not yet saved, and clears which config it belonged to. */
export function finishSession(): void {
  saveUsage();
  usageServerId = null;
}

export function resetRate(): void {
  shownRate = { uplink: 0, downlink: 0 };
}
