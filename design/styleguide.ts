/**
 * The style guide.
 *
 * A design reference that cannot drift, because it is not a description of the design — it is the
 * design, rendered. The page loads `src/styles.css` itself, reads the token values back out of the
 * browser's own stylesheet model, enumerates the icon set from `views/icons.ts`, and mounts the
 * real `StatusCard`. Nothing here restates a value that lives somewhere else, so there is no second
 * copy to forget to update.
 *
 * Two things follow from that, and both are deliberate:
 *
 * - **Both themes are shown at once.** The app follows the system setting and has no in-app theme
 *   switch, so a guide that only rendered the current theme would show half the palette and offer
 *   no way to see the other half. The dark values are read out of the `prefers-color-scheme` block
 *   rather than guessed, so they are the real ones whichever theme you are in.
 * - **The component gallery mirrors the markup in `views/`.** The views are bound to the store, so
 *   they cannot be mounted here directly — except `StatusCard`, which takes a plain model and is
 *   therefore the real component. Everything else reuses the real classes and the real helpers
 *   (`place`, `latency`, `bars`), so only the element nesting is restated.
 *
 * Open it with `npm run design`. It is served by Vite but never built: `vite build` takes only the
 * root `index.html` as an entry, so this page and its fixtures cannot reach a bundle.
 */

import { bars as barCount, latency as gradeLatency, size } from "../src/format";
import { h, render } from "../src/dom";
import { describe } from "../src/share";
import { place } from "../src/geo";
import { shieldState, type ShieldInput } from "../src/shield";
import { trayIcon, type TokenReader } from "../src/trayicon";
import { ICON_NAMES, icon } from "../src/views/icons";
import { StatusCard, type ConnectionState } from "../src/views/status";
import type { Server } from "../src/store";

// ---------------------------------------------------------------- token reading

type Tokens = { light: Map<string, string>; dark: Map<string, string> };

/**
 * Reads the custom properties back out of the live stylesheet.
 *
 * Via the CSSOM rather than `getComputedStyle`, because computed style only ever gives the theme
 * currently in effect. Walking the rules gives both, and gives them in declaration order — which is
 * the order a reader of `styles.css` would meet them in.
 */
function readTokens(): Tokens {
  const tokens: Tokens = { light: new Map(), dark: new Map() };

  for (const sheet of Array.from(document.styleSheets)) {
    let rules: CSSRuleList;
    try {
      rules = sheet.cssRules;
    } catch {
      // A cross-origin sheet throws on access. None of ours is, but a browser extension's might be.
      continue;
    }
    for (const rule of Array.from(rules)) collect(rule, tokens, false);
  }

  return tokens;
}

function collect(rule: CSSRule, tokens: Tokens, inDark: boolean) {
  if (rule instanceof CSSMediaRule) {
    const dark = inDark || rule.conditionText.includes("prefers-color-scheme: dark");
    for (const inner of Array.from(rule.cssRules)) collect(inner, tokens, dark);
    return;
  }

  if (!(rule instanceof CSSStyleRule) || !rule.selectorText.includes(":root")) return;

  const target = inDark ? tokens.dark : tokens.light;
  for (let i = 0; i < rule.style.length; i += 1) {
    const name = rule.style[i];
    if (name.startsWith("--")) target.set(name, rule.style.getPropertyValue(name).trim());
  }
}

/** A token's dark value, falling back to its light one — most tokens are not redefined for dark. */
function darkValue(tokens: Tokens, name: string): string {
  return tokens.dark.get(name) ?? tokens.light.get(name) ?? "";
}

// ---------------------------------------------------------------- colour

interface Group {
  title: string;
  note: string;
  names: string[];
}

/**
 * How the palette is grouped for reading.
 *
 * By the job each token does rather than by hue, because that is the question someone reaching for
 * one actually has: "what do I paint a raised surface with", not "what blues are there". Any token
 * not named here still appears, under `Other` — so adding one to `styles.css` never makes it
 * invisible on this page.
 */
const COLOUR_GROUPS: Group[] = [
  {
    title: "Surfaces",
    note: "Five depths, from the window behind everything to the recessed wells inside a card.",
    names: ["--ground", "--stage", "--surface", "--raised", "--sunk"],
  },
  {
    title: "Text and lines",
    note: "Four weights of ink and two of rule. --faint is the floor: anything quieter fails contrast.",
    names: ["--ink", "--ink-2", "--muted", "--faint", "--line", "--line-2"],
  },
  {
    title: "Brand",
    note: "The resting, interactive colour. The --glow variants are for focus rings and shadows, so they carry alpha.",
    names: ["--brand", "--brand-fill", "--brand-soft", "--brand-glow"],
  },
  {
    title: "Connected",
    note: "Green means the tunnel is up — the one claim this app exists to make. Nothing else may use it.",
    names: ["--live", "--live-fill", "--live-soft", "--live-glow"],
  },
  {
    title: "Warning",
    note: "A quota near its limit, a subscription that failed to refresh, a tunnel that cannot start.",
    names: ["--warn", "--warn-fill", "--warn-soft"],
  },
  {
    title: "Disconnected",
    note: "Not an error colour. Disconnected is a resting state, and an unreachable server is a fact.",
    names: ["--off", "--off-fill", "--off-soft", "--off-glow"],
  },
  {
    title: "Map",
    note: "The dotted land mass and the arc drawn to the active exit.",
    names: ["--map-land", "--map-border", "--map-label", "--map-line"],
  },
];

const NON_COLOUR = ["--shadow", "--shadow-pop", "--ui", "--mono"];

function tokenCard(tokens: Tokens, name: string) {
  const light = tokens.light.get(name) ?? "";
  const dark = darkValue(tokens, name);

  return h(
    "div",
    { class: "sg-token" },
    h(
      "span",
      { class: "sg-chips", title: `light ${light} · dark ${dark}` },
      h("span", { class: "sg-chip", style: `background:${light}` }),
      h("span", { class: "sg-chip", style: `background:${dark}` }),
    ),
    h(
      "span",
      { class: "sg-token-text" },
      h("b", { class: "sg-token-name" }, name),
      h("span", { class: "sg-token-val" }, light === dark ? light : `${light} → ${dark}`),
    ),
  );
}

function colourSection(tokens: Tokens) {
  const named = new Set([...COLOUR_GROUPS.flatMap((g) => g.names), ...NON_COLOUR]);
  const leftover = [...tokens.light.keys()].filter((n) => !named.has(n));

  const groups = leftover.length
    ? [
        ...COLOUR_GROUPS,
        { title: "Other", note: "Declared in styles.css but not yet grouped here.", names: leftover },
      ]
    : COLOUR_GROUPS;

  return section(
    "Colour",
    "Every custom property in styles.css. The left half of each chip is the light value, the right half the dark one.",
    ...groups.map((group) =>
      h(
        "div",
        { class: "sg-group" },
        h("h3", {}, group.title),
        h("p", { class: "sg-note", style: "margin:0 0 10px" }, group.note),
        h("div", { class: "sg-tokens" }, ...group.names.map((n) => tokenCard(tokens, n))),
      ),
    ),
  );
}

// ---------------------------------------------------------------- elevation and type

function elevationSection(tokens: Tokens) {
  return section(
    "Elevation",
    "Two shadows. --shadow sits under resting cards; --shadow-pop under things that float over the map or the window.",
    h(
      "div",
      { class: "sg-specimens" },
      ...["--shadow", "--shadow-pop"].map((name) =>
        h(
          "figure",
          { class: "sg-specimen", style: "margin:0" },
          h("figcaption", {}, name),
          h("div", { class: "sg-shadow", style: `box-shadow:${tokens.light.get(name) ?? ""}` }),
        ),
      ),
    ),
  );
}

function typeSection(tokens: Tokens) {
  const ui = tokens.light.get("--ui") ?? "";
  const mono = tokens.light.get("--mono") ?? "";

  return section(
    "Typography",
    "Two stacks. Manrope leads --ui but is not loaded: the app's CSP is default-src 'self', so the face has to ship in the bundle and does not yet — the system stack is standing in.",
    h(
      "div",
      { class: "sg-specimens" },
      h(
        "figure",
        { class: "sg-specimen", style: "margin:0" },
        h("figcaption", {}, "--ui"),
        h(
          "div",
          { class: "sg-specimen-body", style: `font-family:${ui}` },
          h("p", { class: "sg-type-sample", style: "font-size:22px;font-weight:800" }, "You're protected"),
          h("p", { class: "sg-type-sample", style: "font-size:13px;font-weight:700" }, "Aurora Networks"),
          h("p", { class: "sg-type-sample", style: "font-size:12px;font-weight:400" }, "Frankfurt · VLESS · Reality"),
          h("p", { class: "sg-type-sample", style: "font-size:10px;font-weight:400" }, "310 GB of 500 GB used · resets Oct 8"),
        ),
      ),
      h(
        "figure",
        { class: "sg-specimen", style: "margin:0" },
        h("figcaption", {}, "--mono"),
        h(
          "div",
          { class: "sg-specimen-body", style: `font-family:${mono}` },
          h("p", { class: "sg-type-sample", style: "font-size:12px" }, "utun4 · 172.19.0.1/24"),
          h("p", { class: "sg-type-sample", style: "font-size:11px" }, "[core] inbound/tun[tun-in] started"),
        ),
      ),
    ),
    h(
      "p",
      { class: "sg-note" },
      "There is no global type scale — sizes are set per component, between 9.5px and 26px. Worth" +
        " one if the interface grows; not worth inventing one here, since this page would then be" +
        " documenting a scale the app does not use.",
    ),
  );
}

// ---------------------------------------------------------------- icons

function iconsSection() {
  return section(
    "Icons",
    `All ${ICON_NAMES.length}, inline SVG on a 24 unit grid, stroked with currentColor so they take the colour of whatever contains them. Enumerated from views/icons.ts — adding one there adds it here.`,
    h(
      "div",
      { class: "sg-icons" },
      ...ICON_NAMES.map((name) =>
        h("div", { class: "sg-icon" }, icon(name, 22), h("span", {}, name)),
      ),
    ),
  );
}

// ---------------------------------------------------------------- tray

/** One input per tone, run through the real `shieldState`, so no glyph choice is restated here. */
const TRAY_INPUTS: [string, ShieldInput][] = [
  ["connected", { connection: "on", mode: "vpn", fault: null, exit: { ipv4: "192.0.2.4", ipv6: null } }],
  ["connecting", { connection: "connecting", mode: "vpn", fault: null, exit: null }],
  ["not working", { connection: "on", mode: "vpn", fault: null, exit: { ipv4: null, ipv6: null, failed: "timeout" } }],
  ["off", { connection: "off", mode: "vpn", fault: null, exit: null }],
];

/** The real tray icon's pixels, put back on a canvas at the size the bar draws them. */
function trayCanvas(input: ShieldInput, token: TokenReader, px: number): HTMLCanvasElement {
  const drawn = trayIcon(shieldState(input), token);
  const canvas = document.createElement("canvas");
  canvas.width = drawn.width;
  canvas.height = drawn.height;
  canvas.style.width = canvas.style.height = `${px}px`;
  canvas.getContext("2d")?.putImageData(new ImageData(Uint8ClampedArray.from(drawn.rgba), drawn.width), 0, 0);
  if (drawn.template) canvas.classList.add("template");
  return canvas;
}

function trayBar(caption: string, theme: "light" | "dark", token: TokenReader) {
  return h(
    "div",
    { class: "sg-tray-row" },
    h("span", {}, caption),
    h(
      "div",
      { class: `sg-bar ${theme}` },
      ...TRAY_INPUTS.map(([, input]) => trayCanvas(input, token, 18)),
      h("span", {}, "Tue 14:02"),
    ),
  );
}

function traySection(tokens: Tokens) {
  const light: TokenReader = (name) => tokens.light.get(name) ?? "";
  const dark: TokenReader = (name) => darkValue(tokens, name);
  return section(
    "Tray icon",
    "The app's mark as the menu bar and the top bar show it, drawn by trayicon.ts in the rail shield's colours: green, amber and red when it means something, dimmed when off. On macOS off is a template image, which the system draws in the menu bar's own colour — emulated here.",
    h(
      "div",
      { class: "sg-tray-big" },
      ...TRAY_INPUTS.map(([name, input]) =>
        h("figure", { style: "margin:0" }, trayCanvas(input, light, 72), h("figcaption", {}, name)),
      ),
    ),
    trayBar("macOS · light", "light", light),
    trayBar("macOS · dark", "dark", dark),
  );
}

// ---------------------------------------------------------------- components

/** Sample servers for the specimens. Hosts are under example.net, which RFC 2606 keeps unregistrable. */
function sampleServer(overrides: Partial<Server> = {}): Server {
  return {
    id: "sg-sample",
    groupId: "sg",
    profile: {
      protocol: "vless",
      name: "DE-1 Frankfurt",
      server: "fra-01.example.net",
      port: 443,
      uuid: "00000000-0000-4000-8000-000000000001",
      flow: "xtls-rprx-vision",
      security: "",
      alterId: 0,
      tls: {
        enabled: true,
        sni: "fra-01.example.net",
        insecure: false,
        alpn: [],
        fingerprint: "chrome",
        reality: { publicKey: "MOCK0PUBLIC0KEY", shortId: "0123abcd" },
      },
      transport: {
        kind: "tcp",
        path: "",
        host: "",
        serviceName: "",
        method: "",
        maxEarlyData: 0,
        earlyDataHeader: "",
      },
      password: "",
      wireguard: null,
    },
    country: "DE",
    city: "Frankfurt",
    latency: 24,
    testedAt: Date.now(),
    ...overrides,
  };
}

/**
 * A server row.
 *
 * Mirrors `row()` in views/locations.ts — the one piece of duplication on this page, because that
 * panel subscribes to the store in its constructor and cannot be mounted against a fixture. The
 * classes and the three helpers are the real ones, so only the nesting is restated.
 */
function locRow(server: Server, opts: { selected?: boolean } = {}) {
  const country = place(server.country);
  const { text, grade } = gradeLatency(server.latency);
  const strength = barCount(server.latency);
  const subtitle = [server.city, describe(server.profile)].filter(Boolean).join(" · ");

  return h(
    "button",
    {
      class: `loc${opts.selected ? " active" : ""}${server.retired ? " retired" : ""}`,
    },
    h("span", { class: "flag", style: `background:${country.flag}` }),
    h(
      "span",
      { class: "loc-main" },
      h("span", { class: "loc-name" }, country.name),
      h("span", { class: "loc-sub" }, server.retired ? `${subtitle} · retired` : subtitle),
    ),
    h(
      "span",
      { class: "loc-right" },
      h("span", { class: `bars s${strength} ping ${grade}` }, h("i", {}), h("i", {}), h("i", {})),
      h("span", { class: `ping ${grade}` }, text),
    ),
  );
}

function groupHeader(name: string, meta: string, opts: { failed?: boolean; collapsed?: boolean; sync?: boolean } = {}) {
  return h(
    "div",
    { class: "ghead" },
    h("button", { class: "gchev" }, icon(opts.collapsed ? "chevron-right" : "chevron-down", 10)),
    h(
      "span",
      { class: "gname" },
      h("b", {}, name),
      h("span", { class: `gmeta${opts.failed ? " bad" : ""}` }, meta),
    ),
    opts.sync ? h("button", { class: "gsync" }, icon("refresh", 14)) : null,
  );
}

function quotaBar(used: number, total: number, resets: string) {
  const ratio = used / total;
  return h(
    "div",
    { class: "gquota" },
    h("span", { class: "qbar" }, h("i", { class: ratio > 0.85 ? "low" : "", style: `width:${(ratio * 100).toFixed(1)}%` })),
    h("span", { class: "qtxt" }, `${size(used)} of ${size(total)} used · ${resets}`),
  );
}

/** Mounts the real StatusCard, which takes a plain model and touches no store. */
function statusSpecimen(caption: string, state: ConnectionState, extra: Partial<Parameters<StatusCard["render"]>[0]> = {}) {
  const stage = h("div", { class: "sg-stage" });
  const root = h("section", { class: "status" });
  stage.appendChild(root);

  new StatusCard(root, { onToggle: () => {}, onShare: () => {} }).render({
    state,
    // The specimens show VPN mode unless one overrides it; proxy mode gets its own row below,
    // because the two say materially different things.
    mode: "vpn",
    proxyAddress: null,
    server: sampleServer(),
    connectedAt: Date.now() - 3 * 60_000 - 12_000,
    uplink: 184_000,
    downlink: 2_400_000,
    exitIps: { ipv4: "203.0.113.42", ipv6: "2001:db8::42", cloudflare: { ip: "198.51.100.42", country: "DE" } },
    tunnelDevice: "utun4",
    blockedReason: null,
    ...extra,
  });

  return h("figure", { class: "sg-specimen", style: "margin:0" }, h("figcaption", {}, caption), stage);
}

function specimen(caption: string, ...body: Node[]) {
  return h(
    "figure",
    { class: "sg-specimen", style: "margin:0" },
    h("figcaption", {}, caption),
    h("div", { class: "sg-specimen-body" }, ...body),
  );
}

function railSpecimen(caption: string, ...body: Node[]) {
  return h(
    "figure",
    { class: "sg-specimen", style: "margin:0" },
    h("figcaption", {}, caption),
    h("div", { class: "locs sg-rail", style: "position:static;border:0;width:auto" }, ...body),
  );
}

function componentsSection() {
  const GB = 1024 ** 3;

  return section(
    "Components",
    "The states the interface actually has, including the ones that are awkward to reach by clicking. The status card is the real component; the rest reuse the real classes.",

    h("div", { class: "sg-group" }, h("h3", {}, "Status card")),
    h(
      "div",
      { class: "sg-specimens sg-wide" },
      statusSpecimen("state: off", "off", { blockedReason: null }),
      statusSpecimen("state: off · blocked", "off", {
        blockedReason: "Waiting for the core to start",
      }),
      statusSpecimen("state: connecting", "connecting", { step: "Setting the system proxy…" }),
      statusSpecimen("state: on", "on"),
      statusSpecimen("state: disconnecting", "disconnecting", { step: "Restoring the system proxy…" }),
      // The mode is not a skin on the same card: it changes what the card is allowed to claim.
      // Side by side is the only way to see that "You're protected" has no proxy-mode equivalent.
      statusSpecimen("proxy · off", "off", { mode: "proxy", blockedReason: null }),
      statusSpecimen("proxy · on", "on", {
        mode: "proxy",
        proxyAddress: "127.0.0.1:2080",
      }),
    ),

    h("div", { class: "sg-group" }, h("h3", {}, "Server rows")),
    h(
      "div",
      { class: "sg-specimens" },
      railSpecimen(
        ".loc — latency grades",
        locRow(sampleServer({ latency: 24 })),
        locRow(sampleServer({ country: "FR", city: "Paris", latency: 96 })),
        locRow(sampleServer({ country: "US", city: "New York", latency: 184 })),
        locRow(sampleServer({ country: "SG", city: "Singapore", latency: 341 })),
      ),
      railSpecimen(
        ".loc — non-numeric states",
        locRow(sampleServer({ country: "AU", city: "Sydney", latency: -1 })),
        locRow(sampleServer({ country: "CA", city: "Toronto", latency: null, testedAt: null })),
        locRow(sampleServer({ country: "FI", city: "Helsinki", latency: 31 }), { selected: true }),
        locRow(sampleServer({ country: "HK", city: "Hong Kong", latency: 203, retired: true })),
      ),
    ),

    h("div", { class: "sg-group" }, h("h3", {}, "Groups")),
    h(
      "div",
      { class: "sg-specimens" },
      railSpecimen(
        ".ghead — the three meta lines",
        groupHeader("Personal", "3 servers · added by hand"),
        groupHeader("Aurora Networks", "11 servers · updated 2 hours ago", { sync: true }),
        groupHeader("Backup Provider", "3 servers · update failed 1 week ago", {
          failed: true,
          collapsed: true,
          sync: true,
        }),
      ),
      railSpecimen(
        ".gquota — normal and past 85%",
        quotaBar(0.62 * 500 * GB, 500 * GB, "resets Oct 8"),
        quotaBar(0.91 * 100 * GB, 100 * GB, "resets Sep 23"),
        h("p", { class: "gempty" }, "Paste a share link to add one."),
      ),
    ),

    h("div", { class: "sg-group" }, h("h3", {}, "Controls")),
    h(
      "div",
      { class: "sg-specimens" },
      specimen(
        "buttons",
        h(
          "div",
          { class: "sg-row" },
          h("button", { class: "btn go" }, "Connect"),
          h("button", { class: "btn" }, "Disconnect"),
          h("button", { class: "btn brand" }, "Add servers"),
          h("button", { class: "ghost" }, "Cancel"),
          h("button", { class: "ghost", disabled: true }, "Show config"),
        ),
        h(
          "div",
          { class: "sg-row", style: "margin-top:12px" },
          h("button", { class: "addbtn" }, icon("plus", 15)),
          h("button", { class: "gsync" }, icon("refresh", 14)),
          h("button", { class: "gsync busy" }, icon("refresh", 14)),
        ),
        h("p", { class: "sg-note" }, ".gsync.busy spins; it is the only animated control."),
      ),
      specimen(
        "quick connect · search",
        h(
          "button",
          { class: "quick" },
          icon("bolt", 17),
          h("span", { class: "qt" }, h("b", {}, "Quick Connect"), h("span", {}, "Fastest, most used or most recent")),
        ),
        h(
          "label",
          { class: "search", style: "margin-top:12px" },
          icon("search", 14),
          h("input", { type: "search", placeholder: "Search country or city" }),
        ),
      ),
      specimen(
        "chips",
        h(
          "div",
          { class: "sg-row" },
          h("span", { class: "tag" }, "203.0.113.42"),
          h("span", { class: "tag" }, "VLESS · Reality"),
          h("span", { class: "tag", html: "DNS <b>no leak</b>" }),
          h("span", { class: "tag warn" }, "Waiting for the core to start"),
        ),
        h(
          "div",
          { class: "sg-row", style: "margin-top:12px" },
          h("span", { class: "badge ok" }, "ok"),
          h("span", { class: "badge bad" }, "failed"),
        ),
      ),
      specimen(
        "parsed share links",
        h(
          "div",
          { class: "prow" },
          h("span", { class: "pmark ok" }, icon("check", 11)),
          h("span", { class: "flag", style: `background:${place("DE").flag}` }),
          h("span", { class: "pmain" }, "DE-1 Frankfurt"),
        ),
        h(
          "div",
          { class: "prow" },
          h("span", { class: "pmark no" }, icon("close", 11)),
          h("span", { class: "pmain dim" }, "trojan is not supported yet"),
        ),
      ),
    ),
  );
}

// ---------------------------------------------------------------- page

function section(title: string, note: string, ...body: Node[]) {
  return h("section", { class: "sg-section" }, h("h2", {}, title), h("p", {}, note), ...body);
}

function main() {
  const root = document.getElementById("guide");
  if (!root) return;

  const tokens = readTokens();

  if (tokens.light.size === 0) {
    render(
      root,
      h(
        "div",
        { class: "sg-missing" },
        "No tokens could be read from src/styles.css. The stylesheet did not load, or the browser" +
          " refused access to its rules.",
      ),
    );
    return;
  }

  render(
    root,
    h(
      "header",
      { class: "sg-head" },
      h("h1", {}, "Nunya style guide"),
      h(
        "p",
        {},
        "Rendered from the app's own stylesheet, icon module and status card — not a description of" +
          " them. Both themes are shown side by side because the app follows the system setting and" +
          " has no switch of its own.",
      ),
    ),
    colourSection(tokens),
    elevationSection(tokens),
    typeSection(tokens),
    iconsSection(),
    traySection(tokens),
    componentsSection(),
  );
}

main();
