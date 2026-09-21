/**
 * What Quick Connect offers: the config used last, the one used most, and the fastest.
 *
 * It used to offer only the fastest, which is rarely what a returning user wants — they want what
 * they used yesterday, or what they use every day — and "fastest" could pick a config they had
 * never tried, from a subscription they do not trust. Each is now its own one-click row, so there
 * is no "default" to choose or remember.
 *
 * - **Latest**: the config the tunnel last ran on (`Server.lastConnectedAt`, set on connect).
 *   Not the selection: selecting a row is not connecting to it.
 * - **Most used**: the most traffic in the last `RECENT_DAYS`, from the usage history. Bytes
 *   rather than sessions or hours because that is what is recorded; it favours the config that
 *   carries the heavy traffic, which is usually the one that works.
 * - **Fastest**: the lowest latency among configs whose last test passed.
 *
 * One config can be more than one of these, and is then offered once with both reasons rather
 * than twice under two labels. A target with nothing behind it is left out, not shown disabled:
 * a new install has no history, and Fastest appears when a test first passes.
 *
 * Pure and generic over the item, so it runs under `node --test` on plain objects; the store turns
 * servers into candidates.
 */

export type QuickKind = "latest" | "mostUsed" | "fastest";

/** Days the "most used" ranking looks back over. */
export const RECENT_DAYS = 30;

export interface Candidate<T> {
  item: T;
  /** Unix ms of the last connection on it, or `null` for never. */
  lastConnectedAt: number | null;
  /** Bytes up and down over the last `RECENT_DAYS`. */
  recentBytes: number;
  /** Milliseconds, `-1` unreachable, `null` untested — as `Server.latency`. */
  latency: number | null;
}

export interface QuickTarget<T> {
  candidate: Candidate<T>;
  /** Why it is offered, in the order the rows are listed. */
  kinds: QuickKind[];
}

/** The rows' order: what the user did, then what they do most, then what the network says. */
const ORDER: QuickKind[] = ["latest", "mostUsed", "fastest"];

/** Up to three targets, one per config, in `ORDER`. */
export function quickTargets<T>(candidates: Candidate<T>[]): QuickTarget<T>[] {
  const picks: Record<QuickKind, Candidate<T> | undefined> = {
    latest: best(candidates.filter((c) => c.lastConnectedAt !== null), (c) => c.lastConnectedAt ?? 0),
    mostUsed: best(candidates.filter((c) => c.recentBytes > 0), (c) => c.recentBytes),
    fastest: fastestFirst(candidates)[0],
  };

  const targets: QuickTarget<T>[] = [];
  for (const kind of ORDER) {
    const pick = picks[kind];
    if (!pick) continue;
    const same = targets.find((t) => t.candidate === pick);
    if (same) same.kinds.push(kind);
    else targets.push({ candidate: pick, kinds: [kind] });
  }
  return targets;
}

/** Configs whose last test passed, lowest latency first. */
export function fastestFirst<T>(candidates: Candidate<T>[]): Candidate<T>[] {
  return candidates
    .filter((c) => c.latency !== null && c.latency >= 0)
    .sort((a, b) => (a.latency ?? 0) - (b.latency ?? 0));
}

/** The highest-scoring candidate; the earliest wins a tie, so the list's order breaks it. */
function best<T>(candidates: Candidate<T>[], score: (c: Candidate<T>) => number): Candidate<T> | undefined {
  let top: Candidate<T> | undefined;
  for (const candidate of candidates) {
    if (!top || score(candidate) > score(top)) top = candidate;
  }
  return top;
}
