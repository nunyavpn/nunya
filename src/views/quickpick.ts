/**
 * Quick Connect's prompt: fastest, most used, most recent — each saying what it would connect to.
 *
 * All three are always listed. One with no answer is disabled and says what is missing, rather
 * than left out: a prompt that offered two choices one day and three the next would leave the user
 * wondering where the third went. The config each would pick is named with its flag and group,
 * because "Fastest" alone does not say where the user is about to appear.
 *
 * This only renders. What each choice picks is `quick.ts`; choosing, re-testing and connecting are
 * `main.ts`.
 */

import { h } from "../dom";
import { ago, size } from "../format";
import { place } from "../geo";
import { QUICK_KINDS, RECENT_DAYS, type Candidate, type QuickKind } from "../quick";
import { located, type Server } from "../store";
import { icon } from "./icons";

export interface QuickOptionsView {
  picks: Record<QuickKind, Candidate<Server> | undefined>;
  /** The choice being worked on before it can connect: Fastest, re-testing old results. */
  busy: QuickKind | null;
  /** A line under a choice: why choosing it did not connect. */
  notes: Partial<Record<QuickKind, string>>;
  /** The config the tunnel is running on, if it is. */
  current: string | null;
  groupName: (server: Server) => string;
  onPick: (kind: QuickKind) => void;
}

const OPTION: Record<
  QuickKind,
  {
    label: string;
    glyph: string;
    /** The criterion, for the tooltip. */
    means: string;
    /** Said in place of a config when the choice has no answer. */
    missing: string;
    reason: (c: Candidate<Server>) => string;
  }
> = {
  fastest: {
    label: "Fastest",
    glyph: "bolt",
    means: "The lowest latency of the configs that passed their last test.",
    missing: "No config has passed a test yet.",
    reason: (c) => `${c.latency} ms`,
  },
  mostUsed: {
    label: "Most used",
    glyph: "chart",
    means: `The config that carried the most data in the last ${RECENT_DAYS} days.`,
    missing: `Nothing carried in the last ${RECENT_DAYS} days.`,
    reason: (c) => size(c.recentBytes),
  },
  recent: {
    label: "Most recent",
    glyph: "clock",
    means: "The config the tunnel ran on last.",
    missing: "You have not connected yet.",
    reason: (c) => ago(c.lastConnectedAt),
  },
};

export function quickOptions(view: QuickOptionsView): HTMLElement {
  return h("div", { class: "qpicks" }, ...QUICK_KINDS.map((kind) => option(kind, view)));
}

function option(kind: QuickKind, view: QuickOptionsView) {
  const spec = OPTION[kind];
  const pick = view.picks[kind];
  const server = pick?.item;
  const busy = view.busy === kind;
  const connected = Boolean(server && server.id === view.current);
  const note = view.notes[kind];

  const reason = busy ? "Re-testing…" : connected ? "Connected" : pick ? spec.reason(pick) : "";
  const country = server ? place(located(server).country) : null;

  return h(
    "button",
    {
      class: `qpick${busy ? " busy" : ""}`,
      title: spec.means,
      // Nothing to connect to, already connected to it, or another choice is working.
      disabled: !pick || connected || (view.busy !== null && !busy),
      "aria-busy": busy ? "true" : undefined,
      onclick: () => view.onPick(kind),
    },
    h("span", { class: "qp-icon" }, icon(spec.glyph, 17)),
    h("span", { class: "qp-title" }, spec.label),
    h("span", { class: "qp-reason" }, reason),
    server && country
      ? h(
          "span",
          { class: "qp-sub" },
          h("span", { class: "flag", style: `background:${country.flag}`, "aria-hidden": "true" }),
          h("span", { class: "qp-name" }, server.profile.name),
          h("span", { class: "qp-group" }, view.groupName(server)),
        )
      : h("span", { class: "qp-sub" }, spec.missing),
    note ? h("span", { class: "qp-note" }, note) : null,
  );
}
