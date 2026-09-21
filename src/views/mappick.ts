/**
 * The card that opens from a map dot holding more than one server.
 *
 * A city routinely has several: a provider's three New York servers, the same exit reached over
 * two transports. Picking one of them by clicking the dot would be a coin toss, so the dot opens
 * this instead — the city, a Fastest button that does what Quick Connect does but only among these,
 * and the servers themselves with their latency so the choice can be made by hand.
 *
 * A card over the map rather than a sheet: the map is what the user is looking at, and the card is
 * anchored to the dot it came from, so hiding the map behind a scrim would lose the question.
 */

import { h } from "../dom";
import { latency as gradeLatency } from "../format";
import { place } from "../geo";
import { describe } from "../share";
import type { Server } from "../store";
import type { PickPoint } from "./map";

export interface PickRequest {
  /** "New York" — the city if measured, the country otherwise. */
  title: string;
  country: string;
  servers: Server[];
  selectedId: string | null;
  at: PickPoint;
}

const CARD_WIDTH = 272;

/** The fastest server that answered, or none: an untested or unreachable one is not "fastest". */
export function fastest(servers: Server[]): Server | undefined {
  return servers
    .filter((s) => !s.retired && s.latency !== null && s.latency > 0)
    .sort((a, b) => (a.latency ?? 0) - (b.latency ?? 0))[0];
}

export class MapPicker {
  private card: HTMLElement | null = null;

  constructor(
    private host: HTMLElement,
    private onPick: (server: Server) => void,
  ) {
    // Mousedown rather than click, so a drag that starts on the map closes the card as the user
    // expects rather than when they let go somewhere else.
    document.addEventListener("mousedown", (e) => {
      if (!this.card) return;
      const target = e.target as Node;
      if (this.card.contains(target)) return;
      // A click on another dot reopens the card for that dot; closing first would flash it.
      if ((target as Element).closest?.(".pin.pick")) return;
      this.close();
    });
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape") this.close();
    });
    window.addEventListener("resize", () => this.close());
  }

  get isOpen(): boolean {
    return this.card !== null;
  }

  close() {
    this.card?.remove();
    this.card = null;
    this.host.classList.remove("picking");
  }

  open(req: PickRequest) {
    this.close();
    const best = fastest(req.servers);
    // Fastest first, the rest by latency, the untested last — the order the choice is made in.
    const ordered = [...req.servers].sort((a, b) => rank(a) - rank(b));

    const card = h(
      "div",
      { class: "mpick", role: "dialog", "aria-label": `Servers in ${req.title}` },
      h(
        "div",
        { class: "mpick-head" },
        h("span", { class: "flag", style: `background:${place(req.country).flag}` }),
        h(
          "span",
          { class: "mpick-t" },
          h("b", {}, req.title),
          h("span", {}, `${req.servers.length} servers`),
        ),
      ),
      h(
        "button",
        {
          class: "mpick-fast",
          disabled: !best,
          onclick: () => best && this.pick(best),
        },
        h("span", { class: "k" }, "Fastest"),
        h(
          "span",
          { class: "v" },
          best ? `${best.profile.name} · ${best.latency} ms` : "Nothing here has been tested yet",
        ),
      ),
      h(
        "div",
        { class: "mpick-list" },
        ...ordered.map((server) => {
          const { text, grade } = gradeLatency(server.latency);
          const selected = server.id === req.selectedId;
          return h(
            "button",
            {
              class: `mpick-row${selected ? " on" : ""}`,
              "aria-current": selected ? "true" : undefined,
              onclick: () => this.pick(server),
            },
            h(
              "span",
              { class: "mpick-main" },
              h("b", {}, server.profile.name),
              h("span", {}, describe(server.profile)),
            ),
            h("span", { class: `ping ${grade}` }, text === "—" ? text : `${text} ms`),
          );
        }),
      ),
    );

    this.host.appendChild(card);
    this.card = card;
    // The dot that opened the card is still hovered, and its label would poke out from under it.
    this.host.classList.add("picking");
    this.place(card, req.at);
    card.querySelector<HTMLElement>(".mpick-fast:not(:disabled), .mpick-row")?.focus();
  }

  /**
   * A card of facts about one place — used for the user's own location: their public address,
   * who provides it, and where it is. Same card, same placement and dismissal as the server list.
   */
  openDetails(req: { title: string; subtitle: string; country: string; rows: [string, string][]; note?: string; at: PickPoint }) {
    this.close();
    const card = h(
      "div",
      { class: "mpick details", role: "dialog", "aria-label": req.title },
      h(
        "div",
        { class: "mpick-head" },
        h("span", { class: "flag", style: `background:${place(req.country).flag}` }),
        h("span", { class: "mpick-t" }, h("b", {}, req.title), h("span", {}, req.subtitle)),
      ),
      h(
        "dl",
        { class: "mpick-facts" },
        ...req.rows.flatMap(([k, v]) => [h("dt", {}, k), h("dd", {}, v)]),
      ),
      req.note ? h("p", { class: "mpick-note" }, req.note) : null,
    );
    this.host.appendChild(card);
    this.card = card;
    this.host.classList.add("picking");
    this.place(card, req.at);
  }

  private pick(server: Server) {
    this.close();
    this.onPick(server);
  }

  /**
   * Beside the dot, on whichever side has room, and never past the pane's edges or under the
   * status card — a card cut off by the window is a card with a hidden row.
   */
  private place(card: HTMLElement, at: PickPoint) {
    const pane = this.host.getBoundingClientRect();
    const status = this.host.querySelector<HTMLElement>(".status")?.getBoundingClientRect();
    const floor = (status ? status.top - pane.top : pane.height) - 10;
    const height = card.offsetHeight;
    const gap = 14;

    let left = at.x + gap;
    if (left + CARD_WIDTH > pane.width - 10) left = at.x - gap - CARD_WIDTH;
    left = Math.max(10, left);

    let top = at.y - height / 2;
    top = Math.min(top, floor - height);
    top = Math.max(10, top);

    card.style.left = `${left}px`;
    card.style.top = `${top}px`;
  }
}

function rank(server: Server): number {
  if (server.latency === null) return 1e9;
  if (server.latency < 0) return 1e8;
  return server.latency;
}
