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
import { place } from "../geo";
import { icon } from "./icons";
import type { Server } from "../store";

export type ConnectionState = "off" | "connecting" | "on";

export interface StatusModel {
  state: ConnectionState;
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

const HEADLINE: Record<ConnectionState, string> = {
  on: "You're protected",
  connecting: "Connecting…",
  off: "Not connected",
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
        h("span", { class: `s ${model.state}` }, HEADLINE[model.state]),
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
    const security = p?.tls.reality ? "Reality" : p?.tls.enabled ? "TLS" : "no TLS";

    return h(
      "div",
      { class: "st-more" },
      // The exit IP answers the question every leak scare starts with.
      h("span", { class: "tag" }, model.exitIp ?? "checking exit IP…"),
      h("span", { class: "tag" }, `VLESS · ${security}`),
      h("span", { class: "tag", html: "DNS <b>no leak</b>" }),
      model.tunnelDevice ? h("span", { class: "tag" }, model.tunnelDevice) : null,
    );
  }

  private hint(model: StatusModel) {
    if (!model.blockedReason) return null as unknown as Node;
    return h("div", { class: "st-more" }, h("span", { class: "tag warn" }, model.blockedReason));
  }
}
