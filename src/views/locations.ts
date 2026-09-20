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
import { orderGroups, store, type Group, type Server } from "../store";
import { icon } from "./icons";

export interface LocationsCallbacks {
  onSelect: (server: Server) => void;
  onRefresh: (group: Group) => void;
  onAdd: () => void;
  onQuickConnect: () => void;
  onTestAll: () => void;
}

export class LocationsPanel {
  private filter = "";
  private testing = false;

  constructor(
    private root: HTMLElement,
    private callbacks: LocationsCallbacks,
  ) {
    store.subscribe(() => this.render());
  }

  /** A sweep is running, so the list shows progress rather than stale numbers. */
  setTesting(testing: boolean) {
    this.testing = testing;
    this.render();
  }

  private matches(server: Server): boolean {
    if (!this.filter) return true;
    const needle = this.filter.toLowerCase();
    const country = place(server.country);
    return (
      server.profile.name.toLowerCase().includes(needle) ||
      server.city.toLowerCase().includes(needle) ||
      country.name.toLowerCase().includes(needle) ||
      server.country.toLowerCase().includes(needle)
    );
  }

  render() {
    const data = store.get();
    const selectedId = data.selectedServerId;

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
  }

  private footer(count: number) {
    return h(
      "div",
      { class: "locs-foot" },
      h(
        "button",
        {
          class: "ghost",
          disabled: this.testing || count === 0,
          onclick: () => this.callbacks.onTestAll(),
        },
        this.testing ? "Testing…" : `Test all ${count || ""}`.trim(),
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
            ? `${place(fastest.country).name} · ${fastest.latency} ms`
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
      nodes.push(...servers.map((s) => this.row(s, s.id === selectedId)));
      if (!servers.length) nodes.push(this.emptyRow(group));
    }
    return nodes;
  }

  private groupHeader(group: Group, count: number) {
    const meta = group.lastError
      ? `${count} server${count === 1 ? "" : "s"} · update failed ${ago(group.updatedAt)}`
      : group.kind === "manual"
        ? `${count} server${count === 1 ? "" : "s"} · added by hand`
        : `${count} server${count === 1 ? "" : "s"} · updated ${ago(group.updatedAt)}`;

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
        h("span", { class: `gmeta${group.lastError ? " bad" : ""}` }, meta),
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
    const country = place(server.country);
    const { text, grade } = gradeLatency(server.latency);
    const strength = barCount(server.latency);

    // City and how it is secured, not the raw share-link name — that name is usually the country and
    // city again, and repeating it wastes the only line there is.
    const security = server.profile.tls.reality
      ? "Reality"
      : server.profile.tls.enabled
        ? "TLS"
        : "no TLS";
    const subtitle = [server.city, `VLESS · ${security}`].filter(Boolean).join(" · ");

    return h(
      "button",
      {
        class: `loc${selected ? " active" : ""}${server.retired ? " retired" : ""}`,
        title: server.retired
          ? "This server is no longer in the subscription, and is kept only while it is connected."
          : undefined,
        onclick: () => this.callbacks.onSelect(server),
      },
      h("span", { class: "flag", style: `background:${country.flag}` }),
      h(
        "span",
        { class: "loc-main" },
        h(
          "span",
          { class: "loc-name" },
          country.name === "Unknown" ? server.profile.name : country.name,
        ),
        h("span", { class: "loc-sub" }, server.retired ? `${subtitle} · retired` : subtitle),
      ),
      h(
        "span",
        { class: "loc-right" },
        h(
          "span",
          { class: `bars s${strength} ping ${grade}` },
          h("i", {}),
          h("i", {}),
          h("i", {}),
        ),
        h("span", { class: `ping ${grade}` }, text),
      ),
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
