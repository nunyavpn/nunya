/**
 * The menu-bar popover's panel: the connection and a big switch for it, the server, Quick Connect,
 * a search over the configs, the mode, and the two blockers.
 *
 * It renders a `PopoverModel` and reports what was clicked; it decides nothing. The main window
 * owns the state and answers every intent (`popover-model.ts`), so there is one place that
 * connects, one that picks the fastest server, one that knows what "protected" may mean.
 *
 * The controls are the window's own — `.seg`, `.sw`, `.flag`, the latency bars — so the popover
 * reads as the same app in a smaller frame rather than a second design.
 */

import { h, render } from "../dom";
import { elapsed } from "../format";
import { place } from "../geo";
import type { BlockList } from "../blocking";
import type { PopoverModel, QuickLine, ServerLine } from "../popover-model";
import type { QuickKind } from "../quick";
import { icon } from "./icons";

export interface PopoverCallbacks {
  onToggle: () => void;
  onQuick: (kind: QuickKind) => void;
  onSelect: (id: string) => void;
  onMode: (mode: "proxy" | "vpn") => void;
  onBlock: (list: BlockList, on: boolean) => void;
  onSearch: (query: string) => void;
  onOpen: () => void;
  onQuit: () => void;
}

const BLOCK_GLYPH: Record<BlockList, string> = { ads: "ban", trackers: "eye-off" };

export class PopoverView {
  private model: PopoverModel | null = null;
  /** What is in the search box, which can run ahead of the last answered search. */
  private query = "";

  constructor(
    private root: HTMLElement,
    private callbacks: PopoverCallbacks,
  ) {}

  show(model: PopoverModel) {
    this.model = model;
    this.render();
  }

  /** Once a second while connected, for the timer; cheaper than a whole model. */
  tick() {
    const timer = this.root.querySelector<HTMLElement>(".pv-timer");
    if (timer && this.model?.connectedAt) timer.textContent = elapsed(this.model.connectedAt);
  }

  focusSearch() {
    this.root.querySelector<HTMLInputElement>(".pv-search input")?.focus();
  }

  clearSearch() {
    if (!this.query) return;
    this.query = "";
    this.callbacks.onSearch("");
    this.render();
  }

  private render() {
    const model = this.model;
    if (!model) return;

    // A re-render replaces the search box, so keep what the user was doing in it.
    const box = this.root.querySelector<HTMLInputElement>(".pv-search input");
    const typing = box !== null && document.activeElement === box;
    const caret = typing ? [box.selectionStart, box.selectionEnd] : null;
    const scroll = this.root.querySelector<HTMLElement>(".pv-body")?.scrollTop ?? 0;

    const searching = this.query.trim() !== "";
    render(
      this.root,
      h(
        "div",
        { class: "pv-panel" },
        this.head(),
        h(
          "div",
          { class: "pv-body" },
          this.hero(model),
          this.search(),
          ...(searching ? [this.results(model)] : [this.quick(model), this.settings(model)]),
        ),
        this.foot(),
      ),
    );

    const body = this.root.querySelector<HTMLElement>(".pv-body");
    if (body) body.scrollTop = scroll;
    if (typing) {
      const next = this.root.querySelector<HTMLInputElement>(".pv-search input");
      next?.focus();
      if (next && caret) next.setSelectionRange(caret[0], caret[1]);
    }
  }

  private head() {
    return h(
      "div",
      { class: "pv-head" },
      h("span", { class: "pv-brand" }, "Nunya"),
      h(
        "button",
        {
          class: "pv-icon-btn",
          title: "Open Nunya",
          "aria-label": "Open Nunya",
          onclick: () => this.callbacks.onOpen(),
        },
        icon("open", 16),
      ),
    );
  }

  private hero(model: PopoverModel) {
    const { shield } = model;
    const busy = model.connection === "connecting";
    const on = model.connection === "on";
    const button = on ? "Disconnect" : busy ? "Connecting…" : "Connect";

    return h(
      "section",
      { class: `pv-hero ${shield.tone}` },
      h(
        "div",
        { class: "pv-state" },
        h(
          "span",
          {
            class: `pv-shield logo ${shield.tone}`,
            role: "img",
            "aria-label": shield.label,
            title: shield.label,
          },
          icon(shield.glyph, 20),
        ),
        h(
          "span",
          { class: "pv-state-text" },
          h("span", { class: "pv-headline" }, model.headline),
          h("span", { class: "pv-covers" }, model.covers),
        ),
        on && model.connectedAt
          ? h("span", { class: "pv-timer" }, elapsed(model.connectedAt))
          : null,
      ),
      model.server
        ? h(
            "div",
            { class: "pv-server" },
            flag(model.server.country),
            h(
              "span",
              { class: "pv-server-text" },
              h("span", { class: "pv-name" }, model.server.name),
              h(
                "span",
                { class: "pv-place" },
                [model.server.place, on && model.exit ? `exit ${model.exit}` : null]
                  .filter(Boolean)
                  .join(" · "),
              ),
            ),
            latency(model.server),
          )
        : h(
            "div",
            { class: "pv-server empty" },
            "No server selected — search below, or pick one in Nunya.",
          ),
      model.problem
        ? h("div", { class: "pv-problem" }, icon("shield-alert", 14), h("span", {}, model.problem))
        : null,
      h(
        "button",
        {
          class: `pv-toggle ${on ? "off" : "on"}`,
          disabled: busy || (!on && !model.canConnect),
          onclick: () => this.callbacks.onToggle(),
        },
        icon("power", 16),
        button,
      ),
    );
  }

  private search() {
    return h(
      "label",
      { class: "pv-search" },
      icon("search", 14),
      h("input", {
        type: "search",
        placeholder: "Search configs",
        spellcheck: false,
        value: this.query,
        "aria-label": "Search configs",
        oninput: (e: Event) => {
          this.query = (e.target as HTMLInputElement).value;
          this.callbacks.onSearch(this.query.trim());
          this.render();
        },
      }),
    );
  }

  private results(model: PopoverModel) {
    const answer = model.search;
    // The answer to an older query while a newer one is on its way: say so rather than show it.
    if (!answer || answer.query !== this.query.trim()) {
      return h("div", { class: "pv-results" }, h("div", { class: "pv-empty" }, "Searching…"));
    }
    if (answer.results.length === 0) {
      return h(
        "div",
        { class: "pv-results" },
        h("div", { class: "pv-empty" }, "No config matches."),
      );
    }
    return h(
      "div",
      { class: "pv-results", role: "list" },
      ...answer.results.map((server) =>
        h(
          "button",
          {
            class: `pv-result${model.server?.id === server.id ? " current" : ""}`,
            role: "listitem",
            onclick: () => this.callbacks.onSelect(server.id),
          },
          flag(server.country),
          h(
            "span",
            { class: "pv-server-text" },
            h("span", { class: "pv-name" }, server.name),
            h("span", { class: "pv-place" }, server.place),
          ),
          latency(server),
        ),
      ),
      answer.more > 0
        ? h(
            "div",
            { class: "pv-empty" },
            `${answer.more.toLocaleString()} more — narrow the search, or use the list in Nunya.`,
          )
        : null,
    );
  }

  private quick(model: PopoverModel) {
    const busy = model.quick.some((q) => q.busy);
    return h(
      "section",
      { class: "pv-section" },
      h("h4", {}, "Quick Connect"),
      h("div", { class: "pv-quick" }, ...model.quick.map((q) => this.quickTile(q, busy))),
      ...model.quick
        .filter((q) => q.note)
        .map((q) => h("div", { class: "pv-note" }, q.note)),
    );
  }

  private quickTile(q: QuickLine, anyBusy: boolean) {
    return h(
      "button",
      {
        class: `pv-tile${q.busy ? " busy" : ""}${q.current ? " current" : ""}`,
        disabled: !q.server || q.current || (anyBusy && !q.busy),
        title: q.server
          ? `${q.label}: ${q.server.name} — ${q.reason}`
          : `${q.label}: ${q.reason}`,
        onclick: () => this.callbacks.onQuick(q.kind),
      },
      h("span", { class: "pv-tile-icon" }, icon(q.glyph, 18)),
      h("span", { class: "pv-tile-label" }, q.label),
      h(
        "span",
        { class: "pv-tile-sub" },
        q.busy ? "Re-testing…" : q.current ? "Connected" : q.server ? q.server.name : "—",
      ),
    );
  }

  private settings(model: PopoverModel) {
    return h(
      "section",
      { class: "pv-section" },
      h("h4", {}, "Protection"),
      h(
        "div",
        { class: "pv-row" },
        h("span", { class: "pv-row-icon" }, icon("network", 16)),
        // No subtitle: what the mode covers is the connection's own second line, just above.
        h("span", { class: "pv-row-text" }, h("span", { class: "pv-row-title" }, "Mode")),
        h(
          "span",
          { class: "seg", role: "radiogroup", "aria-label": "Mode" },
          ...(["proxy", "vpn"] as const).map((mode) =>
            h(
              "button",
              {
                class: mode === model.mode ? "on" : "",
                role: "radio",
                "aria-checked": String(mode === model.mode),
                disabled: model.connection === "connecting",
                onclick: () => mode !== model.mode && this.callbacks.onMode(mode),
              },
              mode === "vpn" ? "VPN" : "Proxy",
            ),
          ),
        ),
      ),
      ...model.block.map((b) =>
        h(
          "label",
          { class: "pv-row" },
          h("span", { class: "pv-row-icon" }, icon(BLOCK_GLYPH[b.list], 16)),
          h(
            "span",
            { class: "pv-row-text" },
            h("span", { class: "pv-row-title" }, b.label),
            h("span", { class: "pv-row-sub" }, b.line),
          ),
          h("input", {
            type: "checkbox",
            class: "sw-input",
            checked: b.on,
            disabled: model.connection === "connecting",
            onchange: (e: Event) =>
              this.callbacks.onBlock(b.list, (e.target as HTMLInputElement).checked),
          }),
          h("span", { class: "sw", "aria-hidden": "true" }),
        ),
      ),
      h("p", { class: "pv-note" }, model.blockNote),
    );
  }

  private foot() {
    return h(
      "div",
      { class: "pv-foot" },
      h("button", { class: "pv-link", onclick: () => this.callbacks.onOpen() }, "Open Nunya"),
      h(
        "button",
        { class: "pv-link", onclick: () => this.callbacks.onQuit() },
        icon("power", 13),
        "Quit",
      ),
    );
  }
}

function flag(country: string) {
  const known = place(country);
  return country
    ? h("span", {
        class: "flag",
        style: `background:${known.flag}`,
        title: known.name,
        "aria-hidden": "true",
      })
    : h("span", { class: "flag dim", "aria-hidden": "true" });
}

/** Bars first and colour second, as the list has it, so the ranking survives greyscale. */
function latency(server: ServerLine) {
  return h(
    "span",
    { class: "pv-lat" },
    h(
      "span",
      { class: `bars s${server.bars} ping ${server.grade}` },
      h("i", {}),
      h("i", {}),
      h("i", {}),
    ),
    h("span", { class: `ping ${server.grade}` }, server.latency),
  );
}
