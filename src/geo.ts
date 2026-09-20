/**
 * Country codes to map coordinates, and to the flag colours the list shows.
 *
 * Only what the map needs: a point to put a pin on. Deliberately not a geocoding dataset — a
 * server's city is whatever its name claims, and pinning the country is both honest about that
 * precision and enough for "roughly where does my traffic come out".
 */

interface Place {
  lon: number;
  lat: number;
  name: string;
  /** CSS background for the 26x19 flag chip. Approximate by design, not heraldry. */
  flag: string;
}

const stripes = (...colors: string[]): string => {
  const step = 100 / colors.length;
  const stops = colors
    .map((c, i) => `${c} ${(i * step).toFixed(2)}% ${((i + 1) * step).toFixed(2)}%`)
    .join(",");
  return `linear-gradient(180deg,${stops})`;
};

const bars = (...colors: string[]): string => {
  const step = 100 / colors.length;
  const stops = colors
    .map((c, i) => `${c} ${(i * step).toFixed(2)}% ${((i + 1) * step).toFixed(2)}%`)
    .join(",");
  return `linear-gradient(90deg,${stops})`;
};

/** A centred disc on a plain field, for flags that would otherwise render as a blank rectangle. */
const disc = (mark: string, field: string): string =>
  `radial-gradient(circle at 50% 50%, ${mark} 0 26%, ${field} 26%)`;

const nordicCross = (field: string, cross: string): string =>
  `linear-gradient(90deg,transparent 0 26%,${cross} 26% 45%,transparent 45%),` +
  `linear-gradient(180deg,transparent 0 33%,${cross} 33% 62%,transparent 62%),${field}`;

const PLACES: Record<string, Place> = {
  DE: { lon: 10.4, lat: 51.2, name: "Germany", flag: stripes("#111", "#D7141A", "#FFCE00") },
  NL: { lon: 5.3, lat: 52.1, name: "Netherlands", flag: stripes("#AE1C28", "#fff", "#21468B") },
  FR: { lon: 2.2, lat: 46.6, name: "France", flag: bars("#002395", "#fff", "#ED2939") },
  GB: { lon: -1.5, lat: 52.4, name: "United Kingdom", flag: "#00247D" },
  FI: { lon: 25.7, lat: 61.9, name: "Finland", flag: nordicCross("#fff", "#003580") },
  SE: { lon: 18.6, lat: 60.1, name: "Sweden", flag: nordicCross("#006AA7", "#FECC00") },
  NO: { lon: 8.5, lat: 60.5, name: "Norway", flag: nordicCross("#BA0C2F", "#fff") },
  CH: { lon: 8.2, lat: 46.8, name: "Switzerland", flag: "#D52B1E" },
  AT: { lon: 14.6, lat: 47.5, name: "Austria", flag: stripes("#ED2939", "#fff", "#ED2939") },
  PL: { lon: 19.1, lat: 51.9, name: "Poland", flag: stripes("#fff", "#DC143C") },
  IT: { lon: 12.6, lat: 41.9, name: "Italy", flag: bars("#009246", "#fff", "#CE2B37") },
  ES: { lon: -3.7, lat: 40.5, name: "Spain", flag: stripes("#AA151B", "#F1BF00", "#AA151B") },
  RU: { lon: 37.6, lat: 55.8, name: "Russia", flag: stripes("#fff", "#0039A6", "#D52B1E") },
  TR: { lon: 35.2, lat: 39.0, name: "Türkiye", flag: "#E30A17" },
  AE: { lon: 54.4, lat: 24.0, name: "United Arab Emirates", flag: stripes("#00732F", "#fff", "#000") },
  IR: { lon: 53.7, lat: 32.4, name: "Iran", flag: stripes("#239F40", "#fff", "#DA0000") },
  US: { lon: -98.6, lat: 39.8, name: "United States", flag: stripes("#B22234", "#fff", "#B22234", "#fff", "#3C3B6E") },
  CA: { lon: -106.3, lat: 56.1, name: "Canada", flag: bars("#D80621", "#fff", "#D80621") },
  BR: { lon: -51.9, lat: -14.2, name: "Brazil", flag: "#009C3B" },
  JP: { lon: 138.3, lat: 36.2, name: "Japan", flag: disc("#BC002D", "#fff") },
  KR: { lon: 127.8, lat: 35.9, name: "South Korea", flag: disc("#CD2E3A", "#fff") },
  SG: { lon: 103.8, lat: 1.35, name: "Singapore", flag: stripes("#ED2939", "#fff") },
  HK: { lon: 114.1, lat: 22.4, name: "Hong Kong", flag: "#DE2910" },
  TW: { lon: 121.0, lat: 23.7, name: "Taiwan", flag: "#FE0000" },
  IN: { lon: 78.9, lat: 20.6, name: "India", flag: stripes("#FF9933", "#fff", "#138808") },
  AU: { lon: 133.8, lat: -25.3, name: "Australia", flag: "#00247D" },
  ZA: { lon: 22.9, lat: -30.6, name: "South Africa", flag: stripes("#007A4D", "#fff", "#DE3831") },
};

const UNKNOWN: Place = { lon: 0, lat: 0, name: "Unknown", flag: "var(--raised)" };

export function place(code: string): Place {
  return PLACES[code.toUpperCase()] ?? UNKNOWN;
}

export function isKnown(code: string): boolean {
  return code.toUpperCase() in PLACES;
}

/**
 * Best-effort country code from a server name.
 *
 * Share links carry names like `DE-4 Frankfurt` or `🇳🇱 Amsterdam 02`, so this looks for a standalone
 * two-letter code first, then a country or city name. It is a guess, and a wrong guess only moves a
 * pin — it never changes what the tunnel does.
 */
export function guessCountry(name: string): string {
  const code = name.toUpperCase().match(/\b([A-Z]{2})\b/);
  if (code && isKnown(code[1])) return code[1];

  const lower = name.toLowerCase();
  for (const [iso, p] of Object.entries(PLACES)) {
    if (lower.includes(p.name.toLowerCase())) return iso;
  }
  for (const [city, iso] of Object.entries(CITIES)) {
    if (lower.includes(city)) return iso;
  }
  return "";
}

/** Cities that appear in server names far more often than their country does. */
const CITIES: Record<string, string> = {
  frankfurt: "DE",
  berlin: "DE",
  amsterdam: "NL",
  paris: "FR",
  london: "GB",
  helsinki: "FI",
  stockholm: "SE",
  oslo: "NO",
  zurich: "CH",
  vienna: "AT",
  warsaw: "PL",
  milan: "IT",
  madrid: "ES",
  moscow: "RU",
  istanbul: "TR",
  dubai: "AE",
  tehran: "IR",
  "new york": "US",
  "los angeles": "US",
  dallas: "US",
  seattle: "US",
  miami: "US",
  toronto: "CA",
  tokyo: "JP",
  osaka: "JP",
  seoul: "KR",
  singapore: "SG",
  "hong kong": "HK",
  taipei: "TW",
  mumbai: "IN",
  sydney: "AU",
};

/** Extracts a city from a server name, for the row's second line. */
export function guessCity(name: string): string {
  const lower = name.toLowerCase();
  for (const city of Object.keys(CITIES)) {
    if (lower.includes(city)) {
      return city.replace(/\b\w/g, (c) => c.toUpperCase());
    }
  }
  return "";
}
