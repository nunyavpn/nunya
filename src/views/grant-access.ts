/**
 * The sheet between Connect and macOS's password prompt, in VPN mode without the packet tunnel
 * extension (`core_proc::grant_root`).
 *
 * A password dialog that appears unannounced after a click on Connect looks like something trying
 * to take over the machine, and a user who has learnt to cancel those is right to. So the app says
 * first what it is about to ask for, why, where the password goes, and what the other choice is;
 * macOS asks only once the user has said yes. The shape before this — a Connect that did nothing,
 * and a small "Allow…" beside an amber tag — left most people stuck at the first half.
 *
 * The sheet owns no logic: allowing and switching to proxy mode are `main.ts`'s, passed in.
 */
import { h } from "../dom";
import { icon } from "./icons";
import { openSheet, sheetHead } from "./sheets";

export interface GrantAccessActions {
  /** Asks macOS, then connects. Resolves with why it did not work, or null once it has. */
  allow(): Promise<string | null>;
  /** Connects in proxy mode instead, which needs no password. */
  useProxy(): void;
}

export function openGrantAccess(actions: GrantAccessActions) {
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
          label.textContent = "Waiting for macOS…";
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
        h("p", { class: "grant-lede" }, "VPN mode needs your administrator password"),
        h(
          "p",
          {},
          "To send all of this Mac's traffic through the tunnel, Nunya creates a network " +
            "interface, and macOS allows that only with an administrator's approval.",
        ),
        h(
          "ul",
          { class: "grant-facts" },
          h("li", {}, h("b", {}, "macOS asks, not Nunya."), " The password goes to macOS; Nunya never sees it."),
          h("li", {}, h("b", {}, "Once."), " You're asked again only after Nunya updates."),
          h(
            "li",
            {},
            h("b", {}, "What changes."),
            " Nunya's tunnel engine keeps administrator rights, and runs only when Nunya starts it.",
          ),
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
          " — no password, but it covers only apps set to use it.",
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
