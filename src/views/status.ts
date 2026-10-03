/**
 * The status card over the map.
 *
 * The headline is a claim only a TUN-only client can honestly make: "you're protected" is about the
 * whole device, not one browser. A proxy-mode client cannot say it, which is why it is the sentence
 * this app is built around.
 *
 * The quiet chip row underneath is the compromise with the other audience. Neither NordVPN nor
 * ExpressVPN shows an exit IP, but a v2ray user checks it constantly, so it is present and
 * deliberately understated.
 */

import { elapsed, rate } from "../format";
import { h, render } from "../dom";
import { describe } from "../share";
import { place } from "../geo";
import { icon } from "./icons";
import { CDN_NAMES, cdnOf, located, type Mode, type Server } from "../store";

/**
 * Where the tunnel is. Connecting and disconnecting are states of their own because each takes
 * seconds — the core, then the system proxy — and nothing is to be clicked until it is done; see
 * `serial.ts`.
 */
export type ConnectionState = "off" | "connecting" | "on" | "disconnecting";

/** A measured place, as the card shows it: city, country, and who runs the data center. */
export interface PlaceLine {
  city: string | null;
  /** Null when there is no place to name — an anycast edge not yet observed (`edge.ts`). */
  country: string | null;
  org?: string | null;
  asn?: number | null;
}

export interface StatusModel {
  state: ConnectionState;
  /** While connecting or disconnecting, the step under way: "Setting the system proxy…". */
  step?: string | null;
  /** Which mode is running, because it changes what the card may truthfully claim. */
  mode: Mode;
  /** Proxy mode: where the listener is, so the user can point something at it. */
  proxyAddress: string | null;
  server: Server | undefined;
  connectedAt: number;
  uplink: number;
  downlink: number;
  /** The tunnel's public addresses per family, or null while they are being checked. */
  exitIps: {
    ipv4: string | null;
    ipv6: string | null;
    failed?: string;
    cloudflare: { ip: string; country: string | null } | null;
  } | null;
  tunnelDevice: string | null;
  /** Why the tunnel cannot start, when it cannot. */
  blockedReason: string | null;
  /** Not a blocker, a heads-up: what Connect is going to ask for. */
  grantNote?: string | null;
  /** Where the selected config's traffic leaves, once measured. */
  exitAt?: PlaceLine | null;
  /** Where its address is, when that is not where it exits — a CDN edge, a relay's front. */
  entryAt?: PlaceLine | null;
  /** Proxy mode: the desktop's system proxy points at the listener. */
  systemProxy?: boolean;
  /** Proxy mode: the user asked for the system proxy and setting it failed. */
  systemProxyError?: string | null;
}

export interface StatusCallbacks {
  onToggle: () => void;
  /** Share the selected server as a link and QR code. */
  onShare: () => void;
}

/**
 * What the card may say, per mode.
 *
 * "You're protected" is a claim about the whole device, and only a TUN earns it. In proxy mode
 * the same sentence would be false for everything not pointed at the listener — which is most of
 * the machine — so the headline says what is actually true instead: the proxy is running. Getting
 * this wrong is not a copy nit; it is the difference between a user believing their traffic is
 * covered and it not being.
 */
export const HEADLINE: Record<Mode, Record<ConnectionState, string>> = {
  vpn: {
    on: "You're protected",
    connecting: "Connecting…",
    off: "Not connected",
    disconnecting: "Disconnecting…",
  },
  proxy: {
    on: "Proxy running",
    connecting: "Starting…",
    off: "Proxy off",
    disconnecting: "Stopping…",
  },
};

const TOGGLE_LABEL: Record<ConnectionState, string> = {
  off: "Connect",
  connecting: "Connecting…",
  on: "Disconnect",
  disconnecting: "Disconnecting…",
};

export class StatusCard {
  constructor(
    private root: HTMLElement,
    private callbacks: StatusCallbacks,
  ) {
    this.swipe();
  }

  /**
   * On a phone's map tab the card is a bottom sheet, as in any phone app: it floats over the map,
   * follows the finger while dragged, and settles open or closed when let go — on where it was
   * left, or the way it was flicked. Closed, only its first line shows: the state and Connect.
   *
   * It is moved, not rearranged: every part is laid out, and the sheet is pushed down until only
   * its first line (`--peek`) is above the tab bar. Hiding parts instead could not follow a finger,
   * since a part is either there or not. Closed is the stylesheet's own position — its height less
   * the peek — so it is right in any layout without measuring; this sets `--sheet-y` only while a
   * finger holds the sheet, and to 0 when it settles open. That lives on the card's own element,
   * which `render` never replaces, so a sheet left open stays open while the card repaints every
   * second. On the list, and on a desktop, nothing here applies: the stylesheet moves the card
   * only on the map tab, and the handle shows only there.
   */
  private open = false;
  /** Set by a drag, so the click the browser fires when one ends on the handle does not undo it. */
  private dragged = false;

  private onMap(): boolean {
    return this.root.closest(".window.show-map") !== null;
  }

  /** How far down the closed sheet sits, as the stylesheet places it: its height less the peek. */
  private closedOffset(): number {
    const peek = parseFloat(getComputedStyle(this.root).getPropertyValue("--peek")) || 0;
    return Math.max(0, this.root.offsetHeight - peek);
  }

  private place(offset: number) {
    this.root.style.setProperty("--sheet-y", `${offset}px`);
  }

  private setOpen(open: boolean) {
    this.open = open;
    if (open) this.place(0);
    else this.root.style.removeProperty("--sheet-y");
    this.root.querySelector(".st-grab")?.setAttribute("aria-expanded", String(open));
  }

  private swipe() {
    // Where the drag began, the sheet's offset then, and the last move, for the release speed.
    let drag: { y: number; from: number; lastY: number; lastT: number; v: number } | null = null;

    this.root.addEventListener("pointerdown", (e) => {
      if (!this.onMap() || (e.target as Element).closest?.("button:not(.st-grab)")) return;
      const from = this.open ? 0 : this.closedOffset();
      drag = { y: e.clientY, from, lastY: e.clientY, lastT: e.timeStamp, v: 0 };
      this.dragged = false;
      this.root.setPointerCapture(e.pointerId);
      this.root.classList.add("dragging");
    });

    this.root.addEventListener("pointermove", (e) => {
      if (!drag) return;
      const dy = e.clientY - drag.y;
      if (Math.abs(dy) > 6) this.dragged = true;
      // Pixels per millisecond over the latest move, for telling a flick from a slow drag that
      // stopped halfway.
      const dt = e.timeStamp - drag.lastT;
      if (dt > 0) drag.v = (e.clientY - drag.lastY) / dt;
      drag.lastY = e.clientY;
      drag.lastT = e.timeStamp;
      this.place(Math.min(this.closedOffset(), Math.max(0, drag.from + dy)));
    });

    const settle = (e: PointerEvent) => {
      if (!drag) return;
      const dy = e.clientY - drag.y;
      // A finger that stopped before lifting is not a flick, however fast it moved earlier.
      const speed = e.timeStamp - drag.lastT > 80 ? 0 : drag.v;
      const closed = this.closedOffset();
      const at = Math.min(closed, Math.max(0, drag.from + dy));
      drag = null;
      this.root.classList.remove("dragging");
      // A touch that never became a drag puts the sheet back exactly where it rests.
      if (!this.dragged) return this.setOpen(this.open);
      // A flick decides by its direction; a slow drag by which end it was left nearer.
      if (speed < -0.4) this.setOpen(true);
      else if (speed > 0.4) this.setOpen(false);
      else this.setOpen(at < closed / 2);
    };
    this.root.addEventListener("pointerup", settle);
    this.root.addEventListener("pointercancel", settle);
  }

  render(model: StatusModel) {
    render(
      this.root,
      h(
        "button",
        {
          class: "st-grab",
          "aria-label": "More about this connection",
          "aria-expanded": String(this.open),
          onclick: () => {
            if (this.dragged) this.dragged = false;
            else this.setOpen(!this.open);
          },
        },
        h("i"),
      ),
      h("div", { class: "st-top" }, ...this.top(model)),
      model.state === "on" ? this.chips(model) : this.idle(model),
    );
  }

  private top(model: StatusModel): Node[] {
    const [down, downUnit] = rate(model.downlink);
    const [up, upUnit] = rate(model.uplink);
    // Connected, the live measurement is the truth; the saved one is from the last test.
    const at =
      model.state === "on" && model.exitAt?.country
        ? { country: model.exitAt.country, city: model.exitAt.city ?? "" }
        : model.server
          ? located(model.server)
          : null;
    const country = at ? place(at.country) : null;

    const where = at ? [country?.name, at.city].filter(Boolean).join(" · ") : "No server selected";

    const busy = model.state === "connecting" || model.state === "disconnecting";
    // While it works, the card says what it is doing: a step that takes seconds and says nothing
    // reads as a click that did not register, and invites the second click this is here to stop.
    const subtitle = busy && model.step
      ? model.step
      : model.state === "on"
        ? `${where} · ${elapsed(model.connectedAt)}`
        : where;
    // Amber for both: under way, whichever way.
    const tone = busy ? "connecting" : model.state;

    return [
      h(
        "span",
        { class: `st-badge ${tone}` },
        icon(model.state === "on" ? "shield-check" : "shield", 21),
      ),
      h(
        "span",
        { class: "st-text" },
        h("span", { class: `s ${tone}` }, HEADLINE[model.mode][model.state]),
        h("span", { class: "m", "aria-live": "polite" }, subtitle),
      ),
      // Throughput is meaningless with the tunnel down, and an empty pair of zeroes reads as broken.
      model.state === "on"
        ? h(
            "span",
            { class: "st-flow" },
            this.flow("Down", down, downUnit),
            this.flow("Up", up, upUnit),
          )
        : (null as unknown as Node),
      h(
        "button",
        {
          class: "st-share",
          "aria-label": "Share this server",
          title: "Share this server",
          disabled: !model.server,
          onclick: () => this.callbacks.onShare(),
        },
        icon("share", 17),
      ),
      h(
        "button",
        {
          class: `btn${model.state === "on" || model.state === "disconnecting" ? "" : " go"}${busy ? " busy" : ""}`,
          disabled: busy || (!model.server && model.state === "off"),
          "aria-busy": busy ? "true" : undefined,
          onclick: () => this.callbacks.onToggle(),
        },
        busy ? h("span", { class: "btn-spin", "aria-hidden": "true" }) : null,
        TOGGLE_LABEL[model.state],
      ),
    ].filter(Boolean) as Node[];
  }

  private flow(label: string, value: string, unit: string) {
    return h(
      "span",
      { class: "fl" },
      h("span", { class: "k" }, label),
      h("span", { class: "v" }, value, h("small", {}, unit)),
    );
  }

  private chips(model: StatusModel) {
    const p = model.server?.profile;
    const proxy = model.mode === "proxy";

    return h(
      "div",
      { class: "st-more" },
      // In proxy mode this is the first thing the user needs: nothing is covered until something
      // is pointed at it, so the address comes before anything reassuring.
      proxy && model.proxyAddress
        ? h("span", { class: "tag strong" }, `SOCKS / HTTP  ${model.proxyAddress}`)
        : null,
      ...this.places(model),
      // The public addresses answer the question every leak scare starts with. Both families,
      // each only when it exists: through a relay they can leave from different places, and a
      // "what is my IP" page shows both, so a single address here would look like a discrepancy.
      ...(model.exitIps?.failed
        ? [
            h(
              "span",
              { class: "tag warn", title: model.exitIps.failed },
              "No public IP — traffic is not coming out of this server",
            ),
          ]
        : model.exitIps
        ? [
            model.exitIps.ipv4 ? h("span", { class: "tag ip" }, h("b", {}, "IPv4"), model.exitIps.ipv4) : null,
            model.exitIps.ipv6 ? h("span", { class: "tag ip" }, h("b", {}, "IPv6"), model.exitIps.ipv6) : null,
            // Only when it differs: then the server splits its traffic — a Workers proxy relaying
            // Cloudflare-hosted sites through its provider's proxy IP — and a "what is my IP" page
            // on Cloudflare will show this address, not the ones above. When it matches, it is
            // one of the chips already shown and says nothing new.
            this.cloudflareChip(model.exitIps),
          ]
        : [h("span", { class: "tag" }, "checking public IP…")]),
      p ? h("span", { class: "tag" }, describe(p)) : null,
      model.server && cdnOf(model.server)
        ? h("span", { class: "tag" }, `CDN · ${CDN_NAMES[cdnOf(model.server)!]}`)
        : null,
      // A TUN carries the system resolver, so "no leak" is a property of the mode. A local
      // listener carries only what is handed to it: an app that resolves before connecting has
      // already leaked the name, and claiming otherwise here would be the lie the headline
      // avoids.
      // With the system proxy set, most desktop apps follow it — but not all of them, which is
      // why the chip still says who is left out rather than claiming the machine.
      proxy && model.systemProxy ? h("span", { class: "tag" }, "System proxy set") : null,
      proxy && model.systemProxyError
        ? h("span", { class: "tag warn", title: model.systemProxyError }, "System proxy not set")
        : null,
      proxy
        ? h(
            "span",
            { class: "tag warn" },
            model.systemProxy ? "Apps that ignore it are not covered" : "Only apps set to use it",
          )
        : h("span", { class: "tag", html: "DNS <b>no leak</b>" }),
      !proxy && model.tunnelDevice ? h("span", { class: "tag" }, model.tunnelDevice) : null,
    );
  }

  private cloudflareChip(ips: NonNullable<StatusModel["exitIps"]>) {
    const cf = ips.cloudflare;
    if (!cf || cf.ip === ips.ipv4 || cf.ip === ips.ipv6) return null;
    return h(
      "span",
      { class: "tag ip" },
      h("b", {}, "Cloudflare sites"),
      cf.country ? `${cf.ip} · ${cf.country}` : cf.ip,
    );
  }

  /** Disconnected: where the selected config is, and anything stopping Connect. */
  private idle(model: StatusModel) {
    const places = this.places(model);
    if (!places.length && !model.blockedReason && !model.grantNote) return null as unknown as Node;
    return h(
      "div",
      { class: "st-more" },
      ...places,
      model.blockedReason ? h("span", { class: "tag warn" }, model.blockedReason) : null,
      model.grantNote ? h("span", { class: "tag note" }, icon("lock", 11), model.grantNote) : null,
    );
  }

  /**
   * "Exit  Frankfurt am Main, DE · GTHost · AS63023", and "Entry …" when the address is elsewhere.
   *
   * The data center is what tells a Cloudflare edge from a rented server in the same city, which
   * is the difference between a CDN-fronted config and a direct one — the city alone cannot.
   * Before a config has been tested there is no exit, and its address is labelled as such.
   */
  private places(model: StatusModel): Node[] {
    const line = (label: string, p: PlaceLine) => {
      const where = [p.city, p.country].filter(Boolean).join(", ");
      const network = [p.org, p.asn ? `AS${p.asn}` : null].filter(Boolean).join(" · ");
      return h(
        "span",
        { class: "tag ip place" },
        p.country ? h("span", { class: "flag mini", style: `background:${place(p.country).flag}` }) : null,
        h("b", {}, label),
        network ? `${where} · ${network}` : where,
      );
    };
    const out: Node[] = [];
    if (model.exitAt) out.push(line("Exit", model.exitAt));
    if (model.entryAt) out.push(line(model.exitAt ? "Entry" : "Address", model.entryAt));
    return out;
  }
}
