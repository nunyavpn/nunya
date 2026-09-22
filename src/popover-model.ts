/**
 * What the main window tells the menu-bar popover, and what the popover asks of it.
 *
 * The popover is a second webview with no store of its own (`popover.rs`), so this is the whole
 * contract between the two: the main window builds a `PopoverModel` from its state and sends it on
 * every change (`syncPopover` in `main.ts`), and the popover sends back a `PopoverIntent` for
 * everything it wants done. The model carries what is shown and nothing more — names, places,
 * words — never a profile, so a server's credentials do not travel to a second webview that has
 * no use for them.
 *
 * Types only, so either side can import it without pulling in the other.
 */

import type { BlockList } from "./blocking";
import type { QuickKind } from "./quick";
import type { Shield } from "./shield";
import type { ConnectionState } from "./views/status";

/** A config as the popover lists it. */
export interface ServerLine {
  id: string;
  name: string;
  /** ISO country code, for the flag. */
  country: string;
  /** "Helsinki, Finland" — located the way the list locates it (`located`). */
  place: string;
  latency: string;
  grade: "good" | "mid" | "bad" | "none";
  bars: 0 | 1 | 2 | 3;
}

export interface QuickLine {
  kind: QuickKind;
  label: string;
  glyph: string;
  /** The config it would connect to, or `null` when the choice has no answer. */
  server: ServerLine | null;
  /** Why this config: its latency, its usage, when it was last used; or what is missing. */
  reason: string;
  /** Re-testing a stale Fastest before connecting. */
  busy: boolean;
  /** Why the last pick did not connect. */
  note: string | null;
  /** The tunnel is running on it already. */
  current: boolean;
}

export interface BlockLine {
  list: BlockList;
  label: string;
  on: boolean;
  /** Where its list stands (`listLine`). */
  line: string;
}

export interface PopoverModel {
  connection: ConnectionState;
  mode: "proxy" | "vpn";
  shield: Shield;
  /** The status card's headline, which is held to the modes' rule. */
  headline: string;
  /** Why Connect is refused, or why the last attempt failed. */
  problem: string | null;
  /** What the running connection covers, in the modes' words; the step under way while it works. */
  covers: string;
  connectedAt: number | null;
  /** The public address the tunnel comes out of, once measured. */
  exit: string | null;
  server: ServerLine | null;
  canConnect: boolean;
  quick: QuickLine[];
  block: BlockLine[];
  /** What the blockers reach in this mode. */
  blockNote: string;
  /** The search the popover asked for, answered; `null` while its box is empty. */
  search: { query: string; results: ServerLine[]; more: number } | null;
}

export type PopoverIntent =
  /**
   * Send the model now, whether or not it changed: `shown` when the popover has just been shown,
   * and not when it has only loaded — hidden, it needs one model to be ready, not one a second.
   */
  | { kind: "hello"; shown: boolean }
  | { kind: "toggle" }
  | { kind: "quick"; pick: QuickKind }
  | { kind: "select"; id: string }
  | { kind: "mode"; mode: "proxy" | "vpn" }
  | { kind: "block"; list: BlockList; on: boolean }
  | { kind: "search"; query: string };
