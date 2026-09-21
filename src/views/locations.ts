/**
 * The locations panel: servers grouped by where they came from.
 *
 * Grouping is by subscription rather than by country, because that is the unit users reason about —
 * one subscription may be fast and near expiry, another a rarely-touched backup. Hand-added servers
 * sit first: they are the ones the user chose deliberately, they never change under a refresh, and
 * they are what you reach for when a subscription has gone stale.
 */

import { ago, bars as barCount, latency as gradeLatency, size } from "../format";
import { h, render } from "../dom";
import { place } from "../geo";
import { describe } from "../share";
import {
  CDN_NAMES,
  cdnOf,
  isRelayed,
  located,
  MANUAL_GROUP_ID,
  orderGroups,
  store,
  type Group,
  type Server,
} from "../store";
import { icon } from "./icons";

export interface LocationsCallbacks {
  onSelect: (server: Server) => void;
  onRefresh: (group: Group) => void;
  /** Re-test one server: latency, entry and exit. */
  onCheck: (server: Server) => void;
  onShare: (server: Server) => void;
  onEdit: (server: Server) => void;
  onDelete: (server: Server) => void;
  onRemoveGroup: (group: Group) => void;
  onAdd: () => void;
  onQuickConnect: () => void;
  onTestAll: () => void;
  /** Stop a running Test all after the batch in flight. */
  onStopTest: () => void;
}

/** Rows a group shows before "Show more". */
const ROW_PAGE = 100;

export class LocationsPanel {
  private filter = "";
  /** Test all's progress, while it runs: how many are done, of how many. */
  private progress: { done: number; total: number } | null = null;
  /** Servers being checked on their own from the ⋯ menu; their rows show progress. */
  private checking = new Set<string>();
  private menu: HTMLElement | null = null;
  /** How many rows each group is showing, beyond the first page; see `ROW_PAGE`. */
  private shown = new Map<string, number>();

  constructor(
    private root: HTMLElement,
    private callbacks: LocationsCallbacks,
  ) {
    store.subscribe(() => this.render());
    document.addEventListener("mousedown", (e) => {
      if (this.menu && !this.menu.contains(e.target as Node)) this.closeMenu();
    });
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape") this.closeMenu();
    });
    // A fixed-position menu would stay put while its row scrolled away underneath it.
    root.addEventListener("scroll", () => this.closeMenu(), true);
    window.addEventListener("resize", () => this.closeMenu());
  }

  setChecking(ids: string[], checking: boolean) {
    for (const id of ids) {
      if (checking) this.checking.add(id);
      else this.checking.delete(id);
    }
    this.render();
  }

  /**
   * Test all's progress: "Testing 7/18", then `null` when done. Each row updates on its own as its
   * result arrives; this is only the running count.
   */
  setProgress(progress: { done: number; total: number } | null) {
    this.progress = progress;
    this.render();
  }

  private matches(server: Server): boolean {
    if (!this.filter) return true;
    const needle = this.filter.toLowerCase();
    const where = located(server);
    const country = place(where.country);
    return (
      server.profile.name.toLowerCase().includes(needle) ||
      where.city.toLowerCase().includes(needle) ||
      country.name.toLowerCase().includes(needle) ||
      where.country.toLowerCase().includes(needle)
    );
  }

  render() {
    const data = store.get();
    const selectedId = data.selectedServerId;

    // The list is rebuilt on every change — a test result, a subscription landing, a selection —
    // and a rebuilt scroller starts at the top. So where the user was reading, and the search box
    // they may be typing in, are carried across the rebuild rather than reset under them.
    const scroll = this.root.querySelector<HTMLElement>(".locs-list")?.scrollTop ?? 0;
    const search = this.root.querySelector<HTMLInputElement>(".search input");
    const typing = search !== null && document.activeElement === search;
    const caret = typing ? [search.selectionStart, search.selectionEnd] : null;

    render(
      this.root,
      this.header(),
      this.quickConnect(),
      h(
        "div",
        { class: "locs-list" },
        ...orderGroups(data.groups).flatMap((group) => this.groupSection(group, selectedId)),
      ),
      this.footer(data.servers.length),
    );

    const list = this.root.querySelector<HTMLElement>(".locs-list");
    if (list) list.scrollTop = scroll;
    if (typing && caret) {
      const next = this.root.querySelector<HTMLInputElement>(".search input");
      next?.focus();
      next?.setSelectionRange(caret[0], caret[1]);
    }
  }

  private footer(count: number) {
    return h(
      "div",
      { class: "locs-foot" },
      h(
        "button",
        {
          class: "ghost",
          disabled: count === 0,
          // While running, the same button stops it: a list of thousands takes a long time, and
          // there must be a way out that is not quitting the app.
          onclick: () => (this.progress === null ? this.callbacks.onTestAll() : this.callbacks.onStopTest()),
        },
        this.progress === null
          ? `Test all ${count ? count.toLocaleString() : ""}`.trim()
          : `Stop · Testing ${this.progress.done.toLocaleString()}/${this.progress.total.toLocaleString()}`,
      ),
    );
  }

  private header() {
    return h(
      "div",
      { class: "locs-head" },
      h(
        "div",
        { class: "locs-title" },
        h("h3", {}, "Locations"),
        h(
          "button",
          {
            class: "addbtn",
            title: "Add servers",
            "aria-label": "Add servers",
            onclick: () => this.callbacks.onAdd(),
          },
          icon("plus", 15),
        ),
      ),
      h(
        "label",
        { class: "search" },
        icon("search", 14),
        h("input", {
          type: "search",
          placeholder: "Search country or city",
          value: this.filter,
          oninput: (e: Event) => {
            this.filter = (e.target as HTMLInputElement).value;
            this.render();
          },
        }),
      ),
    );
  }

  private quickConnect() {
    const fastest = store.fastest();
    return h(
      "button",
      {
        class: "quick",
        // Nothing has been tested yet, so there is no "fastest" to honour.
        disabled: !fastest,
        onclick: () => this.callbacks.onQuickConnect(),
      },
      icon("bolt", 17),
      h(
        "span",
        { class: "qt" },
        h("b", {}, "Quick Connect"),
        h(
          "span",
          {},
          fastest
            ? `${fastest.profile.name} · ${fastest.latency} ms`
            : "Test your servers to enable",
        ),
      ),
    );
  }

  private groupSection(group: Group, selectedId: string | null): Node[] {
    const servers = store.serversIn(group.id).filter((s) => this.matches(s));

    // A subscription with no matches should still show its header, so the user can see the filter
    // excluded it rather than wondering where the group went.
    const nodes: Node[] = [this.groupHeader(group, servers.length)];

    if (group.quota) nodes.push(this.quotaBar(group));
    if (!group.collapsed) {
      // A page at a time. Public lists run to tens of thousands of servers, and a row for each is
      // tens of thousands of elements rebuilt on every change — a list that takes seconds to
      // answer a click and can take the webview down with it. The selected server is always
      // shown, wherever it falls, so the one in use never disappears behind the button.
      const limit = this.shown.get(group.id) ?? ROW_PAGE;
      const visible = servers.slice(0, limit);
      const selected = servers.find((s) => s.id === selectedId);
      if (selected && !visible.includes(selected)) visible.push(selected);
      nodes.push(...visible.map((s) => this.row(s, s.id === selectedId)));
      if (servers.length > limit) nodes.push(this.moreRow(group, servers.length - limit, limit));
      if (!servers.length) nodes.push(this.emptyRow(group));
    }
    return nodes;
  }

  private groupHeader(group: Group, count: number) {
    const servers = `${count} server${count === 1 ? "" : "s"}`;
    const meta = group.lastError
      ? // `updatedAt` is only set by a successful refresh, so a subscription that has never had
        // one has no time to quote — and "update failed never" is not a sentence. This is the
        // state a newly added subscription lands in when its very first fetch fails.
        group.updatedAt
        ? `${servers} · update failed ${ago(group.updatedAt)}`
        : `${servers} · could not be updated`
      : group.kind === "manual"
        ? `${servers} · added by hand`
        : `${servers} · updated ${ago(group.updatedAt)}`;

    return h(
      "div",
      { class: "ghead" },
      h(
        "button",
        {
          class: "gchev",
          "aria-label": group.collapsed ? "Expand" : "Collapse",
          "aria-expanded": String(!group.collapsed),
          onclick: () => store.toggleGroup(group.id),
        },
        icon(group.collapsed ? "chevron-right" : "chevron-down", 10),
      ),
      h(
        "span",
        { class: "gname" },
        h("b", {}, group.name),
        // The row has no space for the reason, but a subscription that failed is useless
        // without it, so it is carried as the hover text.
        h(
          "span",
          { class: `gmeta${group.lastError ? " bad" : ""}`, title: group.lastError ?? undefined },
          meta,
        ),
      ),
      // Only a subscription has somewhere to refresh from.
      group.url
        ? h(
            "button",
            {
              class: `gsync${group.refreshing ? " busy" : ""}`,
              "aria-label": `Update ${group.name}`,
              title: group.refreshing ? "Updating…" : `Update ${group.name}`,
              disabled: group.refreshing,
              onclick: () => this.callbacks.onRefresh(group),
            },
            icon("refresh", 14),
          )
        : null,
      // The hand-added group is not removable: it is where a pasted link goes when no
      // subscription was chosen, so there would be nowhere to put the next one.
      group.id === MANUAL_GROUP_ID
        ? null
        : h(
            "button",
            {
              class: "gsync danger",
              "aria-label": `Delete ${group.name}`,
              title: group.url ? "Delete this subscription" : "Delete this group",
              onclick: () => this.callbacks.onRemoveGroup(group),
            },
            icon("trash", 14),
          ),
    );
  }

  private quotaBar(group: Group) {
    const quota = group.quota!;
    const used = quota.totalBytes > 0 ? quota.usedBytes / quota.totalBytes : 0;
    const resets = quota.resetsAt
      ? `resets ${new Date(quota.resetsAt).toLocaleDateString(undefined, {
          day: "numeric",
          month: "short",
        })}`
      : "no reset date";

    return h(
      "div",
      { class: "gquota" },
      h(
        "span",
        { class: "qbar" },
        // Amber past 85%: the point where a user should be thinking about it.
        h("i", {
          class: used > 0.85 ? "low" : "",
          style: `width:${Math.min(100, used * 100).toFixed(1)}%`,
        }),
      ),
      h(
        "span",
        { class: "qtxt" },
        `${size(quota.usedBytes)} of ${size(quota.totalBytes)} used · ${resets}`,
      ),
    );
  }

  private row(server: Server, selected: boolean) {
    const where = located(server);
    const country = place(where.country);
    const { text, grade } = gradeLatency(server.latency);
    const strength = barCount(server.latency);

    // City and how it is secured, not the raw share-link name — that name is usually the country and
    // city again, and repeating it wastes the only line there is.
    // Where it enters, when that is not where it exits: the row's one hint that the server is
    // a relay. A country code, because the line has room for little else.
    const via = isRelayed(server) ? `via ${server.entry!.country}` : null;
    // Where it is — the city, or the country when no city is known — then how it gets there.
    const subtitle = [where.city || (where.country ? country.name : ""), via, describe(server.profile)]
      .filter((part) => part && part !== "Unknown")
      .join(" · ");

    // The row is a button, so the actions cannot live inside it: nested buttons are invalid and
    // the inner click would also select the server. The one ⋯ button sits beside it instead, over
    // the latency reading, and opens a menu.
    //
    // One button rather than a row of them: Delete used to sit a few pixels from Edit and Share,
    // which is a misclick away from losing a hand-added server. In the menu it is last, set apart
    // and red, and it still asks before doing anything.
    const actions = h(
      "span",
      { class: "loc-acts" },
      h(
        "button",
        {
          class: "locact",
          "aria-label": `Actions for ${server.profile.name}`,
          "aria-haspopup": "menu",
          onclick: (e: Event) => this.openMenu(server, e.currentTarget as HTMLElement),
        },
        icon("more", 15),
      ),
    );

    return h(
      "div",
      { class: "loc-wrap" },
      this.selectRow(server, selected, country, subtitle, strength, grade, text),
      actions,
    );
  }

  private selectRow(
    server: Server,
    selected: boolean,
    country: ReturnType<typeof place>,
    subtitle: string,
    strength: number,
    grade: string,
    text: string,
  ) {
    const where = located(server);
    return h(
      "button",
      {
        class: `loc${selected ? " active" : ""}${server.retired ? " retired" : ""}`,
        title: server.retired
          ? "This server is no longer in the subscription, and is kept only while it is connected."
          : // A dash means either "never tested" or "the test failed", and the row has space for
            // neither explanation. The reason is the difference between a dead server and one
            // that is fine but could not reach the endpoint the test used.
            server.latency !== null && server.latency < 0 && server.latencyError
            ? `Last test failed: ${server.latencyError}`
            : undefined,
        onclick: () => this.callbacks.onSelect(server),
      },
      // Flagged by where it was measured to come out, labelled by what it is called. The two
      // disagree often enough that conflating them would rename half a subscription. A relay
      // carries its entry as a small second flag on the corner.
      h(
        "span",
        {
          class: "flag",
          style: `background:${country.flag}`,
          "aria-label":
            where.source === "exit"
              ? `Exits in ${country.name}${isRelayed(server) ? `, enters in ${place(server.entry!.country).name}` : ""}`
              : where.source === "entry"
                ? `Address in ${country.name}; not yet tested`
                : undefined,
        },
        isRelayed(server)
          ? h("span", { class: "flag via", style: `background:${place(server.entry!.country).flag}` })
          : null,
      ),
      h(
        "span",
        { class: "loc-main" },
        h(
          "span",
          { class: "loc-name" },
          // The config's own name, always. Its location is the flag and the subtitle — putting
          // a country here instead made measured rows look renamed, and gave five configs that
          // exit in one country the same title, so they could not be told apart.
          h("span", { class: "nm" }, server.profile.name),
          ...this.cdnTag(server),
        ),
        h("span", { class: "loc-sub" }, server.retired ? `${subtitle} · retired` : subtitle),
      ),
      h(
        "span",
        { class: "loc-right" },
        ...(this.checking.has(server.id)
          ? [h("span", { class: "ping none checking" }, "testing…")]
          : [
              h(
                "span",
                { class: `bars s${strength} ping ${grade}` },
                h("i", {}),
                h("i", {}),
                h("i", {}),
              ),
              h("span", { class: `ping ${grade}` }, text),
            ]),
      ),
    );
  }

  /** "CDN · CF": the config's address is a CDN edge. A tag, so it can be filtered on later. */
  private cdnTag(server: Server): Node[] {
    const cdn = cdnOf(server);
    if (!cdn) return [];
    return [
      h(
        "span",
        { class: `cdn-tag ${cdn}`, "aria-label": `CDN: ${CDN_NAMES[cdn]}` },
        icon("cloud", 11),
        "CDN",
        h("b", {}, cdn === "cloudflare" ? "CF" : CDN_NAMES[cdn]),
      ),
    ];
  }

  // ---------------------------------------------------------------- the ⋯ menu

  private closeMenu() {
    this.menu?.remove();
    this.menu = null;
  }

  /**
   * Check, Share and Edit, then Delete on its own below a divider.
   *
   * Fixed to the viewport rather than placed inside the row, because the list scrolls and clips
   * its overflow — a menu inside it would be cut off at the bottom rows.
   */
  private openMenu(server: Server, anchor: HTMLElement) {
    const wasOpen = this.menu?.dataset.id === server.id;
    this.closeMenu();
    // Pressing ⋯ again closes it, as a menu button is expected to.
    if (wasOpen) return;

    const item = (label: string, glyph: string, run: () => void, danger = false) =>
      h(
        "button",
        {
          class: `rmenu-i${danger ? " danger" : ""}`,
          role: "menuitem",
          onclick: () => {
            this.closeMenu();
            run();
          },
        },
        icon(glyph, 14),
        label,
      );

    const menu = h(
      "div",
      { class: "rmenu", role: "menu", "aria-label": server.profile.name, "data-id": server.id },
      item(
        this.checking.has(server.id) ? "Checking…" : "Check",
        "refresh",
        () => this.callbacks.onCheck(server),
      ),
      item("Share", "share", () => this.callbacks.onShare(server)),
      item("Edit", "pencil", () => this.callbacks.onEdit(server)),
      h("div", { class: "rmenu-sep", role: "separator" }),
      item("Delete…", "trash", () => this.callbacks.onDelete(server), true),
    );
    document.body.appendChild(menu);
    this.menu = menu;

    // Below the button, right-aligned to it; above it when that would run off the window.
    const box = anchor.getBoundingClientRect();
    const height = menu.offsetHeight;
    const below = box.bottom + 4;
    menu.style.top = `${below + height > window.innerHeight - 8 ? box.top - 4 - height : below}px`;
    menu.style.left = `${Math.max(8, box.right - menu.offsetWidth)}px`;
    menu.querySelector<HTMLElement>(".rmenu-i")?.focus();
  }

  private moreRow(group: Group, hidden: number, limit: number) {
    const next = Math.min(hidden, ROW_PAGE * 2);
    return h(
      "button",
      {
        class: "more-row",
        onclick: () => {
          this.shown.set(group.id, limit + next);
          this.render();
        },
      },
      `Show ${next.toLocaleString()} more`,
      h("span", {}, `${hidden.toLocaleString()} not shown`),
    );
  }

  private emptyRow(group: Group) {
    return h(
      "p",
      { class: "gempty" },
      this.filter
        ? "No servers match your search."
        : group.kind === "manual"
          ? "Paste a share link to add one."
          : "This subscription has no servers yet.",
    );
  }
}
