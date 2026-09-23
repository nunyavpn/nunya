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
  ) {}

  render(model: StatusModel) {
    render(
      this.root,
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
    if (!places.length && !model.blockedReason) return null as unknown as Node;
    return h(
      "div",
      { class: "st-more" },
      ...places,
      model.blockedReason ? h("span", { class: "tag warn" }, model.blockedReason) : null,
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
