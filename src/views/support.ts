/**
 * The Support panel: Buy Me a Coffee and crypto wallets, the only two ways to donate to Nunya.
 *
 * Buy Me a Coffee is a plain link opened in the system browser — never its widget or button
 * image, which the CSP forbids and which would tell a third party whenever someone opened this
 * panel. Each wallet shows its full address rather than a shortened one, because a shortened
 * address is exactly what an address-swapping scam counts on nobody checking; with a Copy button
 * and a QR code for paying from a phone.
 *
 * A channel not set up yet shows as coming — the page's button says "Coming soon" and opens
 * nothing, a wallet lists its coin and network with no address — so the panel says support exists
 * without offering anything that could be paid by mistake.
 *
 * The channels come from `support.ts`; opening the page and copying go through `main.ts`.
 */

import { h, render } from "../dom";
import { acceptsDonations, NETWORKS, payable, type Support, type Wallet } from "../support";
import { icon } from "./icons";
import { qrCode } from "./qr";

export interface SupportCallbacks {
  /** Opens a page in the system browser. */
  onOpen: (url: string) => void;
  /** Copies text; resolves to whether it worked. */
  onCopy: (text: string) => Promise<boolean>;
}

export class SupportPanel {
  /** Whether this is the panel on screen; see `BypassPanel.active`. */
  active = false;
  /** Wallets whose QR code is showing, by address. Folded by default: the list stays scannable. */
  private shownQr = new Set<string>();

  constructor(
    private root: HTMLElement,
    private support: Support,
    private callbacks: SupportCallbacks,
  ) {}

  render() {
    const { buyMeACoffee, wallets } = this.support;
    const open = acceptsDonations(this.support);
    render(
      this.root,
      h("div", { class: "sheet-head" }, h("span", { class: "t" }, "Support Nunya")),
      h(
        "div",
        { class: "support-body" },
        h(
          "p",
          { class: "support-intro" },
          "Nunya is free and open source. If it keeps you connected, you can help keep it going.",
        ),
        buyMeACoffee ? this.coffee(buyMeACoffee) : null,
        wallets.length
          ? h("div", { class: "set-group" }, h("h4", {}, "Crypto"), ...wallets.map((w) => this.wallet(w)))
          : null,
        h(
          "p",
          { class: "fnote" },
          !buyMeACoffee && !wallets.length
            ? "Donations aren't set up in this build."
            : open
              ? "These are the only ways to donate to Nunya. Anyone asking for payment in Nunya's name " +
                "anywhere else is not us. A donation is a gift to the project; it doesn't change how " +
                "the app works for you."
              : "Donations aren't open yet. When they are, these will be the only ways to donate to " +
                "Nunya — anyone asking for payment in Nunya's name before then, or anywhere else, is not us.",
        ),
      ),
    );
  }

  private coffee({ url, live }: { url: string; live: boolean }) {
    const page = url.replace(/^https:\/\//, "").replace(/\/$/, "");
    return h(
      "div",
      { class: "set-group" },
      h("h4", {}, "Buy Me a Coffee"),
      h(
        "button",
        {
          class: "btn brand support-coffee",
          disabled: !live,
          onclick: () => live && this.callbacks.onOpen(url),
        },
        icon("coffee", 17),
        live ? "Buy me a coffee" : "Coming soon",
      ),
      h("p", { class: "fnote" }, live ? `${page} · opens in your browser` : "The page isn't open yet."),
    );
  }

  private wallet(wallet: Wallet) {
    const network = NETWORKS[wallet.network];
    // Nothing to copy, scan or send to: a placeholder says what is coming and nothing more.
    if (!payable(wallet)) {
      return h(
        "div",
        { class: "wallet soon" },
        h("div", { class: "wallet-head" }, h("b", {}, wallet.coin), h("span", { class: "wallet-net" }, network.name)),
        h("p", { class: "wallet-soon" }, "Address coming soon"),
      );
    }

    const showing = this.shownQr.has(wallet.address);

    const copy = h("button", { class: "ghost" }, icon("clipboard", 14), "Copy") as HTMLButtonElement;
    let reset = 0;
    copy.onclick = async () => {
      const copied = await this.callbacks.onCopy(wallet.address);
      copy.replaceChildren(icon(copied ? "check" : "clipboard", 14), copied ? "Copied" : "Copy failed");
      window.clearTimeout(reset);
      reset = window.setTimeout(() => copy.replaceChildren(icon("clipboard", 14), "Copy"), 1600);
    };

    return h(
      "div",
      { class: "wallet" },
      h("div", { class: "wallet-head" }, h("b", {}, wallet.coin), h("span", { class: "wallet-net" }, network.name)),
      h("code", { class: "wallet-addr" }, wallet.address),
      h(
        "div",
        { class: "wallet-acts" },
        copy,
        h(
          "button",
          {
            class: "ghost",
            "aria-expanded": String(showing),
            onclick: () => {
              if (showing) this.shownQr.delete(wallet.address);
              else this.shownQr.add(wallet.address);
              this.render();
            },
          },
          icon("scan", 14),
          showing ? "Hide QR" : "QR code",
        ),
      ),
      showing ? h("div", { class: "share-qr wallet-qr" }, qrCode(wallet.address, 176)) : null,
      h("p", { class: "fnote warn" }, `Send only ${wallet.coin} on ${network.name}. Anything else sent here is lost.`),
    );
  }
}
