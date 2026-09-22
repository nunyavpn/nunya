/**
 * Builds the menu-bar popover's model (`popover-model.ts`) from the main window's state.
 *
 * Apart from `main.ts` so that the popover can be previewed in a browser against the mock fixture
 * (`popover-preview.ts`) with exactly the model the app sends it.
 *
 * Every sentence the popover shows about coverage comes from somewhere that already holds it to
 * the modes' rule (CLAUDE.md, *Modes*): the headline is the status card's, the shield's label is
 * the rail's, and `covers` never says "device" in proxy mode.
 */

import { BLOCK_LISTS, listLine, SWITCH, type BlockList, type ListState } from "./blocking";
import { bars, latency } from "./format";
import { place } from "./geo";
import type { BlockLine, PopoverModel, QuickLine, ServerLine } from "./popover-model";
import { QUICK_KINDS, type QuickKind } from "./quick";
import type { Shield } from "./shield";
import { located, store, type Server } from "./store";
import { serverMatches } from "./views/locations";
import { OPTION } from "./views/quickpick";
import { HEADLINE } from "./views/status";

/** What only the main window knows: the connection, and what is in flight. */
export interface PopoverState {
  connection: "off" | "connecting" | "on";
  shield: Shield;
  connectedAt: number | null;
  exit: string | null;
  problem: string | null;
  canConnect: boolean;
  quickBusy: QuickKind | null;
  quickNotes: Partial<Record<QuickKind, string>>;
  blockLists: Record<BlockList, ListState>;
  /** What the popover's search box last asked for. */
  query: string;
}

/**
 * Search results sent at most. A list can run to tens of thousands of configs; a popover is for
 * finding one, and "N more" says to narrow the search rather than to scroll.
 */
const RESULTS_MAX = 40;

const BLOCK_LABEL: Record<BlockList, string> = { ads: "Ad blocker", trackers: "Anti-tracker" };

export function serverLine(server: Server): ServerLine {
  const where = located(server);
  const country = where.country ? place(where.country).name : "";
  const { text, grade } = latency(server.latency);
  return {
    id: server.id,
    name: server.profile.name,
    country: where.country,
    place: [where.city, country].filter(Boolean).join(", "),
    latency: grade === "none" || text === "—" ? "—" : `${text} ms`,
    grade,
    bars: bars(server.latency),
  };
}

export function popoverModel(state: PopoverState): PopoverModel {
  const settings = store.settings();
  const selected = store.selected();
  const now = Date.now();
  const address = `${settings.allowLan ? "0.0.0.0" : "127.0.0.1"}:${settings.proxyPort}`;

  const picks = store.quickPicks(now);
  const running = state.connection === "on" ? store.get().selectedServerId : null;
  const quick: QuickLine[] = QUICK_KINDS.map((kind) => {
    const spec = OPTION[kind];
    const pick = picks[kind];
    return {
      kind,
      label: spec.label,
      glyph: spec.glyph,
      server: pick ? serverLine(pick.item) : null,
      reason: pick ? spec.reason(pick) : spec.missing,
      busy: state.quickBusy === kind,
      note: state.quickNotes[kind] ?? null,
      current: Boolean(pick && pick.item.id === running),
    };
  });

  const block: BlockLine[] = BLOCK_LISTS.map((list) => {
    const on = settings[SWITCH[list]];
    return { list, label: BLOCK_LABEL[list], on, line: listLine(on, state.blockLists[list], now) };
  });

  const query = state.query.trim();
  let search: PopoverModel["search"] = null;
  if (query) {
    // Retired configs are kept only while the tunnel runs on one; they are not offered.
    const found = store.get().servers.filter((s) => !s.retired && serverMatches(s, query));
    search = {
      query,
      results: found.slice(0, RESULTS_MAX).map(serverLine),
      more: Math.max(0, found.length - RESULTS_MAX),
    };
  }

  return {
    connection: state.connection,
    mode: settings.mode,
    shield: state.shield,
    headline: HEADLINE[settings.mode][state.connection],
    problem: state.problem,
    // The address first: in proxy mode it is the one thing the user has to act on, and the line
    // is cut at the end when it does not fit.
    covers:
      settings.mode === "vpn" ? "Everything on this device" : `${address} · only apps set to use it`,
    connectedAt: state.connection === "on" ? state.connectedAt : null,
    exit: state.exit,
    server: selected ? serverLine(selected) : null,
    canConnect: state.canConnect,
    quick,
    block,
    blockNote:
      settings.mode === "vpn"
        ? "Blocking applies to everything on this device while connected."
        : "In proxy mode only apps set to use the proxy are filtered.",
    search,
  };
}
