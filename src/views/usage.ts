/**
 * The usage sheet's contents: totals, a daily chart and, for a subscription, a breakdown by config.
 *
 * The chart is SVG drawn here rather than a chart library, for the reason the frontend has no
 * framework: every shipped byte is something a user has to trust, and thirty bars, two gridlines
 * and a tooltip are a screenful of code. Colours come from the theme's variables, so dark mode
 * needs nothing of its own.
 *
 * This only renders. Where the numbers come from — the tunnel's counters, filed per config per day
 * — is `usage.ts`; opening, refreshing and clearing are `main.ts`.
 */

import { h, svg } from "../dom";
import { size } from "../format";
import type { Quota, Server } from "../store";
import { dayKey, firstDay, lastDays, niceCeiling, total, type Bytes, type DayBytes } from "../usage";

/** Days the chart covers, and the window the headline total is for. */
export const CHART_DAYS = 30;

/** Configs the breakdown lists by name before summing the rest into one line. */
const BREAKDOWN_ROWS = 8;

export interface UsageView {
  /** One config's, or every config in a subscription. */
  servers: Server[];
  /** The provider's own figure for a subscription, shown beside ours because they differ. */
  quota?: Quota | null;
  /** List each config's share: for a subscription, not for a single config. */
  breakdown: boolean;
  now: number;
}

export function usageBody(view: UsageView): HTMLElement {
  const histories = view.servers.map((s) => s.usage);
  const series = lastDays(histories, CHART_DAYS, view.now);
  const recent = total(histories, series[0]?.day);
  const allTime = total(histories);
  const since = firstDay(histories);

  if (!since) {
    return h(
      "div",
      { class: "usage-body" },
      h(
        "p",
        { class: "usage-empty" },
        view.servers.length === 1
          ? "Nothing recorded yet. Usage is counted while the tunnel runs on this config."
          : "Nothing recorded yet. Usage is counted while the tunnel runs on any of these configs.",
      ),
      footnote(),
    );
  }

  return h(
    "div",
    { class: "usage-body" },
    h(
      "div",
      { class: "usage-sum" },
      summary(`Last ${CHART_DAYS} days`, recent),
      summary(`Since ${shortDate(since)}`, allTime),
    ),
    view.quota ? quotaNote(view.quota) : null,
    chart(series, view.now),
    h(
      "div",
      { class: "usage-legend", "aria-hidden": "true" },
      h("span", { class: "down" }, "Download"),
      h("span", { class: "up" }, "Upload"),
    ),
    view.breakdown ? breakdown(view.servers) : null,
    footnote(),
  );
}

function summary(label: string, bytes: Bytes) {
  return h(
    "div",
    { class: "us-cell" },
    h("span", { class: "k" }, label),
    h("b", {}, size(bytes.up + bytes.down)),
    h("span", { class: "s" }, `↓ ${size(bytes.down)} · ↑ ${size(bytes.up)}`),
  );
}

/**
 * The provider's number next to ours, with why they disagree. Theirs counts every device on the
 * subscription and ours only this app, so a gap between them is expected rather than an error.
 */
function quotaNote(quota: Quota) {
  const resets = quota.resetsAt ? `, resets ${shortDate(dayKey(quota.resetsAt))}` : "";
  return h(
    "p",
    { class: "fnote" },
    `Your provider reports ${size(quota.usedBytes)} of ${size(quota.totalBytes)} used${resets}. ` +
      "That counts every device on the subscription; this counts what this app carried.",
  );
}

/** "Sep 21" from a day key, in the user's own month names. */
export function shortDate(day: string): string {
  const [y, m, d] = day.split("-").map(Number);
  return new Date(y, m - 1, d).toLocaleDateString(undefined, { month: "short", day: "numeric" });
}

// ------------------------------------------------------------------------------------ the chart

const W = 440;
const H = 164;
const LEFT = 50;
const RIGHT = 4;
const TOP = 10;
const BOTTOM = 136;

/**
 * Stacked daily bars, download under upload, over a scale that tops out at a round number.
 *
 * Each day carries an invisible full-height target with its tooltip, so a quiet day can be hovered
 * too — otherwise the only days that could answer "how much was that?" are the ones with a bar.
 */
function chart(series: DayBytes[], now: number): SVGElement {
  const peak = Math.max(0, ...series.map((d) => d.up + d.down));
  const top = niceCeiling(peak);
  const plot = BOTTOM - TOP;
  const slot = (W - LEFT - RIGHT) / series.length;
  const bar = Math.max(2, slot * 0.64);
  const y = (bytes: number) => BOTTOM - (bytes / top) * plot;
  // A day that moved anything gets at least a sliver, or a few kilobytes against gigabytes vanish.
  const height = (bytes: number) => (bytes > 0 ? Math.max(1.5, (bytes / top) * plot) : 0);

  const node = svg("svg", {
    class: "usage-chart",
    viewBox: `0 0 ${W} ${H}`,
    role: "img",
    "aria-label": `Daily usage over the last ${series.length} days, up to ${size(top)} a day`,
  });

  for (const fraction of [0, 0.5, 1]) {
    const at = y(top * fraction);
    node.appendChild(svg("line", { class: "uc-grid", x1: LEFT, x2: W - RIGHT, y1: at, y2: at }));
    if (fraction > 0) node.appendChild(label(roundSize(top * fraction), LEFT - 6, at + 3.5, "end"));
  }

  const today = dayKey(now);
  series.forEach((day, i) => {
    const x = LEFT + i * slot + (slot - bar) / 2;
    const down = height(day.down);
    const up = height(day.up);
    const group = svg(
      "g",
      { class: day.day === today ? "uc-day today" : "uc-day" },
      title(`${shortDate(day.day)} · ↓ ${size(day.down)} · ↑ ${size(day.up)}`),
      svg("rect", { class: "uc-hit", x: LEFT + i * slot, y: TOP, width: slot, height: plot }),
    );
    if (down) group.appendChild(svg("rect", { class: "uc-down", x, y: BOTTOM - down, width: bar, height: down, rx: 1 }));
    if (up) group.appendChild(svg("rect", { class: "uc-up", x, y: BOTTOM - down - up, width: bar, height: up, rx: 1 }));
    node.appendChild(group);
  });

  // The first day, the middle and today: enough to place any bar without crowding the axis.
  const middle = Math.floor(series.length / 2);
  const tick = (i: number, text: string, anchor: string) =>
    node.appendChild(label(text, LEFT + i * slot + slot / 2, H - 8, anchor));
  tick(0, shortDate(series[0].day), "start");
  tick(middle, shortDate(series[middle].day), "middle");
  tick(series.length - 1, "Today", "end");

  return node;
}

/** A scale label without the zeros `size` keeps for a steady column: "2 GB", "2.5 GB". */
function roundSize(bytes: number): string {
  return size(bytes).replace(/(\.\d*?)0+ /, "$1 ").replace(/\. /, " ");
}

function label(text: string, x: number, y: number, anchor: string) {
  return svg("text", { class: "uc-axis", x, y, "text-anchor": anchor }, document.createTextNode(text));
}

function title(text: string) {
  return svg("title", {}, document.createTextNode(text));
}

// -------------------------------------------------------------------------------- the breakdown

/** Each config's share, largest first, as bars against the largest. */
function breakdown(servers: Server[]) {
  const rows = servers
    .map((server) => {
      const bytes = total([server.usage]);
      return { server, bytes: bytes.up + bytes.down };
    })
    .filter((row) => row.bytes > 0)
    .sort((a, b) => b.bytes - a.bytes);
  if (!rows.length) return null;

  const largest = rows[0].bytes;
  const shown = rows.slice(0, BREAKDOWN_ROWS);
  const rest = rows.slice(BREAKDOWN_ROWS);
  const restBytes = rest.reduce((sum, row) => sum + row.bytes, 0);

  return h(
    "div",
    { class: "usage-break" },
    h("h4", {}, "By config"),
    ...shown.map((row) =>
      h(
        "div",
        { class: "ub-row" },
        h("span", { class: "ub-name", title: row.server.profile.name }, row.server.profile.name),
        h("span", { class: "ub-bar" }, h("i", { style: `width:${((row.bytes / largest) * 100).toFixed(1)}%` })),
        h("span", { class: "ub-val" }, size(row.bytes)),
      ),
    ),
    rest.length
      ? h(
          "div",
          { class: "ub-row rest" },
          h("span", { class: "ub-name" }, `${rest.length} more`),
          h("span", { class: "ub-bar" }),
          h("span", { class: "ub-val" }, size(restBytes)),
        )
      : null,
  );
}

/** Where the numbers come from and how long they last, said once, where the numbers are. */
function footnote() {
  return h(
    "p",
    { class: "fnote" },
    "Counted on this device from the tunnel's own counters, never sent anywhere, and kept for as " +
      "long as the config is in your list.",
  );
}
