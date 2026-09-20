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
import type { Mode, Server } from "../store";

export type ConnectionState = "off" | "connecting" | "on";

export interface StatusModel {
  state: ConnectionState;
  /** Which mode is running, because it changes what the card may truthfully claim. */
  mode: Mode;
  /** Proxy mode: where the listener is, so the user can point something at it. */
  proxyAddress: string | null;
  server: Server | undefined;
  connectedAt: number;
  uplink: number;
  downlink: number;
  exitIp: string | null;
  tunnelDevice: string | null;
  /** Why the tunnel cannot start, when it cannot. */
  blockedReason: string | null;
}

export interface StatusCallbacks {
  onToggle: () => void;
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
const HEADLINE: Record<Mode, Record<ConnectionState, string>> = {
  vpn: {
    on: "You're protected",
    connecting: "Connecting…",
    off: "Not connected",
  },
  proxy: {
    on: "Proxy running",
    connecting: "Starting…",
    off: "Proxy off",
  },
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
      model.state === "on" ? this.chips(model) : this.hint(model),
    );
  }

  private top(model: StatusModel): Node[] {
    const [down, downUnit] = rate(model.downlink);
    const [up, upUnit] = rate(model.uplink);
    const country = model.server ? place(model.server.country) : null;

    const where = model.server
      ? [country?.name, model.server.city].filter(Boolean).join(" · ")
      : "No server selected";

    const subtitle =
      model.state === "on" ? `${where} · ${elapsed(model.connectedAt)}` : where;

    return [
      h(
        "span",
        { class: `st-badge ${model.state}` },
        icon(model.state === "on" ? "shield-check" : "shield", 21),
      ),
      h(
        "span",
        { class: "st-text" },
        h("span", { class: `s ${model.state}` }, HEADLINE[model.mode][model.state]),
        h("span", { class: "m" }, subtitle),
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
          class: `btn${model.state === "on" ? "" : " go"}`,
          disabled: model.state === "connecting" || (!model.server && model.state === "off"),
          onclick: () => this.callbacks.onToggle(),
        },
        model.state === "on" ? "Disconnect" : "Connect",
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
      // The exit IP answers the question every leak scare starts with.
      h("span", { class: "tag" }, model.exitIp ?? "checking exit IP…"),
      p ? h("span", { class: "tag" }, describe(p)) : null,
      // A TUN carries the system resolver, so "no leak" is a property of the mode. A local
      // listener carries only what is handed to it: an app that resolves before connecting has
      // already leaked the name, and claiming otherwise here would be the lie the headline
      // avoids.
      proxy
        ? h("span", { class: "tag warn" }, "Only apps set to use it")
        : h("span", { class: "tag", html: "DNS <b>no leak</b>" }),
      !proxy && model.tunnelDevice ? h("span", { class: "tag" }, model.tunnelDevice) : null,
    );
  }

  private hint(model: StatusModel) {
    if (!model.blockedReason) return null as unknown as Node;
    return h("div", { class: "st-more" }, h("span", { class: "tag warn" }, model.blockedReason));
  }
}
