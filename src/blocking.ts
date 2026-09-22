/**
 * The ad blocker and the anti-tracker, as the frontend sees them: which list each switch needs,
 * when a list is due for fetching, and what the switch's row says.
 *
 * The lists themselves are the Rust side's (`blocklists.rs`): it downloads them, has the core
 * check them and keeps them on disk, and a switch whose list is not there yet is left out of the
 * config rather than failing the connection. So a switch can be on while blocking nothing, and
 * the row says so — "not downloaded yet" — instead of letting "on" stand for a promise nobody is
 * keeping.
 *
 * Nothing here imports anything, so it runs under `node --test`.
 */

export type BlockList = "ads" | "trackers";

export const BLOCK_LISTS: readonly BlockList[] = ["ads", "trackers"];

/** How messages name each list, as `List::name` in `blocklists.rs` does. */
export const LIST_NAMES: Record<BlockList, string> = { ads: "ad", trackers: "tracker" };

/** The setting that switches each list on. */
export const SWITCH = { ads: "blockAds", trackers: "blockTrackers" } as const;

export interface BlockSwitches {
  blockAds: boolean;
  blockTrackers: boolean;
}

export interface ListState {
  /** When the list on disk was fetched (ms since the epoch); `null` when there is none. */
  updatedAt: number | null;
  /** A fetch is in flight. */
  busy: boolean;
  /** Why the last fetch failed; cleared by the next one that succeeds. */
  error: string | null;
}

/**
 * A list older than this is fetched again. HaGeZi's lists say they expire in eight hours; a day is
 * close enough for lists that change by a few names, and it keeps the request count down.
 */
export const STALE_MS = 24 * 60 * 60 * 1000;

/** The switched-on lists that are missing or stale and not already being fetched. */
export function listsToFetch(
  switches: BlockSwitches,
  states: Record<BlockList, ListState>,
  now: number,
): BlockList[] {
  return BLOCK_LISTS.filter((list) => {
    const state = states[list];
    if (!switches[SWITCH[list]] || state.busy) return false;
    return state.updatedAt === null || now - state.updatedAt > STALE_MS;
  });
}

/** What the switch's row says about its list. */
export function listLine(on: boolean, state: ListState, now: number): string {
  if (state.busy) return state.updatedAt === null ? "downloading the list…" : "updating the list…";
  if (state.updatedAt === null) {
    if (!on) return "the list is downloaded when you turn it on";
    // Said plainly: the switch is on and nothing is blocked yet.
    return state.error
      ? `not blocking yet: the list could not be downloaded (${state.error}); it is tried again through the tunnel when you connect`
      : "not blocking yet: the list has not been downloaded";
  }
  const age = `list updated ${ago(now - state.updatedAt)}`;
  // A stale list still blocks; the failure to refresh it is worth a mention, not an alarm.
  return state.error ? `${age}; the last update failed (${state.error})` : age;
}

function ago(ms: number): string {
  const minutes = Math.floor(ms / 60_000);
  if (minutes < 1) return "just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.floor(hours / 24);
  return days === 1 ? "yesterday" : `${days} days ago`;
}
