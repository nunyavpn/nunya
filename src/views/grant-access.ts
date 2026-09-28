/**
 * The sheet between Connect and the system's own prompt (macOS's password, Windows' UAC), in VPN
 * mode without the packet tunnel
 * extension (`platform::grant` in `platform/macos.rs`).
 *
 * A password dialog that appears unannounced after a click on Connect looks like something trying
 * to take over the machine, and a user who has learnt to cancel those is right to. So the app says
 * first what it is about to ask for, why, where the password goes, and what the other choice is;
 * macOS asks only once the user has said yes. The shape before this — a Connect that did nothing,
 * and a small "Allow…" beside an amber tag — left most people stuck at the first half.
 *
 * The sheet owns no logic: allowing and switching to proxy mode are `main.ts`'s, passed in. Nor
 * does it know the platform: its words come from the backend's platform layer (`platform::GRANT`),
 * beside the code that makes them true.
 */
import { h } from "../dom";
import type { GrantCopy } from "../features/tunnel";
import { icon } from "./icons";
import { openSheet, sheetHead } from "./sheets";

export interface GrantAccessActions {
  /** Asks the system, then connects. Resolves with why it did not work, or null once it has. */
  allow(): Promise<string | null>;
  /** Connects in proxy mode instead, which needs no privilege. */
  useProxy(): void;
}

export function openGrantAccess(copy: GrantCopy, actions: GrantAccessActions) {
  openSheet((close) => {
    const error = h("p", { class: "grant-error", role: "alert", hidden: true });
    const label = h("span", {}, "Continue");
    const allow = h(
      "button",
      {
        class: "btn go",
        onclick: async () => {
          allow.disabled = true;
          allow.classList.add("busy");
          allow.prepend(h("span", { class: "btn-spin", "aria-hidden": "true" }));
          label.textContent = copy.waiting;
          error.hidden = true;
          const why = await actions.allow();
          if (!why) return close();
          allow.disabled = false;
          allow.classList.remove("busy");
          allow.querySelector(".btn-spin")?.remove();
          label.textContent = "Try again";
          error.textContent = why;
          error.hidden = false;
        },
      },
      label,
    );

    return h(
      "div",
      { class: "app sheet grant", role: "dialog", "aria-label": "Allow VPN mode" },
      sheetHead("Allow VPN mode", close),
      h(
        "div",
        { class: "grant-body" },
        h("span", { class: "grant-badge" }, icon("lock", 22)),
        h("p", { class: "grant-lede" }, copy.lede),
        h("p", {}, copy.why),
        h(
          "ul",
          { class: "grant-facts" },
          ...copy.facts.map((f) => h("li", {}, h("b", {}, f.title), ` ${f.text}`)),
        ),
        error,
        h(
          "p",
          { class: "grant-alt" },
          "Rather not? ",
          h(
            "button",
            {
              class: "linkish",
              onclick: () => {
                close();
                actions.useProxy();
              },
            },
            "Use proxy mode",
          ),
          ` — ${copy.alt}`,
        ),
      ),
      h(
        "div",
        { class: "sheet-foot" },
        h("span", { class: "gpick" }),
        h("button", { class: "ghost", onclick: close }, "Not now"),
        allow,
      ),
    );
  });
}
