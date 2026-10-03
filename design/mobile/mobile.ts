/**
 * The phone design board: Nunya on a phone, screen by screen, in both themes.
 *
 * A proposal to review before the phone stylesheet is rewritten, not a copy of what ships. The
 * first attempt grew the desktop window down to a phone, a rule at a time, and each round of
 * feedback found another place where a desktop habit did not fit a thumb. So this starts from the
 * phone: three labelled tabs, the connection as the home screen over the map, a server list with
 * the connection kept in reach, and sheets that rise from the bottom.
 *
 * Built from the app's own parts, so it cannot describe a different app: the colour tokens are read
 * out of `src/styles.css` (light and dark, set on each phone so the two sit side by side), the
 * icons are `views/icons.ts`, the flags `place`, the latency grades `format.ts`, and the map is the
 * real `WorldMap`. The markup is the board's own (`mb-` classes), because the layout is what is
 * being proposed. Open it with `npm run design` and go to /design/mobile/. Like the style guide it
 * is served by Vite and never built.
 */

import { h, render } from "../../src/dom";
import { bars as barCount, latency as gradeLatency } from "../../src/format";
import { place } from "../../src/geo";
import { icon } from "../../src/views/icons";
import { WorldMap, type Hop, type Pin } from "../../src/views/map";

// ---------------------------------------------------------------- tokens

type Theme = "light" | "dark";
type Tokens = Record<Theme, Map<string, string>>;

/** The custom properties of styles.css, light and dark, read from the stylesheet itself. */
function readTokens(): Tokens {
  const tokens: Tokens = { light: new Map(), dark: new Map() };
  const walk = (rule: CSSRule, dark: boolean) => {
    if (rule instanceof CSSMediaRule) {
      const inDark = dark || rule.conditionText.includes("prefers-color-scheme: dark");
      for (const inner of Array.from(rule.cssRules)) walk(inner, inDark);
      return;
    }
    if (!(rule instanceof CSSStyleRule) || !rule.selectorText.startsWith(":root")) return;
    for (const name of Array.from(rule.style)) {
      if (name.startsWith("--")) {
        (dark ? tokens.dark : tokens.light).set(name, rule.style.getPropertyValue(name).trim());
      }
    }
  };
  for (const sheet of Array.from(document.styleSheets)) {
    try {
      for (const rule of Array.from(sheet.cssRules)) walk(rule, false);
    } catch {
      // A cross-origin sheet (an extension's) cannot be read; none of ours is.
    }
  }
  return tokens;
}

const TOKENS = readTokens();

function themed(el: HTMLElement, theme: Theme): HTMLElement {
  for (const [name, value] of TOKENS.light) el.style.setProperty(name, value);
  if (theme === "dark") for (const [name, value] of TOKENS.dark) el.style.setProperty(name, value);
  return el;
}

// ---------------------------------------------------------------- the fixture

type Row = { name: string; country: string; city: string; tail: string; ms: number | null; cdn?: string; on?: boolean };

const PERSONAL: Row[] = [
  { name: "DE-1 Frankfurt", country: "DE", city: "Frankfurt", tail: "VLESS · Reality", ms: 24 },
  { name: "NL-2 Amsterdam", country: "US", city: "Ashburn · via NL", tail: "VLESS · TLS · ws", ms: 138, cdn: "CF" },
  { name: "IR-1 Tehran", country: "IR", city: "Tehran", tail: "VLESS · no TLS", ms: null },
];

const AURORA: Row[] = [
  { name: "FI-1 Helsinki", country: "FI", city: "Helsinki", tail: "VLESS · Reality", ms: 31, on: true },
  { name: "SE-1 Stockholm", country: "SE", city: "Stockholm", tail: "VLESS · Reality", ms: 44 },
  { name: "GB-3 London", country: "GB", city: "London", tail: "VLESS · TLS · ws", ms: 71, cdn: "CF" },
  { name: "FR-2 Paris", country: "FR", city: "Paris", tail: "VMess · TLS · grpc", ms: 96 },
  { name: "CH-1 Zurich", country: "CH", city: "Zurich", tail: "VLESS · Reality", ms: 112 },
];

const HOME: Hop = { lon: 9.19, lat: 45.46 };
const EXIT: Hop = { lon: 24.94, lat: 60.17 };
const DOTS: [number, number][] = [
  [8.68, 50.11], [-77.49, 39.04], [51.39, 35.69], [18.07, 59.33], [-0.13, 51.51], [2.35, 48.86], [8.54, 47.37],
];

// ---------------------------------------------------------------- parts

function flag(country: string) {
  return h("span", { class: "mb-flag", style: `background:${place(country).flag}` });
}

function ping(ms: number | null) {
  const { text, grade } = gradeLatency(ms);
  return h(
    "span",
    { class: `mb-ping ${grade}` },
    h("span", { class: `mb-bars b${barCount(ms)}` }, h("i"), h("i"), h("i")),
    text,
  );
}

function phone(theme: Theme, screen: Node, tabs: number | null, overlay?: Node) {
  const el = themed(
    h(
      "div",
      { class: "mb-phone" },
      h("div", { class: "mb-statusbar" }, h("span", {}, "9:41"), h("span", {}, h("i"), h("i"))),
      h("div", { class: "mb-screen" }, screen, overlay ?? null),
      tabs === null ? null : tabBar(tabs),
      h("div", { class: "mb-gesture" }),
    ),
    theme,
  );
  return el;
}

function tabBar(active: number) {
  const tabs: [string, string][] = [
    ["shield", "Home"],
    ["globe", "Servers"],
    ["sliders", "Settings"],
  ];
  return h(
    "nav",
    { class: "mb-tabs" },
    ...tabs.map(([glyph, label], i) =>
      h("span", { class: `mb-tab${i === active ? " on" : ""}` }, h("span", { class: "mb-pill" }, icon(glyph, 22)), label),
    ),
  );
}

// ---------------------------------------------------------------- home

type State = "off" | "connecting" | "on";

const HEADLINE: Record<State, string> = { off: "Not connected", connecting: "Connecting…", on: "Proxy running" };

function mountMap(host: HTMLElement, state: State) {
  const canvas = h("canvas") as HTMLCanvasElement;
  const pins = h("div");
  host.append(canvas, pins);
  // After the phone is in the document, so the map has a size and reads the phone's own tokens.
  requestAnimationFrame(() => {
    const map = new WorldMap(canvas, pins);
    const all: Pin[] = DOTS.map(([lon, lat]) => ({ lon, lat }));
    all.push({ ...EXIT, label: "Helsinki", active: state === "on" });
    all.push({ ...HOME, home: true, here: state !== "on", label: state === "on" ? undefined : "You · Milan" });
    map.setPins(all, state === "on" ? [HOME, EXIT] : []);
  });
}

function home(state: State, open: boolean) {
  const mapHost = h("div", { class: "mb-map" }) as HTMLElement;
  mountMap(mapHost, state);
  const toggle =
    state === "on"
      ? h("button", { class: "mb-cta stop" }, icon("power", 20), "Disconnect")
      : state === "connecting"
        ? h("button", { class: "mb-cta busy" }, h("span", { class: "mb-spin" }), "Connecting…")
        : h("button", { class: "mb-cta" }, icon("power", 20), "Connect");

  const details = open
    ? [
        h("div", { class: "mb-label" }, "Traffic"),
        h(
          "div",
          { class: "mb-flow" },
          h("div", {}, h("small", {}, "Down"), h("b", {}, "2.31", h("small", {}, "MB/s"))),
          h("div", {}, h("small", {}, "Up"), h("b", {}, "58.4", h("small", {}, "KB/s"))),
          h("button", { class: "mb-round", "aria-label": "Share" }, icon("share", 20)),
        ),
        h("div", { class: "mb-label" }, "Connection"),
        h(
          "div",
          { class: "mb-facts" },
          h("div", { class: "mb-fact strong" }, h("span", { class: "mb-k" }, "Listening"), h("code", {}, "SOCKS / HTTP 127.0.0.1:2080")),
          h("div", { class: "mb-fact" }, h("span", { class: "mb-k" }, "Exit"), flag("FI"), h("code", {}, "Helsinki · Example Hosting · AS64500")),
          h("div", { class: "mb-fact" }, h("span", { class: "mb-k" }, "IPv4"), h("code", {}, "192.0.2.4")),
          h("div", { class: "mb-fact" }, h("span", { class: "mb-k" }, "IPv6"), h("code", {}, "2001:db8::24")),
          h("div", { class: "mb-fact" }, h("span", { class: "mb-k" }, "Protocol"), h("code", {}, "VLESS · Reality")),
        ),
      ]
    : [];

  const quick = h(
    "div",
    {},
    h("div", { class: "mb-label" }, "Quick connect"),
    h(
      "div",
      { class: "mb-quick" },
      h("div", {}, h("span", {}, icon("bolt", 18)), h("b", {}, "Fastest"), h("small", {}, "DE-1 Frankfurt")),
      h("div", {}, h("span", {}, icon("chart", 18)), h("b", {}, "Most used"), h("small", {}, "FI-1 Helsinki")),
      h("div", {}, h("span", {}, icon("clock", 18)), h("b", {}, "Recent"), h("small", {}, "SE-1 Stockholm")),
    ),
  );

  const sheet = h(
    "div",
    { class: "mb-sheet" },
    h("div", { class: "mb-handle" }),
    h(
      "div",
      { class: "mb-state" },
      h("span", { class: `mb-state-icon ${state}` }, icon(state === "on" ? "shield-check" : "shield", 24)),
      h(
        "div",
        {},
        h("b", { class: state }, HEADLINE[state]),
        h("span", {}, state === "on" ? "Finland · Helsinki · 00:12:40" : state === "connecting" ? "Starting the tunnel…" : "Choose a server, then connect"),
      ),
    ),
    h(
      "div",
      { class: "mb-server" },
      flag("FI"),
      h("div", { class: "mb-server-text" }, h("b", {}, "FI-1 Helsinki"), h("span", {}, "Aurora Networks · VLESS · Reality")),
      ping(31),
      h("span", { class: "mb-chev" }, icon("chevron-right", 18)),
    ),
    toggle,
    // Proxy mode's honesty is not a swipe away: it is on the sheet, closed or open.
    state === "on" ? h("div", { class: "mb-warn" }, icon("eye-off", 16), "Only apps set to use the proxy are covered") : null,
    state === "off" ? quick : null,
    ...details,
  );

  const chip = h(
    "span",
    { class: `mb-chip ${state}` },
    h("span", { class: "mb-dot" }),
    state === "on" ? "Connected" : state === "connecting" ? "Connecting" : "Off",
  );
  return h(
    "div",
    { class: "mb-screen" },
    mapHost,
    h("div", { class: "mb-topbar" }, chip, h("span", { class: "mb-chip mode" }, icon("network", 16), "Proxy")),
    sheet,
  );
}

// ---------------------------------------------------------------- servers

function row(r: Row) {
  return h(
    "div",
    { class: `mb-loc${r.on ? " on" : ""}` },
    flag(r.country),
    h(
      "div",
      { class: "mb-server-text" },
      h("b", {}, r.name, r.cdn ? h("span", { class: "mb-cdn" }, `CDN ${r.cdn}`) : null),
      h("span", {}, `${r.city} · ${r.tail}`),
    ),
    ping(r.ms),
    h("button", { class: "mb-icon-btn", "aria-label": `Actions for ${r.name}` }, icon("more", 20)),
  );
}

function servers(connected: boolean) {
  return h(
    "div",
    { class: "mb-list-screen" },
    h(
      "div",
      { class: "mb-title" },
      h("h3", {}, "Servers"),
      h("button", { class: "mb-fab-small", "aria-label": "Add servers" }, icon("plus", 22)),
    ),
    h("div", { class: "mb-search" }, icon("search", 20), "Search name, country or city"),
    h(
      "div",
      { class: "mb-group" },
      h("span", { class: "mb-chev" }, icon("chevron-down", 16)),
      h("div", { class: "mb-group-text" }, h("b", {}, "Personal"), h("span", {}, "3 servers · added by hand")),
      h("button", { class: "mb-icon-btn" }, icon("more", 20)),
    ),
    ...PERSONAL.map(row),
    h(
      "div",
      { class: "mb-group" },
      h("span", { class: "mb-chev" }, icon("chevron-down", 16)),
      h("div", { class: "mb-group-text" }, h("b", {}, "Aurora Networks"), h("span", {}, "12 servers · updated 2 h ago")),
      h("button", { class: "mb-icon-btn" }, icon("refresh", 19)),
      h("button", { class: "mb-icon-btn" }, icon("more", 20)),
    ),
    h("div", { class: "mb-quota" }, h("div", {}, h("i", { style: "width:62%" })), h("span", {}, "310 GB of 500 GB · resets Oct 22")),
    ...AURORA.map(row),
    h("div", { class: "mb-fade" }),
    mini(connected),
  );
}

/** The connection, one line, over the list: Connect is never a tab away. */
function mini(connected: boolean) {
  return h(
    "div",
    { class: "mb-mini", style: "position:absolute;left:0;right:0;bottom:0" },
    h("span", { class: `mb-state-icon ${connected ? "on" : ""}`, style: "width:40px;height:40px;border-radius:12px" }, icon(connected ? "shield-check" : "shield", 20)),
    h(
      "div",
      { class: "mb-mini-text" },
      h("b", { class: connected ? "on" : "" }, connected ? "Proxy running" : "Not connected"),
      h("span", {}, connected ? "FI-1 Helsinki · only apps set to use it" : "FI-1 Helsinki"),
    ),
    h("button", { class: `mb-mini-btn${connected ? " stop" : ""}` }, connected ? "Stop" : "Connect"),
  );
}

function actionsSheet() {
  const action = (glyph: string, text: string, danger = false) =>
    h("div", { class: `mb-action${danger ? " danger" : ""}` }, h("span", {}, icon(glyph, 20)), text);
  return h(
    "div",
    { class: "mb-scrim" },
    h(
      "div",
      { class: "mb-modal" },
      h("div", { class: "mb-handle" }),
      h(
        "div",
        { class: "mb-modal-head" },
        flag("DE"),
        h("div", {}, h("b", {}, "DE-1 Frankfurt"), h("span", {}, "de1.example.net:443 · VLESS · Reality")),
      ),
      action("power", "Connect"),
      action("refresh", "Check"),
      action("chart", "Usage"),
      action("share", "Share"),
      action("pencil", "Edit"),
      h("div", { class: "mb-sep" }),
      action("trash", "Delete…", true),
    ),
  );
}

function addSheet() {
  return h(
    "div",
    { class: "mb-scrim" },
    h(
      "div",
      { class: "mb-modal" },
      h("div", { class: "mb-handle" }),
      h("div", { class: "mb-modal-head" }, h("b", {}, "Add servers"), h("span", { style: "margin-left:auto;color:var(--faint)" }, icon("close", 18))),
      h("div", { class: "mb-seg" }, h("span", { class: "on" }, "Link"), h("span", {}, "Scan QR"), h("span", {}, "Manual")),
      h("div", { class: "mb-paste" }, icon("clipboard", 26), "Paste from clipboard", h("small", {}, "Share links, a subscription URL, or a WireGuard config")),
      h(
        "div",
        { class: "mb-found" },
        h("div", { class: "mb-label", style: "margin-top:0" }, "Found · 1 subscription · 2 servers"),
        h(
          "div",
          { class: "mb-facts" },
          h("div", { class: "mb-fact" }, icon("globe", 18), h("code", {}, "My provider · subscription")),
          h("div", { class: "mb-fact" }, flag("DE"), h("code", {}, "DE-2 Frankfurt · de2.example.net")),
          h("div", { class: "mb-fact" }, icon("network", 18), h("code", {}, "Office WireGuard · vpn.example.net")),
        ),
      ),
      h("div", { class: "mb-modal-foot" }, h("button", { class: "mb-ghost" }, "Cancel"), h("button", { class: "mb-cta" }, "Add 3")),
    ),
  );
}

function consentSheet() {
  const fact = (glyph: string, title: string, text: string) =>
    h("div", { class: "mb-fact-row" }, h("span", {}, icon(glyph, 18)), h("div", {}, h("b", {}, title), text));
  return h(
    "div",
    { class: "mb-scrim" },
    h(
      "div",
      { class: "mb-modal" },
      h("div", { class: "mb-handle" }),
      h(
        "div",
        { class: "mb-explain" },
        h("span", { class: "mb-state-icon", style: "color:var(--brand);background:var(--brand-soft)" }, icon("shield", 24)),
        h("h4", {}, "Allow VPN mode"),
        h("p", {}, "To carry every app on this phone through the tunnel, Android has to let Nunya set up a VPN. It asks you once."),
        fact("check", "Android asks, not Nunya.", "The next screen is the system's own; you can turn it off in Settings any time."),
        fact("eye-off", "Nothing leaves this phone.", "No account, no telemetry. Your servers stay on the device."),
        fact("network", "Prefer not to?", "Proxy mode needs no permission, but covers only apps set to use it."),
      ),
      h("div", { class: "mb-modal-foot" }, h("button", { class: "mb-ghost" }, "Not now"), h("button", { class: "mb-cta" }, "Continue")),
    ),
  );
}

// ---------------------------------------------------------------- settings

function settings() {
  const set = (glyph: string, title: string, sub: string | null, right: Node) =>
    h(
      "div",
      { class: "mb-set" },
      h("span", { class: "mb-set-icon" }, icon(glyph, 18)),
      h("div", { class: "mb-set-text" }, h("b", {}, title), sub ? h("span", {}, sub) : null),
      right,
    );
  const go = (value = "") => h("span", { class: "mb-val" }, value, icon("chevron-right", 16));
  const sw = (on: boolean) => h("span", { class: `mb-switch${on ? " on" : ""}` });
  return h(
    "div",
    { class: "mb-settings" },
    h("div", { class: "mb-title", style: "background:var(--ground)" }, h("h3", {}, "Settings")),
    h("div", { class: "mb-group-label" }, "Connection"),
    h(
      "div",
      { class: "mb-card" },
      set("network", "Mode", "Proxy: only apps set to use it", h("span", { class: "mb-mode" }, h("span", { class: "on" }, "Proxy"), h("span", {}, "VPN"))),
      set("lock", "Port", "SOCKS and HTTP on one port", h("span", { class: "mb-val" }, "2080")),
      set("globe", "Bypass rules", "Leave on your normal connection", go("4")),
    ),
    h("div", { class: "mb-group-label" }, "Blocking"),
    h(
      "div",
      { class: "mb-card" },
      set("ban", "Ad blocker", "List updated 3 h ago", sw(true)),
      set("eye-off", "Anti-tracker", "Downloaded when you turn it on", sw(false)),
    ),
    h("div", { class: "mb-group-label" }, "App"),
    h(
      "div",
      { class: "mb-card" },
      set("cloud", "DNS", "https://1.1.1.1/dns-query", go()),
      set("activity", "Diagnostics", "Log, config, engine", go()),
      set("heart", "Support Nunya", null, go()),
      set("open", "About", "0.8.6 · updated by F-Droid", go()),
    ),
  );
}

// ---------------------------------------------------------------- the board

function section(title: string, note: string, shots: [string, (theme: Theme) => Node][]) {
  return h(
    "section",
    { class: "mb-section" },
    h("h2", {}, title),
    h("p", {}, note),
    h(
      "div",
      { class: "mb-row" },
      ...shots.flatMap(([caption, draw]) =>
        (["light", "dark"] as Theme[]).map((theme) =>
          h("figure", { class: "mb-shot", style: "margin:0" }, draw(theme), h("figcaption", {}, `${caption} · ${theme}`)),
        ),
      ),
    ),
  );
}

render(
  document.getElementById("board")!,
  h(
    "header",
    { class: "mb-intro" },
    h("h1", {}, "Nunya on a phone"),
    h(
      "p",
      {},
      "A proposal, to be agreed before the app's phone layout is rebuilt to match. The desktop window has three panes side by side; a phone gets one screen at a time, three labelled tabs, and sheets from the bottom edge. Every colour, icon, flag and the map are the app's own.",
    ),
    h(
      "div",
      { class: "mb-spec" },
      h("div", {}, h("b", {}, "Tabs"), h("span", {}, "Home (the connection over the map) · Servers · Settings. Bypass rules, Diagnostics and Support move into Settings. Labels under every icon.")),
      h("div", {}, h("b", {}, "Touch"), h("span", {}, "Every target 44px or more; rows 64px; the Connect button 56px and full width, where the thumb rests.")),
      h("div", {}, h("b", {}, "Spacing"), h("span", {}, "16px gutters; cards 18–24px corners; nothing meets the screen's edge but the map and the list's selected row.")),
      h("div", {}, h("b", {}, "Type"), h("span", {}, "Titles 30/800 · state 22/800 · rows 16/700 with 13 under · labels 11.5 caps · fields 16, so a phone never zooms the page.")),
      h("div", {}, h("b", {}, "The connection"), h("span", {}, "On Home, a sheet over the map: closed it is the state, the server and Connect; dragged up it is traffic and addresses. On every other tab, a one-line bar above the tabs.")),
      h("div", {}, h("b", {}, "Honesty"), h("span", {}, "Proxy mode's “only apps set to use it” stays on the sheet and the bar, closed or open: the headline never says more than the mode covers.")),
    ),
  ),
  section("Home", "The map fills the screen; the connection is a sheet over it. Pinch and drag move the map; there are no zoom buttons.", [
    ["Off", (t) => phone(t, home("off", false), 0)],
    ["Connecting", (t) => phone(t, home("connecting", false), 0)],
    ["Connected", (t) => phone(t, home("on", false), 0)],
    ["Connected · sheet dragged up", (t) => phone(t, home("on", true), 0)],
  ]),
  section("Servers", "One list, full width. The selected server is tinted edge to edge, under its own ⋯. The connection rides above the tabs, so Connect is one tap away from any row.", [
    ["Servers · connected", (t) => phone(t, servers(true), 1)],
    ["Row actions", (t) => phone(t, servers(false), 1, actionsSheet())],
  ]),
  section("Sheets", "Everything that was a dialog rises from the bottom, with its actions at thumb height.", [
    ["Add servers", (t) => phone(t, servers(false), 1, addSheet())],
    ["VPN mode, before Android asks", (t) => phone(t, home("off", false), 0, consentSheet())],
  ]),
  section("Settings", "Grouped rows in place of the desktop's long panel. Mode is a switch between two named things, never a toggle, because they cover different amounts of the phone.", [
    ["Settings", (t) => phone(t, settings(), 2)],
  ]),
);
