/**
 * The world map behind the main window: countries with their borders, zoomable and pannable.
 *
 * The borders are Natural Earth's (public domain), in the `world-atlas` package's TopoJSON, and
 * they ship with the app. Nothing is fetched from a map server: a tile service would learn the
 * address of every user of a VPN client each time they looked at the map, and would make the map
 * one more thing that stops working exactly when the network is being difficult. Two detail
 * levels are bundled — 1:110m (about 100 KB) drawn at world scale, and 1:50m (about 750 KB),
 * read from the app's own assets the first time the user zooms in far enough to see the
 * difference.
 *
 * Drawn on canvas as one `Path2D` in degree space, reused on every frame under a transform, so a
 * pan or a zoom re-fills one path rather than rebuilding eighty thousand points. The projection is
 * equirectangular and cropped top and bottom, as it always was: pins, routes and the city picker
 * all keep working in the same coordinates, only now through the view's zoom and offset.
 */

import url110 from "world-atlas/countries-110m.json?url";
import url50 from "world-atlas/countries-50m.json?url";

// Equirectangular, cropped top and bottom. Projecting the full -90..90 into the pane would stretch
// latitude about 2.5x and the continents stop being recognisable.
const LAT_TOP = 75;
const LAT_BOTTOM = -55;
const LAT_SPAN = LAT_TOP - LAT_BOTTOM;

/** Vertical space the status card covers; the map band centres in what is left above it. */
const CARD_RESERVE = 110;

const MIN_ZOOM = 1;
const MAX_ZOOM = 14;
/** From this zoom the 1:50m borders are drawn; below it the 1:110m ones are indistinguishable. */
const DETAIL_ZOOM = 2.5;
/** From this zoom country names are written on the map. */
const LABEL_ZOOM = 2.2;

export interface Pin {
  lon: number;
  lat: number;
  label?: string;
  active?: boolean;
  /** The device itself. Muted behind a running tunnel, prominent when it is the only dot that matters. */
  home?: boolean;
  /** With `home`: the tunnel is down, so this is where traffic actually comes from. */
  here?: boolean;
  /**
   * Makes the dot a button. The map does not know what a dot stands for — it hands the key back
   * through `onPick` and the caller decides — which keeps servers and the store out of this file.
   */
  key?: string;
  /** What the dot is, for its hover label and its accessible name: "New York · 3 servers". */
  name?: string;
  /**
   * Hangs the label under the dot. For a relay's entry, whose label would otherwise sit on top of
   * the exit's whenever the two are a country apart — which is the usual case.
   */
  labelBelow?: boolean;
  /** A node the route passes through on its way to the exit: drawn bold, like the ends. */
  hop?: boolean;
}

/** Where a picked dot is, in the map pane's own coordinates, so a card can be placed beside it. */
export interface PickPoint {
  x: number;
  y: number;
}

/** One hop of the path traffic takes: the device, then each server in order, the exit last. */
export interface Hop {
  lon: number;
  lat: number;
}

// ---------------------------------------------------------------- the borders

/** The parts of a TopoJSON topology this reads. */
interface Topology {
  transform: { scale: [number, number]; translate: [number, number] };
  arcs: number[][][];
  objects: { countries: { geometries: Geometry[] } };
}

type Geometry =
  | { type: "Polygon"; arcs: number[][]; properties?: { name?: string } }
  | { type: "MultiPolygon"; arcs: number[][][]; properties?: { name?: string } }
  | { type: null; properties?: { name?: string } };

/** A country's name and where to write it, in degree space, with the width it has to fit in. */
interface CountryLabel {
  name: string;
  x: number;
  y: number;
  width: number;
}

interface Borders {
  /** Every country's rings in one path, in degree space: x = lon + 180, y = LAT_TOP - lat. */
  path: Path2D;
  labels: CountryLabel[];
}

/**
 * Turns a TopoJSON topology into one path and a label per country.
 *
 * TopoJSON stores each border once, as a shared "arc" that neighbouring countries both refer to,
 * delta-encoded and quantised. Decoding it is thirty lines, which is less to trust than a library
 * for a format this settled.
 */
function decode(topo: Topology): Borders {
  const [sx, sy] = topo.transform.scale;
  const [tx, ty] = topo.transform.translate;
  const arcs = topo.arcs.map((arc) => {
    let x = 0;
    let y = 0;
    return arc.map(([dx, dy]) => {
      x += dx;
      y += dy;
      return [x * sx + tx + 180, LAT_TOP - (y * sy + ty)] as [number, number];
    });
  });

  // An arc index `~i` means arc `i` walked backwards; consecutive arcs share their joining point.
  //
  // A ring that crosses the antimeridian — Russia's far east, Fiji — jumps from one edge of the
  // map to the other between two neighbouring points, which drawn as-is is a line straight across
  // the world. So the longitudes are unwrapped into one continuous run, which may reach past 360°;
  // such a ring is then drawn a second time, shifted by a world's width, and the clip to the
  // map's band shows each half on its own side.
  const ring = (indices: number[]): [number, number][] => {
    const points: [number, number][] = [];
    for (const index of indices) {
      const arc = index < 0 ? [...arcs[~index]].reverse() : arcs[index];
      for (const [x, y] of points.length ? arc.slice(1) : arc) {
        const previous = points[points.length - 1];
        let ux = x;
        if (previous) {
          while (ux - previous[0] > 180) ux -= 360;
          while (ux - previous[0] < -180) ux += 360;
        }
        points.push([ux, y]);
      }
    }
    return points;
  };

  const path = new Path2D();
  const labels: CountryLabel[] = [];
  for (const geometry of topo.objects.countries.geometries) {
    if (geometry.type === null) continue;
    const polygons = geometry.type === "Polygon" ? [geometry.arcs] : geometry.arcs;

    // The label goes on the country's largest piece — mainland France, not French Guiana.
    let best = { area: 0, x: 0, y: 0, width: 0 };
    for (const polygon of polygons) {
      polygon.forEach((indices, r) => {
        const points = ring(indices);
        if (points.length < 3) return;
        let [minX, maxX] = [Infinity, -Infinity];
        for (const [x] of points) {
          minX = Math.min(minX, x);
          maxX = Math.max(maxX, x);
        }
        const shifts = [0, ...(maxX > 360 ? [-360] : []), ...(minX < 0 ? [360] : [])];
        for (const shift of shifts) {
          path.moveTo(points[0][0] + shift, points[0][1]);
          for (let i = 1; i < points.length; i++) path.lineTo(points[i][0] + shift, points[i][1]);
          path.closePath();
        }
        if (r !== 0) return; // holes do not host labels
        let [x0, y0, x1, y1] = [Infinity, Infinity, -Infinity, -Infinity];
        for (const [x, y] of points) {
          x0 = Math.min(x0, x);
          y0 = Math.min(y0, y);
          x1 = Math.max(x1, x);
          y1 = Math.max(y1, y);
        }
        const area = (x1 - x0) * (y1 - y0);
        // A label's centre is brought back onto the map when an unwrapped ring pushed it off.
        const cx = ((((x0 + x1) / 2) % 360) + 360) % 360;
        if (area > best.area) best = { area, x: cx, y: (y0 + y1) / 2, width: x1 - x0 };
      });
    }
    const name = geometry.properties?.name;
    if (name && best.area > 0) labels.push({ name, x: best.x, y: best.y, width: best.width });
  }
  return { path, labels };
}

async function loadBorders(url: string): Promise<Borders> {
  const response = await fetch(url);
  return decode((await response.json()) as Topology);
}

function token(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

// ---------------------------------------------------------------- the map

/**
 * Draws the map into a canvas and positions HTML pins over it.
 *
 * Pins are DOM rather than canvas so they can carry labels, a CSS pulse and a hit target without
 * hand-rolling any of it.
 */
export class WorldMap {
  private ctx: CanvasRenderingContext2D;
  private pins: Pin[] = [];
  private route: Hop[] = [];
  /** What was last drawn, so an unchanged frame is not redrawn. */
  private drawn = "";

  /** The whole world fitted to the pane: its left, top and pixels per degree at zoom 1. */
  private base = { x: 0, y: 0, perDegree: 1 };
  /** The view on top of it: a zoom, and the pane offset that zoom is taken from. */
  private view = { k: 1, x: 0, y: 0 };
  private size = { width: 0, height: 0 };

  private coarse: Borders | null = null;
  private fine: Borders | null = null;
  private loadingFine = false;
  private frameRequested = false;

  /** Called whenever the view moves, so anything placed against it (the city picker) can close. */
  onViewChange?: () => void;

  constructor(
    private canvas: HTMLCanvasElement,
    private pinHost: HTMLElement,
    private onPick?: (key: string, at: PickPoint) => void,
  ) {
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("canvas 2d context unavailable");
    this.ctx = ctx;

    if (typeof ResizeObserver !== "undefined") {
      new ResizeObserver(() => this.draw()).observe(canvas);
    } else {
      window.addEventListener("resize", () => this.draw());
    }

    // The land and border colours are theme tokens, so a theme change has to repaint.
    window.matchMedia?.("(prefers-color-scheme: dark)").addEventListener?.("change", () => this.draw());

    void loadBorders(url110).then((borders) => {
      this.coarse = borders;
      this.draw();
    });

    this.bindInput();
    this.addControls();
  }

  /**
   * `route` is the path, drawn as one arc per leg. Today it is the device and the exit; a proxy
   * chain adds its hops in between and nothing here changes.
   */
  setPins(pins: Pin[], route: Hop[] = []) {
    // The app refreshes once a second while connected, for the uptime line. Redrawing an
    // unchanged map each time repaints every border for nothing and rebuilds the pins under the
    // cursor, which drops their hover state — a label that blinks off every second.
    const next = JSON.stringify([pins, route]);
    if (next === this.drawn) return;
    this.drawn = next;
    this.pins = pins;
    this.route = route;
    this.draw();
  }

  // ---------------------------------------------------------------- the view

  private get scale(): number {
    return this.base.perDegree * this.view.k;
  }

  /** Pane x of degree-space x. */
  private sx(x: number): number {
    return this.view.x + this.view.k * this.base.x + this.scale * x;
  }

  private sy(y: number): number {
    return this.view.y + this.view.k * this.base.y + this.scale * y;
  }

  private px(lon: number): number {
    return this.sx(lon + 180);
  }

  private py(lat: number): number {
    return this.sy(LAT_TOP - lat);
  }

  private fitBase(width: number, height: number) {
    const w = width * 1.06; // a touch of bleed past the pane edges at the widest view
    const h = w * (LAT_SPAN / 360);
    const free = Math.max(height - CARD_RESERVE, h);
    this.base = { x: (width - w) / 2, y: (free - h) / 2, perDegree: w / 360 };
  }

  /**
   * Zooms by `factor` about a point in the pane, keeping whatever is under that point there —
   * the behaviour every map has taught people to expect from a scroll wheel.
   */
  private zoomAt(factor: number, px: number, py: number) {
    const k = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, this.view.k * factor));
    if (k === this.view.k) return;
    const ratio = k / this.view.k;
    this.view = { k, x: px - (px - this.view.x) * ratio, y: py - (py - this.view.y) * ratio };
    this.moved();
  }

  private panBy(dx: number, dy: number) {
    this.view = { ...this.view, x: this.view.x + dx, y: this.view.y + dy };
    this.moved();
  }

  private reset() {
    this.view = { k: 1, x: 0, y: 0 };
    this.moved();
  }

  /**
   * Keeps the world on screen: an edge can be dragged as far as the middle of the view, never
   * past it, so there is always map to look at.
   *
   * Deliberately looser than "an edge may never come inside the pane". That stricter rule pulls
   * the map whenever the zoom point is near an edge — and Europe, where most servers are, sits
   * near the top — so the place under the cursor slid away while zooming into it.
   */
  private clamp() {
    const { width, height } = this.size;
    const k = this.view.k;
    const floor = height - CARD_RESERVE;
    const left = k * this.base.x;
    const right = k * (this.base.x + 360 * this.base.perDegree);
    const top = k * this.base.y;
    const bottom = k * (this.base.y + LAT_SPAN * this.base.perDegree);

    // On screen, left edge = view.x + left: it may not pass the middle going right, and the right
    // edge may not pass it going left. The same vertically, about the middle of the map area.
    this.view.x = Math.min(Math.max(this.view.x, width / 2 - right), width / 2 - left);
    this.view.y = Math.min(Math.max(this.view.y, floor / 2 - bottom), floor / 2 - top);
    // At zoom 1 the world sits exactly where it always did.
    if (k === 1) this.view = { k: 1, x: 0, y: 0 };
  }

  private moved() {
    this.clamp();
    this.onViewChange?.();
    if (this.view.k >= DETAIL_ZOOM && !this.fine && !this.loadingFine) {
      this.loadingFine = true;
      void loadBorders(url50).then((borders) => {
        this.fine = borders;
        this.draw();
      });
    }
    // One draw per frame however many wheel or pointer events arrive in it.
    if (this.frameRequested) return;
    this.frameRequested = true;
    requestAnimationFrame(() => {
      this.frameRequested = false;
      this.draw();
    });
  }

  // ---------------------------------------------------------------- input

  /**
   * Wheel, drag and double-click, taken on the whole pane rather than the canvas: the pins sit
   * above the canvas, and a map that stops zooming whenever the cursor is over a city — which is
   * exactly where people zoom — is broken. The status card, the picker and the zoom buttons keep
   * their own input.
   */
  private bindInput() {
    const canvas = this.canvas;
    const pane = canvas.parentElement ?? canvas;
    const local = (e: MouseEvent) => {
      const box = canvas.getBoundingClientRect();
      return [e.clientX - box.left, e.clientY - box.top] as const;
    };
    const onMap = (e: Event) => !(e.target as Element).closest?.(".status, .mpick, .mapzoom");

    pane.addEventListener(
      "wheel",
      (e) => {
        if (!onMap(e)) return;
        e.preventDefault();
        const [x, y] = local(e);
        // Trackpads send many small deltas, wheels a few large ones; exp() treats both evenly.
        this.zoomAt(Math.exp(-e.deltaY * 0.0022), x, y);
      },
      { passive: false },
    );

    pane.addEventListener("dblclick", (e) => {
      if (!onMap(e) || (e.target as Element).closest?.(".pin")) return;
      const [x, y] = local(e);
      this.zoomAt(e.shiftKey ? 0.5 : 2, x, y);
    });

    let dragging: { x: number; y: number } | null = null;
    pane.addEventListener("pointerdown", (e) => {
      // A pin is a button: pressing it picks it rather than starting a drag.
      if (e.button !== 0 || !onMap(e) || (e.target as Element).closest?.(".pin")) return;
      dragging = { x: e.clientX, y: e.clientY };
      canvas.setPointerCapture(e.pointerId);
      canvas.classList.add("dragging");
    });
    canvas.addEventListener("pointermove", (e) => {
      if (!dragging) return;
      this.panBy(e.clientX - dragging.x, e.clientY - dragging.y);
      dragging = { x: e.clientX, y: e.clientY };
    });
    const stop = (e: PointerEvent) => {
      if (!dragging) return;
      dragging = null;
      canvas.releasePointerCapture(e.pointerId);
      canvas.classList.remove("dragging");
    };
    canvas.addEventListener("pointerup", stop);
    canvas.addEventListener("pointercancel", stop);
  }

  /** + / − / reset, over the map's top-right corner, for anyone without a wheel. */
  private addControls() {
    const host = this.canvas.parentElement;
    if (!host) return;
    const button = (label: string, text: string, run: () => void) => {
      const b = document.createElement("button");
      b.type = "button";
      b.setAttribute("aria-label", label);
      b.textContent = text;
      b.addEventListener("click", run);
      return b;
    };
    const centre = () => [this.size.width / 2, (this.size.height - CARD_RESERVE) / 2] as const;
    const controls = document.createElement("div");
    controls.className = "mapzoom";
    controls.append(
      button("Zoom in", "+", () => this.zoomAt(1.6, ...centre())),
      button("Zoom out", "−", () => this.zoomAt(1 / 1.6, ...centre())),
      button("Show the whole world", "⤢", () => this.reset()),
    );
    host.appendChild(controls);
  }

  // ---------------------------------------------------------------- drawing

  draw() {
    const box = this.canvas.getBoundingClientRect();
    if (!box.width || !box.height) return;

    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    const width = Math.round(box.width * dpr);
    const height = Math.round(box.height * dpr);
    if (this.canvas.width !== width || this.canvas.height !== height) {
      this.canvas.width = width;
      this.canvas.height = height;
    }
    this.size = { width: box.width, height: box.height };
    this.fitBase(box.width, box.height);
    this.clamp();

    const ctx = this.ctx;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, box.width, box.height);

    const borders = (this.view.k >= DETAIL_ZOOM && this.fine) || this.coarse;
    if (borders) {
      const s = this.scale;
      ctx.save();
      ctx.setTransform(dpr * s, 0, 0, dpr * s, dpr * this.sx(0), dpr * this.sy(0));
      // The map's band: one world wide, cropped at the latitudes the projection keeps. Anything
      // outside — Antarctica, the far Arctic, the shifted copies of antimeridian rings — is cut.
      ctx.beginPath();
      ctx.rect(0, 0, 360, LAT_SPAN);
      ctx.clip();
      ctx.fillStyle = token("--map-land") || "#c9d1e0";
      ctx.fill(borders.path, "evenodd");
      ctx.strokeStyle = token("--map-border") || "#eef1f7";
      // A constant width on screen, whatever the zoom: thicker than a hairline at world scale so
      // neighbours read as separate, never so thick up close that a small country disappears.
      ctx.lineWidth = Math.min(1.1, 0.5 + this.view.k * 0.08) / s;
      ctx.lineJoin = "round";
      ctx.stroke(borders.path);
      ctx.restore();
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      if (this.view.k >= LABEL_ZOOM) this.drawLabels(borders.labels);
    }

    this.drawConnection();
    this.placePins();
  }

  /** Country names, where there is room for them: a name that does not fit its country is left out. */
  private drawLabels(labels: CountryLabel[]) {
    const ctx = this.ctx;
    const { width, height } = this.size;
    ctx.font = `600 ${this.view.k >= 5 ? 12 : 11}px ${token("--ui") || "sans-serif"}`;
    ctx.textAlign = "center";
    ctx.textBaseline = "middle";
    ctx.fillStyle = token("--map-label") || "#69738c";
    for (const label of labels) {
      const x = this.sx(label.x);
      const y = this.sy(label.y);
      if (x < -50 || x > width + 50 || y < -20 || y > height + 20) continue;
      if (ctx.measureText(label.name).width + 8 > label.width * this.scale) continue;
      ctx.fillText(label.name, x, y);
    }
  }

  /**
   * The route as arcs, one per leg, so the tunnel reads as a path rather than a dot — and with
   * arrows, so it reads *from* the user *to* the exit, through every hop in between.
   *
   * Each leg bows upward in proportion to its length, which keeps a short hop from looking like
   * a straight tick and a long one from leaving the map. A leg whose ends land within a few pixels
   * of each other — a server in the user's own city — is skipped rather than drawn as a smudge.
   *
   * Two arrows a leg: a chevron halfway, which says the direction wherever the eye lands, and a
   * head just short of the far end, clear of the pin it points at.
   */
  private drawConnection() {
    if (this.route.length < 2) return;

    this.ctx.strokeStyle = token("--map-line") || "#2e90fa";
    this.ctx.lineWidth = 1.6;
    this.ctx.setLineDash([5, 4]);
    this.ctx.globalAlpha = 0.9;

    for (let i = 1; i < this.route.length; i++) {
      const from = this.route[i - 1];
      const to = this.route[i];
      const x1 = this.px(from.lon);
      const y1 = this.py(from.lat);
      const x2 = this.px(to.lon);
      const y2 = this.py(to.lat);
      const length = Math.hypot(x2 - x1, y2 - y1);
      if (length < 6) continue;

      const cx = (x1 + x2) / 2;
      const cy = (y1 + y2) / 2 - length * 0.3;
      this.ctx.beginPath();
      this.ctx.moveTo(x1, y1);
      this.ctx.quadraticCurveTo(cx, cy, x2, y2);
      this.ctx.stroke();

      // Points on the curve and its direction there: B(t) and B'(t) of the quadratic.
      const at = (t: number) => ({
        x: (1 - t) ** 2 * x1 + 2 * (1 - t) * t * cx + t * t * x2,
        y: (1 - t) ** 2 * y1 + 2 * (1 - t) * t * cy + t * t * y2,
        angle: Math.atan2(2 * (1 - t) * (cy - y1) + 2 * t * (y2 - cy), 2 * (1 - t) * (cx - x1) + 2 * t * (x2 - cx)),
      });
      this.ctx.setLineDash([]);
      this.arrow(at(0.5), 5);
      // Stop short of the end by roughly a pin's radius, so the head is not hidden under the pin.
      if (length > 40) this.arrow(at(1 - Math.min(0.2, 14 / length)), 7);
      this.ctx.setLineDash([5, 4]);
    }

    this.ctx.setLineDash([]);
    this.ctx.globalAlpha = 1;
  }

  /** A filled arrowhead at a point on a leg, pointing along it. */
  private arrow(p: { x: number; y: number; angle: number }, size: number) {
    const ctx = this.ctx;
    ctx.save();
    ctx.translate(p.x, p.y);
    ctx.rotate(p.angle);
    ctx.beginPath();
    ctx.moveTo(size, 0);
    ctx.lineTo(-size * 0.8, -size * 0.75);
    ctx.lineTo(-size * 0.35, 0);
    ctx.lineTo(-size * 0.8, size * 0.75);
    ctx.closePath();
    ctx.fillStyle = token("--map-line") || "#2e90fa";
    ctx.fill();
    ctx.restore();
  }

  private placePins() {
    this.pinHost.replaceChildren();
    // While a route is drawn, the nodes on it are what the map is about; the rest step back.
    this.pinHost.classList.toggle("routing", this.route.length >= 2);
    const { width, height } = this.size;
    for (const pin of this.pins) {
      const x = this.px(pin.lon);
      const y = this.py(pin.lat);
      // Zoomed in, most pins are off screen; building them anyway would only give the keyboard
      // somewhere invisible to tab to.
      if (x < -20 || x > width + 20 || y < -20 || y > height + 20) continue;

      const pickable = pin.key !== undefined && this.onPick !== undefined;
      const node = document.createElement(pickable ? "button" : "span");
      node.className = `pin${pin.active ? " act" : ""}${pin.home ? " you" : ""}${pin.here ? " here" : ""}${pickable ? " pick" : ""}${pin.labelBelow ? " below" : ""}${pin.hop ? " hop" : ""}`;
      node.style.left = `${x.toFixed(1)}px`;
      node.style.top = `${y.toFixed(1)}px`;

      if (pickable && pin.key !== undefined) {
        const key = pin.key;
        (node as HTMLButtonElement).type = "button";
        if (pin.name) node.setAttribute("aria-label", pin.name);
        node.addEventListener("click", (e) => {
          // The document listener that closes an open card must not see this click as "outside".
          e.stopPropagation();
          this.onPick?.(key, { x, y });
        });
      }

      // A standing label (the exit, the user) wins; otherwise the name appears on hover, the way
      // a provider's map lets you read a dot before committing to it.
      const text = pin.label ?? (pickable ? pin.name : undefined);
      if (text) {
        const label = document.createElement("b");
        label.textContent = text;
        if (!pin.label) label.className = "hover";
        node.appendChild(label);
      }
      node.appendChild(document.createElement("i"));
      this.pinHost.appendChild(node);
    }
  }
}
