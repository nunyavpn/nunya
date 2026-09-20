/**
 * The Advanced panel.
 *
 * Everything the reference consumer VPNs would never show lives behind this one door, with defaults
 * that work unattended. A user who never opens this should get a correct tunnel.
 *
 * Notably absent: a kill switch toggle. In a TUN-only client that is just `strict_route`, which is
 * on by default — it is the behaviour, not a feature to sell. And no protocol picker: the protocol
 * is a property of the server you imported, not something to choose.
 */

import { DEFAULT_SETTINGS, store, type Settings } from "../store";
import { h, render } from "../dom";

export class SettingsPanel {
  constructor(private root: HTMLElement) {
    store.subscribe(() => this.render());
  }

  render() {
    const s = store.settings();

    render(
      this.root,
      h(
        "div",
        { class: "sheet-head" },
        h("span", { class: "t" }, "Advanced"),
        h("span", { class: "warn-pill" }, "Defaults are fine"),
      ),
      h("div", { class: "set-scroll" }, ...this.groups(s)),
    );
  }

  private groups(s: Settings): Node[] {
    return [
      this.group("Tunnel", [
        this.segmented(
          "Network stack",
          "gvisor is slower but more portable",
          [
            ["system", "system"],
            ["gvisor", "gvisor"],
          ],
          s.stack,
          (v) => store.updateSettings({ stack: v as Settings["stack"] }),
        ),
        this.number("MTU", null, s.mtu, 576, 9000, (v) => store.updateSettings({ mtu: v })),
        this.text("Interface address", null, s.ipv4Cidr, (v) =>
          store.updateSettings({ ipv4Cidr: v }),
        ),
        this.toggle(
          "Strict route",
          "blocks anything trying to leave the tunnel",
          s.strictRoute,
          (v) => store.updateSettings({ strictRoute: v }),
        ),
        this.toggle("Carry IPv6", "off leaves v6 on the physical link", s.ipv6, (v) =>
          store.updateSettings({ ipv6: v }),
        ),
      ]),

      this.group("DNS", [
        this.text("Resolver", "queries resolve inside the tunnel", s.dns, (v) =>
          store.updateSettings({ dns: v }),
        ),
      ]),

      this.group("Diagnostics", [
        this.segmented(
          "Log level",
          "debug is very noisy",
          [
            ["error", "error"],
            ["info", "info"],
            ["debug", "debug"],
          ],
          s.logLevel,
          (v) => store.updateSettings({ logLevel: v as Settings["logLevel"] }),
        ),
      ]),

      h(
        "div",
        { class: "set-group" },
        h(
          "button",
          {
            class: "ghost wide",
            onclick: () => store.updateSettings({ ...DEFAULT_SETTINGS }),
          },
          "Reset to defaults",
        ),
      ),
    ];
  }

  private group(title: string, rows: Node[]) {
    return h("div", { class: "set-group" }, h("h4", {}, title), ...rows);
  }

  private label(title: string, hint: string | null) {
    return h(
      "span",
      { class: "slab" },
      h("span", { class: "t" }, title),
      hint ? h("span", { class: "d" }, hint) : null,
    );
  }

  private toggle(title: string, hint: string | null, value: boolean, onChange: (v: boolean) => void) {
    return h(
      "label",
      { class: "srow" },
      this.label(title, hint),
      h("input", {
        type: "checkbox",
        class: "sw-input",
        checked: value,
        onchange: (e: Event) => onChange((e.target as HTMLInputElement).checked),
      }),
      h("span", { class: "sw", "aria-hidden": "true" }),
    );
  }

  private segmented(
    title: string,
    hint: string | null,
    options: [string, string][],
    value: string,
    onChange: (v: string) => void,
  ) {
    return h(
      "div",
      { class: "srow" },
      this.label(title, hint),
      h(
        "span",
        { class: "seg", role: "radiogroup", "aria-label": title },
        ...options.map(([key, text]) =>
          h(
            "button",
            {
              class: key === value ? "on" : "",
              role: "radio",
              "aria-checked": String(key === value),
              onclick: () => onChange(key),
            },
            text,
          ),
        ),
      ),
    );
  }

  private text(title: string, hint: string | null, value: string, onChange: (v: string) => void) {
    // Stacked: an address or a DoH URL needs the whole column, and squeezing it beside a label
    // breaks the label across lines.
    return h(
      "div",
      { class: "srow stack" },
      this.label(title, hint),
      h("input", {
        type: "text",
        class: "val",
        spellcheck: false,
        value,
        // Committed on blur rather than per keystroke: a half-typed CIDR is not a setting worth
        // persisting, and re-rendering mid-edit would fight the caret.
        onchange: (e: Event) => onChange((e.target as HTMLInputElement).value.trim()),
      }),
    );
  }

  private number(
    title: string,
    hint: string | null,
    value: number,
    min: number,
    max: number,
    onChange: (v: number) => void,
  ) {
    return h(
      "div",
      { class: "srow" },
      this.label(title, hint),
      h("input", {
        type: "number",
        class: "val num",
        value: String(value),
        min: String(min),
        max: String(max),
        onchange: (e: Event) => {
          const input = e.target as HTMLInputElement;
          const parsed = Number(input.value);
          // Out-of-range values snap back rather than reaching the config builder.
          if (!Number.isFinite(parsed) || parsed < min || parsed > max) {
            input.value = String(value);
            return;
          }
          onChange(Math.round(parsed));
        },
      }),
    );
  }
}
