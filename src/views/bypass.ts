/**
 * The bypass rules panel: split tunnelling reduced to one list.
 *
 * The Qt build models this as route profiles with thirteen rule types and four outbound targets.
 * Almost everyone only ever wants one thing from it — keep *these* out of the tunnel — so that is
 * all this offers.
 *
 * There is deliberately no inverse ("route only these through the VPN"). Two directions means every
 * rule has to be read twice, and the inverse quietly turns a tunnel back into a proxy, which is the
 * model this client exists to avoid.
 */

import { classifyBypass, store, type BypassKind, type BypassRule } from "../store";
import { h, render } from "../dom";
import { icon } from "./icons";

const KIND_LABEL: Record<BypassKind, string> = {
  domain: "Domain",
  address: "Address",
  range: "Range",
};

/** Ranges the tunnel already leaves alone, shown so nobody re-adds them and wonders why nothing changed. */
const ALWAYS_DIRECT = ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "169.254.0.0/16"];

export class BypassPanel {
  private draft = "";
  /**
   * Whether this is the panel on screen. The panels share one container, so one that re-rendered
   * on every store change regardless would paint itself over whichever panel the user is on.
   */
  active = false;

  constructor(private root: HTMLElement) {
    store.subscribe(() => this.active && this.render());
  }

  private get kind(): BypassKind | null {
    return classifyBypass(this.draft);
  }

  private commit() {
    const kind = this.kind;
    if (!kind) return;
    store.addBypass(kind, this.draft.trim());
    this.draft = "";
    this.render();
    this.root.querySelector<HTMLInputElement>(".rule-input input")?.focus();
  }

  render() {
    const rules = store.get().bypass;

    render(
      this.root,
      h(
        "div",
        { class: "sheet-head" },
        h("span", { class: "t" }, "Bypass rules"),
      ),
      h(
        "p",
        { class: "rules-note" },
        "Anything matching a rule below leaves on your normal connection. Everything else goes " +
          "through the tunnel.",
      ),
      this.input(),
      h("p", { class: "rlabel" }, "Always bypassed"),
      this.locked(),
      h(
        "p",
        { class: "rlabel" },
        "Your rules",
        h("span", {}, `${rules.length} rule${rules.length === 1 ? "" : "s"}`),
      ),
      h("div", { class: "rlist" }, ...(rules.length ? rules.map((r) => this.row(r)) : [this.empty()])),
    );
  }

  private input() {
    const kind = this.kind;

    const field = h("input", {
      type: "text",
      class: "txt",
      spellcheck: false,
      placeholder: "Domain, address or range",
      value: this.draft,
      oninput: (e: Event) => {
        this.draft = (e.target as HTMLInputElement).value;
        // Re-rendering the whole panel would steal focus mid-typing, so only the chip and the
        // button state change here.
        this.syncAffordances();
      },
      onkeydown: (e: Event) => {
        if ((e as KeyboardEvent).key === "Enter") this.commit();
      },
    });

    return h(
      "div",
      { class: "rule-input" },
      field,
      // The chip names what was detected before the rule is committed, so there is no type picker
      // to get wrong.
      h("span", { class: `kind${kind ? "" : " idle"}` }, kind ? KIND_LABEL[kind] : "—"),
      h(
        "button",
        {
          class: "addrule",
          "aria-label": "Add rule",
          disabled: !kind,
          onclick: () => this.commit(),
        },
        icon("plus", 14),
      ),
    );
  }

  /** Updates just the chip and the button, so typing never loses the caret. */
  private syncAffordances() {
    const kind = this.kind;
    const chip = this.root.querySelector<HTMLElement>(".rule-input .kind");
    const button = this.root.querySelector<HTMLButtonElement>(".addrule");
    if (chip) {
      chip.textContent = kind ? KIND_LABEL[kind] : "—";
      chip.classList.toggle("idle", !kind);
    }
    if (button) button.disabled = !kind;
  }

  private locked() {
    return h(
      "div",
      { class: "rlock" },
      h("span", { class: "rmark lock" }, icon("lock", 13)),
      h(
        "span",
        { class: "lk" },
        h("b", {}, "Your local network"),
        // Not a rule the user chose: it is how the tunnel is already built.
        h("span", {}, ALWAYS_DIRECT.join(" · ")),
      ),
    );
  }

  private row(rule: BypassRule) {
    return h(
      "div",
      { class: "rrow" },
      h("span", { class: "rmark" }, icon(rule.kind === "domain" ? "globe" : "network", 13)),
      h("span", { class: "rtxt" }, rule.value),
      h("span", { class: "rkind" }, rule.kind),
      h(
        "button",
        {
          class: "rdel",
          "aria-label": `Remove ${rule.value}`,
          onclick: () => store.removeBypass(rule.id),
        },
        icon("close", 13),
      ),
    );
  }

  private empty() {
    return h(
      "p",
      { class: "gempty" },
      "Nothing is bypassed yet. Everything goes through the tunnel.",
    );
  }
}
