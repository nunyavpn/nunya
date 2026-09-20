/**
 * The dot-matrix world map behind the main window.
 *
 * Drawn on canvas from coarse continent outlines rather than shipping a GeoJSON file: at this
 * resolution the map is a stylised backdrop that tells you roughly where your exit is, not a survey
 * map, and a few hundred coordinates cost nothing next to a topology dataset.
 */

/** Rough continent outlines, `[lon, lat, lon, lat, ...]`. Nothing here is survey-accurate. */
const LAND: number[][] = [
  // North America
  [-168, 65, -160, 71, -140, 70, -125, 70, -102, 69, -85, 70, -75, 68, -62, 60, -55, 50, -66, 45,
   -70, 42, -76, 35, -81, 25, -90, 29, -97, 26, -105, 20, -115, 30, -125, 40, -125, 48, -135, 58,
   -150, 60, -165, 55],
  // Greenland
  [-45, 60, -22, 70, -20, 82, -58, 82, -58, 70],
  // South America
  [-80, 8, -62, 10, -50, 0, -35, -5, -35, -22, -48, -33, -58, -40, -66, -55, -73, -52, -72, -40,
   -70, -20, -80, -5],
  // Eurasia
  [-10, 36, -9, 43, 0, 49, 5, 53, 8, 57, 5, 60, 12, 65, 20, 70, 30, 71, 45, 68, 60, 72, 75, 73,
   90, 75, 105, 77, 115, 74, 130, 72, 145, 70, 160, 69, 170, 66, 178, 65, 170, 60, 160, 58, 155, 50,
   140, 45, 130, 42, 126, 35, 122, 30, 118, 24, 108, 21, 105, 10, 100, 5, 95, 8, 90, 21, 80, 10,
   72, 20, 65, 25, 60, 22, 50, 28, 45, 12, 43, 12, 35, 28, 32, 31, 35, 36, 28, 36, 20, 40, 15, 38,
   12, 45, 8, 44, 3, 42, -5, 36],
  // Africa
  [-17, 15, -16, 22, -10, 30, 0, 34, 10, 37, 20, 32, 32, 31, 35, 22, 40, 15, 43, 11, 48, 5, 52, 0,
   48, -10, 40, -18, 35, -24, 32, -28, 25, -34, 18, -34, 12, -18, 9, -2, 5, 5, -5, 5, -12, 8],
  // Australia
  [114, -22, 113, -26, 115, -34, 125, -33, 135, -35, 140, -38, 147, -38, 150, -35, 153, -28,
   146, -19, 142, -11, 135, -12, 130, -11, 125, -14, 120, -20],
  // British Isles
  [-6, 50, -2, 50, 0, 53, -2, 58, -6, 58],
  // Japan
  [130, 32, 140, 35, 145, 43, 141, 45, 135, 34],
  // Madagascar
  [43, -13, 50, -15, 50, -25, 45, -25],
  // New Zealand
  [166, -46, 170, -46, 178, -38, 172, -34, 168, -40],
  // Indonesia / New Guinea
  [95, 5, 120, 0, 140, -2, 150, -8, 140, -9, 120, -9, 100, -5],
];

// Equirectangular, cropped top and bottom. Projecting the full -90..90 into the pane would stretch
// latitude about 2.5x and the continents stop being recognisable.
const LAT_TOP = 75;
const LAT_BOTTOM = -55;
const LAT_SPAN = LAT_TOP - LAT_BOTTOM;

/** Vertical space the status card covers; the map band centres in what is left above it. */
const CARD_RESERVE = 110;

export interface Pin {
  lon: number;
  lat: number;
  label?: string;
  active?: boolean;
  /** The device itself, drawn muted and unlabelled. */
  home?: boolean;
}

interface Frame {
  x: number;
  y: number;
  w: number;
  h: number;
}

function inPolygon(lon: number, lat: number, poly: number[]): boolean {
  let inside = false;
  for (let i = 0, j = poly.length - 2; i < poly.length; j = i, i += 2) {
    const xi = poly[i];
    const yi = poly[i + 1];
    const xj = poly[j];
    const yj = poly[j + 1];
    if (yi > lat !== yj > lat && lon < ((xj - xi) * (lat - yi)) / (yj - yi) + xi) {
      inside = !inside;
    }
  }
  return inside;
}

function onLand(lon: number, lat: number): boolean {
  return LAND.some((poly) => inPolygon(lon, lat, poly));
}

function token(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

/**
 * Draws the map into a canvas and positions HTML pins over it.
 *
 * Pins are DOM rather than canvas so they can carry labels, a CSS pulse and a hit target without
 * hand-rolling any of it.
 */
export class WorldMap {
  private ctx: CanvasRenderingContext2D;
  private frame: Frame = { x: 0, y: 0, w: 0, h: 0 };
  private pins: Pin[] = [];

  constructor(
    private canvas: HTMLCanvasElement,
    private pinHost: HTMLElement,
  ) {
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("canvas 2d context unavailable");
    this.ctx = ctx;

    if (typeof ResizeObserver !== "undefined") {
      new ResizeObserver(() => this.draw()).observe(canvas);
    } else {
      window.addEventListener("resize", () => this.draw());
    }

    // The dot colour is a theme token, so a theme change has to repaint.
    window.matchMedia?.("(prefers-color-scheme: dark)").addEventListener?.("change", () => this.draw());
  }

  setPins(pins: Pin[]) {
    this.pins = pins;
    this.draw();
  }

  private computeFrame(width: number, height: number): Frame {
    const w = width * 1.06; // a touch of bleed past the pane edges
    const h = w * (LAT_SPAN / 360);
    const free = Math.max(height - CARD_RESERVE, h);
    return { x: (width - w) / 2, y: (free - h) / 2, w, h };
  }

  private px(lon: number): number {
    return this.frame.x + ((lon + 180) / 360) * this.frame.w;
  }

  private py(lat: number): number {
    return this.frame.y + ((LAT_TOP - lat) / LAT_SPAN) * this.frame.h;
  }

  draw() {
    const box = this.canvas.getBoundingClientRect();
    if (!box.width || !box.height) return;

    const dpr = Math.min(window.devicePixelRatio || 1, 2);
    this.canvas.width = Math.round(box.width * dpr);
    this.canvas.height = Math.round(box.height * dpr);
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    this.ctx.clearRect(0, 0, box.width, box.height);

    this.frame = this.computeFrame(box.width, box.height);

    const perDegree = this.frame.w / 360;
    const step = Math.max(1.1, 3.4 / perDegree);
    const radius = Math.max(0.85, perDegree * 0.6);

    this.ctx.fillStyle = token("--map-dot") || "#c3ccdd";
    for (let lon = -180; lon <= 180; lon += step) {
      for (let lat = LAT_TOP; lat >= LAT_BOTTOM; lat -= step) {
        if (!onLand(lon, lat)) continue;
        this.ctx.beginPath();
        this.ctx.arc(this.px(lon), this.py(lat), radius, 0, Math.PI * 2);
        this.ctx.fill();
      }
    }

    this.drawConnection();
    this.placePins();
  }

  /** An arc from the device to the active exit, so the tunnel reads as a path rather than a dot. */
  private drawConnection() {
    const home = this.pins.find((p) => p.home);
    const active = this.pins.find((p) => p.active);
    if (!home || !active) return;

    const x1 = this.px(home.lon);
    const y1 = this.py(home.lat);
    const x2 = this.px(active.lon);
    const y2 = this.py(active.lat);
    const cx = (x1 + x2) / 2;
    const cy = (y1 + y2) / 2 - Math.abs(x2 - x1) * 0.3;

    this.ctx.strokeStyle = token("--map-line") || "#2e90fa";
    this.ctx.lineWidth = 1.6;
    this.ctx.setLineDash([5, 4]);
    this.ctx.globalAlpha = 0.9;
    this.ctx.beginPath();
    this.ctx.moveTo(x1, y1);
    this.ctx.quadraticCurveTo(cx, cy, x2, y2);
    this.ctx.stroke();
    this.ctx.setLineDash([]);
    this.ctx.globalAlpha = 1;
  }

  private placePins() {
    this.pinHost.replaceChildren();
    for (const pin of this.pins) {
      const node = document.createElement("span");
      node.className = `pin${pin.active ? " act" : ""}${pin.home ? " you" : ""}`;
      node.style.left = `${this.px(pin.lon).toFixed(1)}px`;
      node.style.top = `${this.py(pin.lat).toFixed(1)}px`;

      if (pin.label) {
        const label = document.createElement("b");
        label.textContent = pin.label;
        node.appendChild(label);
      }
      node.appendChild(document.createElement("i"));
      this.pinHost.appendChild(node);
    }
  }
}
