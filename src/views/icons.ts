/**
 * Inline SVG icons.
 *
 * Inline rather than an icon font or sprite sheet: the app's CSP forbids remote resources, these
 * are a few hundred bytes in total, and inlining means they inherit `currentColor` and need no
 * loading state.
 */

import { svg } from "../dom";

type Icon = { paths: string[]; circles?: [number, number, number][]; fill?: boolean };

const ICONS: Record<string, Icon> = {
  plus: { paths: ["M12 5v14M5 12h14"] },
  search: { paths: ["M16.5 16.5 21 21"], circles: [[10.5, 10.5, 6.5]] },
  bolt: { paths: ["M13 2 4.5 13.4H11l-1 8.6 8.5-11.4H12Z"], fill: true },
  refresh: { paths: ["M20 12a8 8 0 1 1-2.4-5.7", "M20 4v4.4h-4.4"] },
  "chevron-down": { paths: ["M4 8l6 6 6-6"] },
  "chevron-right": { paths: ["M8 4l6 6-6 6"] },
  shield: { paths: ["M12 3 19.5 6v6c0 4.4-3.2 7.8-7.5 9-4.3-1.2-7.5-4.6-7.5-9V6Z"] },
  "shield-check": {
    paths: ["M12 3 19.5 6v6c0 4.4-3.2 7.8-7.5 9-4.3-1.2-7.5-4.6-7.5-9V6Z", "M8.8 12.2 11 14.4l4.4-4.6"],
  },
  globe: {
    paths: [
      "M3.5 12h17",
      "M12 3.5c2.4 2.4 3.6 5.3 3.6 8.5s-1.2 6.1-3.6 8.5c-2.4-2.4-3.6-5.3-3.6-8.5S9.6 5.9 12 3.5Z",
    ],
    circles: [[12, 12, 8.5]],
  },
  activity: { paths: ["M3 14.5l4.5-6 3.5 4 3-8 3.5 10h3.5"] },
  sliders: {
    paths: ["M4 7h8M17.5 7H20", "M4 12h10M19 12h1", "M4 17h5M14.5 17H20"],
    circles: [
      [14.7, 7, 2.2],
      [16.7, 12, 2.2],
      [11.7, 17, 2.2],
    ],
  },
  clipboard: { paths: [], circles: [] },
  power: { paths: ["M12 3.2v8.4", "M6.9 6.6a7.4 7.4 0 1 0 10.2 0"] },
  close: { paths: ["M5 5l14 14M19 5L5 19"] },
  check: { paths: ["M5 12.5 9.5 17 19 7"] },
  network: { paths: ["M6.6 7.8h.01M6.6 16.3h.01"] },
  lock: { paths: ["M8.2 10.5V7.6a3.8 3.8 0 0 1 7.6 0v2.9"] },
  cloud: { paths: ["M7 18.5h10.5a4 4 0 0 0 .6-7.95A6 6 0 0 0 6.6 9.2 4.7 4.7 0 0 0 7 18.5Z"] },
  more: { paths: ["M6 12h.01M12 12h.01M18 12h.01"] },
  scan: {
    paths: [
      "M4 8.5V5.5A1.5 1.5 0 0 1 5.5 4h3",
      "M15.5 4h3A1.5 1.5 0 0 1 20 5.5v3",
      "M20 15.5v3a1.5 1.5 0 0 1-1.5 1.5h-3",
      "M8.5 20h-3A1.5 1.5 0 0 1 4 18.5v-3",
      "M4 12h16",
    ],
  },
  share: { paths: ["M12 15V3.5", "M7.5 8 12 3.5 16.5 8", "M5 12.5V20h14v-7.5"] },
  pencil: { paths: ["M4 20h4L19 9a2.5 2.5 0 0 0-3.5-3.5L4.5 16.5 4 20Z", "M14.5 6.5 17.5 9.5"] },
  trash: {
    paths: [
      "M4.5 6.5h15",
      "M9 6.5V4.8h6v1.7",
      "M6.5 6.5 7.4 20h9.2l.9-13.5",
      "M10.3 10v6M13.7 10v6",
    ],
  },
};

/** Icons whose shape is a rectangle rather than a path. */
const RECTS: Record<string, [number, number, number, number, number][]> = {
  clipboard: [
    [5, 5, 14, 16, 2.6],
    [9, 2.6, 6, 4.2, 1.4],
  ],
  network: [
    [3, 4.5, 18, 6.5, 2],
    [3, 13, 18, 6.5, 2],
  ],
  lock: [[4.5, 10.5, 15, 9.5, 2.5]],
};

/**
 * Every icon this app has, in declaration order.
 *
 * Exported for the style guide in `design/`, which renders the set from this list rather than from
 * a list of its own — so an icon added above appears there without anyone remembering to add it.
 */
export const ICON_NAMES: readonly string[] = Object.keys(ICONS);

export function icon(name: string, size = 16): SVGElement {
  const spec = ICONS[name];
  if (!spec) throw new Error(`unknown icon: ${name}`);

  const node = svg("svg", {
    viewBox: "0 0 24 24",
    width: size,
    height: size,
    fill: spec.fill ? "currentColor" : "none",
    stroke: spec.fill ? "none" : "currentColor",
    "stroke-width": 1.9,
    "stroke-linecap": "round",
    "stroke-linejoin": "round",
    "aria-hidden": "true",
  });

  for (const [x, y, w, h, r] of RECTS[name] ?? []) {
    node.appendChild(svg("rect", { x, y, width: w, height: h, rx: r }));
  }
  for (const d of spec.paths) {
    node.appendChild(svg("path", { d }));
  }
  for (const [cx, cy, r] of spec.circles ?? []) {
    node.appendChild(svg("circle", { cx, cy, r }));
  }
  return node;
}
