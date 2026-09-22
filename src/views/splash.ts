/**
 * The splash screen: the mark, animated, while the app gets ready at launch.
 *
 * Three things have to be true before the window is worth looking at: the data file has loaded
 * (or the list flashes empty), the core is up (or Connect is refused), and this machine has been
 * placed (or the map has no "you" to draw routes from). Each takes from a moment to a few seconds,
 * and without this the window arrived in pieces. The splash covers it until all three are done, or
 * `MAX_MS` has passed, whichever is first: a slow geo lookup must not hold the app hostage, and a
 * core that never comes up is the status card's to explain, not the splash's to hide. Something
 * the window has to show at once — a data file that would not load — takes it down straight away
 * (`leave`).
 *
 * The animation is the mark being made: the arch draws itself, the road runs out of the tunnel,
 * rings recede into it, and a light passes over the whole; leaving, it flies into the tunnel as it
 * fades. It is drawn from the `mark` icon's own paths (`icons.ts`), so it cannot drift from the
 * tray icon, in the artwork's own colours (`design/nonya.png`). Under reduced motion it is the
 * finished mark, still.
 *
 * `index.html` has the cover from the first paint — a plain field in the page's colour — so the
 * empty window never shows before this script has run; this only fills it in.
 */

import { h, render, svg } from "../dom";
import { iconPaths } from "./icons";

/** The least time on screen, so the mark is finished before it goes rather than cut off. */
const MIN_MS = 1700;

/** The most: past this, what is still missing is the window's to say. */
const MAX_MS = 6000;

/** How long leaving takes; must match the `.splash` transition in `styles.css`. */
const LEAVE_MS = 450;

/** The artwork's gradient (`design/nonya.png`): cyan at the top left to violet at the bottom right. */
const INK: [string, string][] = [
  ["0", "#4bcdfd"],
  ["0.55", "#2f6ff6"],
  ["1", "#4428fa"],
];

/** The tunnel on the mark's grid: the arch's centre, and a radius just inside its inner edge. */
const TUNNEL = { cx: 12, cy: 11, r: 6.4 };

/** Rings receding into the tunnel at once; staggered, they read as a steady flow. */
const RINGS = 4;

export interface SplashStep {
  /** What the line says while this is what the splash is waiting for. */
  line: string;
  done: Promise<unknown>;
}

export class Splash {
  private line: HTMLElement;
  private leaving = false;

  constructor(private root: HTMLElement) {
    this.line = h("span", { class: "splash-line", role: "status" });
    render(root, art(), h("span", { class: "splash-word" }, "Nunya"), this.line);
    root.setAttribute("aria-busy", "true");
  }

  /** Waits for each step in turn — saying which one it is on — then leaves. */
  async wait(steps: SplashStep[]): Promise<void> {
    const all = (async () => {
      for (const step of steps) {
        if (this.leaving) return;
        this.line.textContent = step.line;
        // A step that fails is done too: the window says what went wrong.
        await step.done.catch(() => {});
      }
    })();
    // Measured from the page's start, which is when the cover first painted.
    await Promise.race([all, after(MAX_MS - performance.now())]);
    await after(MIN_MS - performance.now());
    this.leave();
  }

  /** Goes: at once when called directly, for something the window has to show now. */
  leave() {
    if (this.leaving) return;
    this.leaving = true;
    this.root.removeAttribute("aria-busy");
    this.root.classList.add("gone");
    setTimeout(() => this.root.remove(), LEAVE_MS + 50);
  }
}

function after(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, Math.max(0, ms)));
}

function art(): SVGElement {
  const [arch, road] = iconPaths("mark");
  return svg(
    "svg",
    { class: "splash-mark", viewBox: "0 0 24 24", "aria-hidden": "true" },
    svg(
      "defs",
      {},
      svg(
        "linearGradient",
        { id: "splash-ink", x1: "0", y1: "0", x2: "1", y2: "1" },
        ...INK.map(([offset, color]) => svg("stop", { offset, "stop-color": color })),
      ),
      svg(
        "linearGradient",
        { id: "splash-shine", x1: "0", y1: "0", x2: "1", y2: "0" },
        svg("stop", { offset: "0", "stop-color": "#fff", "stop-opacity": "0" }),
        svg("stop", { offset: "0.5", "stop-color": "#fff", "stop-opacity": "0.6" }),
        svg("stop", { offset: "1", "stop-color": "#fff", "stop-opacity": "0" }),
      ),
      svg("clipPath", { id: "splash-shape" }, svg("path", { d: arch }), svg("path", { d: road })),
    ),
    // Beneath the arch and the road, so the road runs out over them.
    svg(
      "g",
      { class: "splash-rings" },
      ...Array.from({ length: RINGS }, (_, i) =>
        svg("circle", {
          class: "splash-ring",
          ...TUNNEL,
          style: `animation-delay: ${((i * 2.6) / RINGS).toFixed(2)}s`,
        }),
      ),
    ),
    // `pathLength` 1, so drawing is a dash offset from 1 to 0 whatever the outline's length.
    svg("path", { class: "splash-arch", d: arch, pathLength: 1 }),
    svg("path", { class: "splash-road", d: road, pathLength: 1 }),
    svg(
      "g",
      { "clip-path": "url(#splash-shape)" },
      svg("rect", {
        class: "splash-sheen",
        x: -10,
        y: 0,
        width: 7,
        height: 24,
        fill: "url(#splash-shine)",
      }),
    ),
  );
}
