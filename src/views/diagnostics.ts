/**
 * The diagnostics panel: the core's log, the transport's state, and the generated config.
 *
 * Kept as a real screen rather than a debug hatch. When a tunnel misbehaves the useful question is
 * "what did the core actually say", and a user who can answer that can file a report worth reading.
 * Showing the generated config matters for the same reason — a config nobody can inspect is a
 * config nobody can trust.
 */

import { h, render } from "../dom";
import { icon } from "./icons";

export interface DiagnosticsModel {
  /** Where the data is kept, so it is clear whether changes are surviving a restart. */
  storage: string;
  storageError: string | null;
  lines: string[];
  transport: string;
  transportState: string;
  transportDetail: string | null;
  coreConnected: boolean;
}

export interface DiagnosticsCallbacks {
  onClear: () => void;
  /** Renders the config the current selection would produce, without starting anything. */
  onPreviewConfig: () => Promise<string>;
}

export class DiagnosticsPanel {
  private model: DiagnosticsModel = {
    storage: "unknown",
    storageError: null,
    lines: [],
    transport: "unknown",
    transportState: "unknown",
    transportDetail: null,
    coreConnected: false,
  };

  constructor(
    private root: HTMLElement,
    private callbacks: DiagnosticsCallbacks,
  ) {}

  /** Whether this is the panel on screen; see `BypassPanel.active`. */
  active = false;

  update(model: DiagnosticsModel) {
    this.model = model;
    // Not `root.hidden`: the container is shared, and it is visible whenever any panel is.
    if (this.active) this.render();
  }

  render() {
    const m = this.model;

    render(
      this.root,
      h("div", { class: "sheet-head" }, h("span", { class: "t" }, "Diagnostics")),
      h(
        "div",
        { class: "badges" },
        h(
          "span",
          { class: `badge ${m.coreConnected ? "ok" : "bad"}` },
          `core: ${m.coreConnected ? "connected" : "not running"}`,
        ),
        h(
          "span",
          { class: `badge ${m.transportState === "disconnected" ? "ok" : "bad"}` },
          `${m.transport}: ${m.transportState}`,
        ),
        h(
          "span",
          { class: `badge ${m.storageError ? "bad" : "ok"}` },
          `storage: ${m.storageError ? "failing" : m.storage}`,
        ),
      ),
      m.storageError ? h("p", { class: "diag-detail" }, m.storageError) : null,
      m.transportDetail ? h("p", { class: "diag-detail" }, m.transportDetail) : null,
      h(
        "div",
        { class: "diag-actions" },
        h("button", { class: "ghost", onclick: () => void this.showConfig() }, "Show config"),
        h("button", { class: "ghost", onclick: () => this.callbacks.onClear() }, "Clear log"),
        h("button", { class: "ghost", onclick: () => void this.copyLog() }, "Copy log"),
      ),
      this.logView(m.lines),
    );
  }

  private logView(lines: string[]) {
    const pre = h("pre", { class: "log" }, lines.join("\n") || "Nothing logged yet.");
    // Queue the scroll: the node has no height until it is in the document.
    queueMicrotask(() => {
      pre.scrollTop = pre.scrollHeight;
    });
    return pre;
  }

  private async showConfig() {
    try {
      const json = await this.callbacks.onPreviewConfig();
      const view = this.root.querySelector<HTMLElement>(".log");
      if (view) {
        view.textContent = json;
        view.scrollTop = 0;
      }
    } catch (e) {
      const view = this.root.querySelector<HTMLElement>(".log");
      if (view) view.textContent = `Could not build a config: ${String(e)}`;
    }
  }

  private async copyLog() {
    try {
      await navigator.clipboard.writeText(this.model.lines.join("\n"));
    } catch {
      // Clipboard access can be refused; the log is selectable either way.
    }
  }
}

/** The icon the rail uses for this panel, kept here so the rail does not import the view. */
export const diagnosticsIcon = () => icon("activity", 18);
