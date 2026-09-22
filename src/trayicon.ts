/**
 * The tray icon: the app's own mark, as pixels for the macOS menu bar and the Linux top bar,
 * coloured by the connection's state.
 *
 * The bar icon is the one piece of a VPN client on screen while its window is closed, so it is
 * worth something only if it changes with the tunnel — OpenVPN's goes grey, amber, green. Here it
 * is the mark from `design/nonya.png` (the arch and the road, `MARK` in `views/icons.ts`) in the
 * rail shield's four states and colours (`shield.ts`, `.logo`'s tokens): so the bar says both
 * which app this is and whether it is working, and cannot disagree with the window about the
 * second.
 *
 * It is painted here, on a canvas, rather than in Rust or as committed PNGs, because the path and
 * the tokens live in the frontend: a second copy of either would drift the first time `--live`
 * changed. `Path2D` reads SVG path data as it is.
 *
 * **Coloured when it means something, dimmed when off.** Connected, connecting and not working are
 * green, amber and red: the colour is the point. Off is the mark at half strength, and on macOS a
 * template image, which the system tints to match the menu bar in light and dark appearance like
 * every icon beside it — present, but asking for nothing.
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
 * A stroke in the mark's own colour, in grid units, to thicken it. Traced faithfully, the road's
 * far end and the arch come out thinner at 18pt than the system icons beside them; much more and
 * the road runs into the arch.
 */
const WEIGHT = 0.8;

/** How strong off is: enough to find the icon, not enough to read as a state. */
const OFF_ALPHA = 0.55;

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

  // The mark fills 22 of the grid's 24 units; shrunk by the stroke's width so the stroke stays
  // inside the square too.
  g.scale(SIZE / 24, SIZE / 24);
  g.translate(12, 12);
  g.scale(1 - WEIGHT / 24, 1 - WEIGHT / 24);
  g.translate(-12, -12);
  g.lineJoin = "round";
  g.lineWidth = WEIGHT;

  if (shield.tone === "off") {
    // `--muted`, not the rail's `--faint`: there the glyph sits on a tile, here on whatever the
    // panel is, and a Linux top bar is dark in either theme. macOS keeps only the alpha (template).
    g.fillStyle = token("--muted");
    g.strokeStyle = token("--muted");
    g.globalAlpha = OFF_ALPHA;
  } else {
    const [from, to] = GRADIENT[shield.tone];
    // Corner to corner, as `.logo`'s 145deg runs across its tile.
    const gradient = g.createLinearGradient(1, 1, 23, 23);
    gradient.addColorStop(0, token(from));
    gradient.addColorStop(1, token(to));
    g.fillStyle = gradient;
    g.strokeStyle = gradient;
  }

  for (const d of iconPaths("mark")) {
    const path = new Path2D(d);
    g.fill(path);
    g.stroke(path);
  }

  const { data } = g.getImageData(0, 0, SIZE, SIZE);
  return { width: SIZE, height: SIZE, rgba: Array.from(data), template: shield.tone === "off" };
}
