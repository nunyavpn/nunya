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

import { listLine, type BlockList, type ListState } from "../blocking";
import { DEFAULT_SETTINGS, store, type Settings } from "../store";
import { h, render } from "../dom";

export interface SettingsCallbacks {
  onDisconnect: () => void;
  /** Where a block list stands: on disk since when, being fetched, or failed. */
  blockList: (list: BlockList) => ListState;
}

export class SettingsPanel {
  /** Whether this is the panel on screen; see `BypassPanel.active`. */
  active = false;
  private locked = false;

  constructor(
    private root: HTMLElement,
    private callbacks: SettingsCallbacks,
  ) {
    store.subscribe(() => this.active && this.render());
  }

  /**
   * Settings are read when the tunnel starts, so changing one while it runs would change nothing
   * until the next connect — or, worse, look applied when it is not. So they are locked while
   * connected (or connecting), and the panel says why and offers the way out.
   */
  setLocked(locked: boolean) {
    if (locked === this.locked) return;
    this.locked = locked;
    if (this.active) this.render();
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
      this.locked
        ? h(
            "div",
            { class: "lockbar" },
            h("span", {}, "Disconnect to change settings."),
            h("button", { class: "ghost", onclick: () => this.callbacks.onDisconnect() }, "Disconnect"),
          )
        : null,
      h(
        "div",
        { class: "set-scroll" },
        // A disabled fieldset disables every control inside it natively — keyboard included —
        // rather than each control having to be told.
        h("fieldset", { class: "set-lock", disabled: this.locked }, ...this.groups(s)),
      ),
    );
  }

  private groups(s: Settings): Node[] {
    return [
      this.group("Mode", [
        this.segmented(
          "Carry traffic with",
          s.mode === "vpn"
            ? "a TUN interface: everything on this device"
            : "a local port: only apps set to use it",
          [
            ["proxy", "Proxy"],
            ["vpn", "VPN"],
          ],
          s.mode,
          (v) => store.updateSettings({ mode: v as Settings["mode"] }),
        ),
        // Only proxy mode has a listener to configure, and showing a port that nothing binds
        // would suggest VPN mode has one too.
        ...(s.mode === "proxy"
          ? [
              this.number("Port", "SOCKS and HTTP on one port", s.proxyPort, 1, 65535, (v) =>
                store.updateSettings({ proxyPort: v }),
              ),
              this.toggle(
                "Allow LAN",
                "lets other machines on the network use it too",
                s.allowLan,
                (v) => store.updateSettings({ allowLan: v }),
              ),
              this.toggle(
                "Set system proxy",
                "points this desktop's proxy setting here while connected, and puts it back after",
                s.systemProxy,
                (v) => store.updateSettings({ systemProxy: v }),
              ),
            ]
          : []),
      ]),

      this.blockingGroup(s),

      // Only VPN mode builds a TUN, so its settings are noise the rest of the time.
      ...(s.mode === "vpn" ? [this.tunnelGroup(s)] : []),

      this.group("DNS", [
        this.text(
          "Resolver",
          // Where the query travels differs by mode, and "inside the tunnel" is only true of one
          // of them. In proxy mode a name an app resolved before connecting was never seen here.
          s.mode === "vpn"
            ? "queries resolve inside the tunnel"
            : "used for names the proxy resolves itself",
          s.dns,
          (v) => store.updateSettings({ dns: v }),
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

  /**
   * Each switch says where its list stands, because a switch can be on while blocking nothing:
   * its list is left out of the config until it has been downloaded (`blocklists.rs`).
   */
  private blockingGroup(s: Settings) {
    const now = Date.now();
    const line = (list: BlockList, on: boolean) => listLine(on, this.callbacks.blockList(list), now);
    return this.group("Blocking", [
      this.toggle(
        "Ad blocker",
        `ad networks · ${line("ads", s.blockAds)}`,
        s.blockAds,
        (v) => store.updateSettings({ blockAds: v }),
      ),
      this.toggle(
        "Anti-tracker",
        `tracking built into systems, devices and apps · ${line("trackers", s.blockTrackers)}`,
        s.blockTrackers,
        (v) => store.updateSettings({ blockTrackers: v }),
      ),
      // What the switches reach follows the mode, like everything else that claims coverage.
      h(
        "p",
        { class: "set-note" },
        s.mode === "vpn"
          ? "Applies to everything on this device while connected."
          : "In proxy mode only apps set to use the proxy are filtered.",
      ),
    ]);
  }

  private tunnelGroup(s: Settings) {
    return this.group("Tunnel", [
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
    ]);
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
