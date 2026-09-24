/**
 * The Share server sheet — a link or a wg-quick config, with a QR code and a copy button — and
 * the clipboard helper it uses, extracted out of `main.ts`'s own "sheets" section per
 * `ENGINEERING_STANDARDS.md`.
 *
 * The link is generated here, from the profile, rather than kept from whatever was pasted: the
 * profile is what the app actually connects with, so an edit made since import is what gets
 * shared. The standard `vless://` / `vmess://` / `trojan://` form is what v2rayNG, Hiddify and
 * Streisand all scan.
 *
 * A WireGuard server can also be shared as a wg-quick config, and opens on it: the official
 * WireGuard apps — most people's phone client for WireGuard — scan only that, not a link. A WARP
 * server says why it cannot be (`wgQuickRefusal`) and opens on the link instead. The config's DNS
 * line comes from the app's DNS setting when that names an address; when it names a host, the
 * sheet says so rather than choosing a resolver for the user.
 *
 * The sheet says plainly that what it shows is the credential. A QR code on screen looks like a
 * harmless picture, and anyone who photographs it can use the server exactly as the user does.
 */
import { h, render } from "../dom";
import { describe, dnsAddressOf, toShareLink, toWgQuick, wgQuickRefusal } from "../share";
import { store, type Server } from "../store";
import { qrCode } from "./qr";
import { openSheet, sheetHead } from "./sheets";

/** What `main.ts` supplies so this module never has to reach back into it directly. */
export interface ShareServerHooks {
  log(line: string): void;
}

let hooks: ShareServerHooks;

/** Must be called once, during boot, before the Share server sheet can be opened. */
export function initShareServer(next: ShareServerHooks): void {
  hooks = next;
}

/**
 * Puts text on the clipboard, and says whether it got there.
 *
 * The async Clipboard API is the right one, but WebKitGTK builds that predate it — or refuse it for
 * the app's origin — reject, so the old selection-and-`execCommand` route is kept as a fallback
 * rather than reporting a copy that did not happen.
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const scratch = h("textarea", { class: "offscreen", readonly: true }) as HTMLTextAreaElement;
    scratch.value = text;
    document.body.append(scratch);
    scratch.select();
    const ok = document.execCommand("copy");
    scratch.remove();
    return ok;
  }
}

export function openShareServer(server: Server) {
  let link: string;
  try {
    link = toShareLink(server.profile);
  } catch (e) {
    hooks.log(`[ui] could not write a share link for ${server.profile.name}: ${String(e)}`);
    return;
  }

  const wireguard = server.profile.protocol === "wireguard";
  const refusal = wireguard ? wgQuickRefusal(server.profile) : null;
  const dnsSetting = store.settings().dns;
  const dns = dnsAddressOf(dnsSetting);
  const config = wireguard && !refusal ? toWgQuick(server.profile, dns ? [dns] : []) : null;
  let format: "link" | "config" = config ? "config" : "link";

  openSheet((close) => {
    const body = h("div", { class: "share-body" });
    const copyButton = h("button", { class: "btn brand" }) as HTMLButtonElement;
    let reset = 0;
    const copyLabel = () => (format === "config" ? "Copy config" : "Copy link");
    copyButton.onclick = async () => {
      const text = format === "config" ? config : link;
      if (!text) return;
      const copied = await copyText(text);
      copyButton.textContent = copied ? "Copied" : "Copy failed";
      window.clearTimeout(reset);
      reset = window.setTimeout(() => (copyButton.textContent = copyLabel()), 1600);
    };

    const textBox = (value: string, label: string, rows: number, extra = "") => {
      const box = h("textarea", {
        class: `val sharelink${extra}`,
        readonly: true,
        rows,
        spellcheck: false,
        "aria-label": label,
        onclick: (e: Event) => (e.target as HTMLTextAreaElement).select(),
      }) as HTMLTextAreaElement;
      box.value = value;
      return box;
    };

    const paint = () => {
      window.clearTimeout(reset);
      copyButton.textContent = copyLabel();
      copyButton.disabled = format === "config" && !config;

      const choice = wireguard
        ? h(
            "span",
            { class: "seg share-format", role: "radiogroup", "aria-label": "Share as" },
            ...(
              [
                ["config", "WireGuard config"],
                ["link", "Link"],
              ] as const
            ).map(([key, text]) =>
              h(
                "button",
                {
                  class: key === format ? "on" : "",
                  role: "radio",
                  "aria-checked": String(key === format),
                  onclick: () => {
                    format = key;
                    paint();
                  },
                },
                text,
              ),
            ),
          )
        : null;

      const shown =
        format === "config"
          ? config
            ? [
                h("div", { class: "share-qr" }, qrCode(config, 248)),
                h("p", { class: "fnote" }, "Scan with the WireGuard app: Add a tunnel, then Create from QR code."),
                // Tall enough for every line, and a row for the horizontal scrollbar a long key may
                // need: the sheet focuses its first text box on open, which puts the caret at the end
                // and would otherwise scroll the [Interface] header out of view.
                textBox(config, "WireGuard config", config.trimEnd().split("\n").length + 1, " wgconf"),
                dns
                  ? null
                  : h(
                      "p",
                      { class: "fnote warn" },
                      `There is no DNS line: your DNS setting (${dnsSetting}) names a host, not an address. ` +
                        "Add one in the WireGuard app, or names may not resolve through the tunnel.",
                    ),
                h(
                  "p",
                  { class: "fnote warn" },
                  "The config contains this server's private key. Anyone who has it can use the server.",
                ),
              ]
            : [h("p", { class: "fnote warn" }, refusal ?? "")]
          : [
              h("div", { class: "share-qr" }, qrCode(link)),
              // Nunya first: it is the client this link is written for. It has no phone app, so a
              // phone is pointed at the kind of client rather than at another product by name.
              h(
                "p",
                { class: "fnote" },
                "Import it in Nunya on another device, or scan it on a phone with a client that reads share links.",
              ),
              textBox(link, "Share link", 4),
              h(
                "p",
                { class: "fnote warn" },
                "The link contains this server's credentials. Anyone who has it can use the server.",
              ),
            ];

      render(body, h("p", { class: "share-name" }, `${server.profile.name} · ${describe(server.profile)}`), choice, ...shown);
    };
    paint();

    return h(
      "div",
      { class: "app sheet share", role: "dialog", "aria-label": "Share server" },
      sheetHead("Share server", close),
      body,
      h(
        "div",
        { class: "sheet-foot" },
        h("span", { class: "gpick" }),
        h("button", { class: "ghost", onclick: close }, "Done"),
        copyButton,
      ),
    );
  });
}
