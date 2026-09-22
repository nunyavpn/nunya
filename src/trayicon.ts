/**
 * The tray icon: the rail's shield, as pixels for the macOS menu bar and the Linux top bar.
 *
 * The bar icon is the one piece of a VPN client on screen while its window is closed, so it is
 * worth something only if it changes with the tunnel — OpenVPN's goes grey, amber, green. Here it
 * is the rail shield: the states of `shield.ts`, the glyphs of `views/icons.ts` and the colour
 * tokens of `.logo`, so the bar and the window cannot disagree.
 *
 * It is painted here, on a canvas, rather than in Rust or as committed PNGs, because the glyphs
 * and the tokens live in the frontend: a second copy of either would drift the first time `--live`
 * changed. `Path2D` reads SVG path data as it is, so these are the rail's own paths.
 *
 * **Filled when it means something, an outline when off.** At 18pt only a filled shield with its
 * mark cut out keeps the check and the exclamation mark legible, but a filled *grey* shield is the
 * heaviest icon in the bar — backwards for the one state that needs no attention. So off is the
 * rail's outline, and on macOS a template image, which the system tints to match the menu bar in
 * light and dark appearance like every icon beside it. The other three keep their colour: the
 * colour is the point.
 *
 * Connecting is steady amber, not the rail's pulse; a bar icon that blinks demands attention that
 * nothing requires.
 */

import type { Shield, ShieldTone } from "./shield";
import { iconPaths } from "./views/icons";

export interface TrayIcon {
  width: number;
  height: number;
  /** Straight (not premultiplied) RGBA, row by row, as `getImageData` gives it. */
  rgba: number[];
  /** Draw it in the menu bar's own colour, ignoring ours. macOS only; off only. */
  template: boolean;
}

/** A colour token's value, e.g. `--live` → `#0e8c5b`. */
export type TokenReader = (name: string) => string;

/** Each coloured tone's gradient, as `.logo` paints it: from the lighter `-fill` to the base. */
const GRADIENT: Record<Exclude<ShieldTone, "off">, [string, string]> = {
  on: ["--live-fill", "--live"],
  connecting: ["--warn-fill", "--warn"],
  failed: ["--off-fill", "--off"],
};

/**
 * macOS draws a status item 18pt tall, so 36px is exact on a Retina display; a Linux panel's
 * icons are 22px, so 44px is exact at 2x there. Anything else is scaled from these.
 */
const SIZE = /Mac/.test(navigator.platform) ? 36 : 44;

/**
 * The shield's size against the 24-unit icon grid, whose margin is meant for a button. At 1.12 it
 * stands 16pt of the menu bar's 18, level with the system icons beside it.
 */
const SCALE = 1.12;

/** The outline's weight: `icon()`'s own, so off is the rail's glyph exactly. */
const OUTLINE = 1.9;

/** The cut-out mark's weight, a little heavier than the outline so it survives 18pt. */
const MARK = 2.2;

/** Reads tokens from the stylesheet, in whichever theme is in effect. */
function currentTokens(): TokenReader {
  const style = getComputedStyle(document.documentElement);
  return (name) => style.getPropertyValue(name).trim();
}

export function trayIcon(shield: Shield, token: TokenReader = currentTokens()): TrayIcon {
  const canvas = document.createElement("canvas");
  canvas.width = SIZE;
  canvas.height = SIZE;
  const g = canvas.getContext("2d", { willReadFrequently: true });
  if (!g) throw new Error("no 2D canvas to draw the tray icon on");

  g.scale(SIZE / 24, SIZE / 24);
  g.translate(12, 12);
  g.scale(SCALE, SCALE);
  g.translate(-12, -12);
  g.lineCap = "round";
  g.lineJoin = "round";

  const [outline, ...marks] = iconPaths(shield.glyph).map((d) => new Path2D(d));

  if (shield.tone === "off") {
    // `--muted`, not the rail's `--faint`: there the glyph sits on a tile, here on whatever the
    // panel is, and a Linux top bar is dark in either theme. macOS ignores it (template).
    g.strokeStyle = token("--muted");
    g.lineWidth = OUTLINE;
    for (const path of [outline, ...marks]) g.stroke(path);
  } else {
    const [from, to] = GRADIENT[shield.tone];
    // Corner to corner of the shield, as `.logo`'s 145deg runs across its tile.
    const gradient = g.createLinearGradient(4.5, 3, 19.5, 21);
    gradient.addColorStop(0, token(from));
    gradient.addColorStop(1, token(to));
    g.fillStyle = gradient;
    g.strokeStyle = gradient;
    // A thin stroke in the same colour rounds the corners, as the outline's round join does.
    g.lineWidth = 1.2;
    g.fill(outline);
    g.stroke(outline);
    // Cut out rather than painted white, as the menu bar's own glyphs are: the mark is the bar
    // showing through, and it reads on a light bar and a dark one alike.
    g.globalCompositeOperation = "destination-out";
    g.lineWidth = MARK;
    for (const path of marks) g.stroke(path);
  }

  const { data } = g.getImageData(0, 0, SIZE, SIZE);
  return { width: SIZE, height: SIZE, rgba: Array.from(data), template: shield.tone === "off" };
}
