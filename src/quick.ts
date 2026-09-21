/**
 * What Quick Connect offers: the fastest config, the one used most, and the one used most recently.
 *
 * It used to offer only the fastest, which is rarely what a returning user wants — they want what
 * they used yesterday, or what they use every day — and "fastest" could pick a config they had
 * never tried, from a subscription they do not trust. Quick Connect now opens a prompt with all
 * three, and the user picks by what they care about this time.
 *
 * - **Fastest**: the lowest latency among configs whose last test passed.
 * - **Most used**: the most traffic in the last `RECENT_DAYS`, from the usage history. Bytes
 *   rather than sessions or hours because that is what is recorded; it favours the config that
 *   carries the heavy traffic, which is usually the one that works.
 * - **Most recent**: the config the tunnel last ran on (`Server.lastConnectedAt`, set on connect).
 *   Not the selection: selecting a row is not connecting to it.
 *
 * Each choice is answered on its own, so two may name the same config; the prompt is a choice of
 * criterion, and each option has to say what it would connect to.
 *
 * Pure and generic over the item, so it runs under `node --test` on plain objects; the store turns
 * servers into candidates.
 */

export type QuickKind = "fastest" | "mostUsed" | "recent";

/** The prompt's order. */
export const QUICK_KINDS: readonly QuickKind[] = ["fastest", "mostUsed", "recent"];

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

/** Each choice's config, or `undefined` when nothing qualifies — a new install has no history. */
export function quickPicks<T>(candidates: Candidate<T>[]): Record<QuickKind, Candidate<T> | undefined> {
  return {
    fastest: fastestFirst(candidates)[0],
    mostUsed: best(candidates.filter((c) => c.recentBytes > 0), (c) => c.recentBytes),
    recent: best(candidates.filter((c) => c.lastConnectedAt !== null), (c) => c.lastConnectedAt ?? 0),
  };
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
