/**
 * Generic sheet plumbing (`openSheet`/`sheetHead`/`confirmSheet`), and the sheets built directly
 * on it that need nothing beyond the store and the tunnel's connection state: Usage and the two
 * delete confirmations — extracted out of `main.ts`'s own "sheets" section per
 * `ENGINEERING_STANDARDS.md`.
 *
 * `openEditServer`, `openShareServer` and `buildAddServers` stay in `main.ts` for now: each pulls
 * in its own larger dependency (the profile editor, QR codes and the clipboard, the add-servers
 * parser) and is a separate future extraction, not this one.
 */
import { h, qs, render } from "../dom";
import { size } from "../format";
import { describe } from "../share";
import { store, type Group, type Server } from "../store";
import { connection, disconnect } from "../features/tunnel";
import { firstDay, total } from "../usage";
import { icon } from "./icons";
import { shortDate, usageBody } from "./usage";

/** What `main.ts` supplies so this module never has to reach back into it directly. */
export interface SheetHooks {
  log(line: string): void;
}

let hooks: SheetHooks;

/** Must be called once, during boot, before any sheet in this module can be opened. */
export function initSheets(next: SheetHooks): void {
  hooks = next;
}

/**
 * Opens a modal over the scrim and hands the builder a way to close it.
 *
 * The dialogs differ only in their contents, so the plumbing — the scrim, click-outside, Escape —
 * lives here once. Escape in particular is why this exists rather than three copies: a modal that
 * traps you until you find the right button is a modal people learn to distrust.
 */
export function openSheet(build: (close: () => void) => Node) {
  const scrim = qs<HTMLElement>("#scrim");

  const onKey = (event: KeyboardEvent) => {
    if (event.key === "Escape") close();
  };
  const onClick = (event: MouseEvent) => {
    if (event.target === scrim) close();
  };

  function close() {
    scrim.hidden = true;
    scrim.replaceChildren();
    // Both are removed, or every sheet ever opened keeps listening for the rest of the session.
    document.removeEventListener("keydown", onKey);
    scrim.removeEventListener("click", onClick);
  }

  scrim.hidden = false;
  render(scrim, build(close));
  // After insertion, not inside the builder: an element outside the document cannot take focus.
  scrim.querySelector<HTMLElement>("textarea, input")?.focus();
  document.addEventListener("keydown", onKey);
  scrim.addEventListener("click", onClick);
}

/** Head and close button, identical across the sheets. */
export function sheetHead(title: string, close: () => void) {
  return h(
    "div",
    { class: "sheet-head" },
    h("span", { class: "t" }, title),
    h("button", { class: "x", "aria-label": "Close", onclick: close }, icon("close", 15)),
  );
}

/**
 * A yes/no sheet for something that cannot be undone.
 *
 * This app has no undo, and what is being deleted is usually a credential — a hand-added server or
 * a subscription URL exists nowhere else. So the sheet states what will be lost rather than asking
 * "are you sure?", which is a question nobody reads.
 */
export function confirmSheet(options: {
  title: string;
  lines: (string | null)[];
  confirmLabel: string;
  onConfirm: () => void;
}) {
  openSheet((close) =>
    h(
      "div",
      { class: "app sheet confirm", role: "dialog", "aria-label": options.title },
      sheetHead(options.title, close),
      h(
        "div",
        { class: "confirm-body" },
        ...options.lines.filter((l): l is string => Boolean(l)).map((line) => h("p", {}, line)),
      ),
      h(
        "div",
        { class: "sheet-foot" },
        h("span", { class: "gpick" }),
        h("button", { class: "ghost", onclick: close }, "Cancel"),
        h(
          "button",
          {
            class: "btn danger",
            onclick: () => {
              options.onConfirm();
              close();
            },
          },
          options.confirmLabel,
        ),
      ),
    ),
  );
}

/** Whether the tunnel is currently running on this server. */
export function isLive(server: Server): boolean {
  return connection !== "off" && store.get().selectedServerId === server.id;
}

/**
 * The usage sheet, for one config or for every config in a group.
 *
 * It stays live while open — the numbers climb as the tunnel runs — by repainting on store
 * changes; the listener drops itself the first time it fires after the sheet has gone. Clearing
 * asks in the footer rather than in a second sheet, and says what goes: the history, never the
 * configs.
 */
export function openUsage(target: { server: Server } | { group: Group }) {
  const configs = (): Server[] =>
    "server" in target
      ? [store.server(target.server.id)].filter((s): s is Server => Boolean(s))
      : store.serversIn(target.group.id);

  openSheet((close) => {
    const body = h("div", { class: "share-body usage-host" });
    const foot = h("div", { class: "sheet-foot" });
    let confirming = false;

    const paint = () => {
      const servers = configs();
      const group = "group" in target ? store.group(target.group.id) : store.group(servers[0]?.groupId ?? "");
      const heading =
        "server" in target
          ? [target.server.profile.name, group?.name]
          : [target.group.name, `${servers.length} config${servers.length === 1 ? "" : "s"}`];
      render(
        body,
        h("p", { class: "share-name" }, heading[0], heading[1] ? h("span", { class: "usage-of" }, ` · ${heading[1]}`) : null),
        usageBody({
          servers,
          quota: "group" in target ? group?.quota : null,
          breakdown: "group" in target,
          now: Date.now(),
        }),
      );

      const histories = servers.map((s) => s.usage);
      const since = firstDay(histories);
      const recorded = total(histories);
      if (confirming && since) {
        render(
          foot,
          h(
            "span",
            { class: "gpick" },
            `Clears ${size(recorded.up + recorded.down)} recorded since ${shortDate(since)}. The configs stay.`,
          ),
          h("button", { class: "ghost", onclick: () => ((confirming = false), paint()) }, "Cancel"),
          h(
            "button",
            {
              class: "btn danger",
              onclick: () => {
                confirming = false;
                store.clearUsage(servers.map((s) => s.id));
                hooks.log(`[ui] cleared the usage history of ${heading[0]}`);
              },
            },
            "Clear",
          ),
        );
      } else {
        render(
          foot,
          h("span", { class: "gpick" }),
          h(
            "button",
            { class: "ghost danger", disabled: !since, onclick: () => ((confirming = true), paint()) },
            "Clear history",
          ),
          h("button", { class: "ghost", onclick: close }, "Done"),
        );
      }
    };
    paint();

    // After insertion: `subscribe` calls the listener at once, and a sheet not yet in the document
    // would read as already closed.
    queueMicrotask(() => {
      let off: (() => void) | null = null;
      off = store.subscribe(() => {
        if (!body.isConnected) off?.();
        else paint();
      });
    });

    return h(
      "div",
      { class: "app sheet usage", role: "dialog", "aria-label": "Usage" },
      sheetHead("Usage", close),
      body,
      foot,
    );
  });
}

export function confirmDeleteServer(server: Server) {
  const group = store.group(server.groupId);
  const live = isLive(server);

  confirmSheet({
    title: "Delete this server?",
    lines: [
      `${server.profile.name} · ${describe(server.profile)}`,
      // A subscription server returns on the next update, so removing it is housekeeping. A
      // hand-added one is gone for good, and its credentials with it.
      group?.kind === "subscription"
        ? `It will come back the next time ${group.name} updates.`
        : "Its address and credentials are not saved anywhere else.",
      live ? "The tunnel is running on it and will be disconnected." : null,
    ],
    confirmLabel: "Delete",
    onConfirm: () => {
      store.removeServer(server.id);
      hooks.log(`[ui] deleted ${server.profile.name}`);
      if (live) void disconnect();
    },
  });
}

export function confirmDeleteGroup(group: Group) {
  const servers = store.serversIn(group.id);
  const live = servers.some(isLive);

  confirmSheet({
    title: group.url ? "Delete this subscription?" : "Delete this group?",
    lines: [
      group.name,
      servers.length
        ? `Its ${servers.length} server${servers.length === 1 ? "" : "s"} go with it.`
        : "It has no servers.",
      // The URL is the credential: anyone holding it can read the whole server list, and this is
      // the only copy the app has.
      group.url ? "The subscription address is not saved anywhere else." : null,
      live ? "The tunnel is running on one of them and will be disconnected." : null,
    ],
    confirmLabel: "Delete",
    onConfirm: () => {
      store.removeGroup(group.id);
      hooks.log(`[ui] deleted ${group.name}`);
      if (live) void disconnect();
    },
  });
}
