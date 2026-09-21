/**
 * Country codes to map coordinates, names and flags.
 *
 * Coordinates are only what the map needs: a point to put a pin on. Deliberately not a geocoding
 * dataset — a server's city is whatever its name claims, and pinning the country is both honest
 * about that precision and enough for "roughly where does my traffic come out".
 *
 * Names and flags are not table-driven, because since the exit country is measured rather than
 * guessed a server can legitimately be anywhere, and a hand-kept list of two dozen countries
 * means a blank chip for the rest. Names come from `Intl.DisplayNames`, which the platform
 * already has, and flags from SVGs copied into `public/flags/` by `scripts/sync-flags.mjs`.
 */

interface Place {
  lon: number;
  lat: number;
  name: string;
  /** CSS background for the 26x19 flag chip. */
  flag: string;
}

/** Only the countries the map can place a pin for. */
interface Coords {
  lon: number;
  lat: number;
}

/**
 * The platform's own country names, so every code gets one.
 *
 * Built once: constructing it per row is measurably slow, and the list rebuilds on every store
 * change. Wrapped because a runtime without it should cost a name, not the whole list.
 */
const REGION_NAMES = (() => {
  try {
    return new Intl.DisplayNames(["en"], { type: "region" });
  } catch {
    return null;
  }
})();

/**
 * The chip background for a country code.
 *
 * The plain field stays underneath the image, so a code with no flag on disk — or one that is not
 * a country at all — degrades to the same grey chip an unknown server has always had, rather than
 * a broken image.
 */
function flagFor(code: string): string {
  const lower = code.trim().toLowerCase();
  if (!/^[a-z]{2}$/.test(lower)) return "var(--raised)";
  return `var(--raised) url("/flags/${lower}.svg") center / cover no-repeat`;
}

const COORDS: Record<string, Coords> = {
  DE: { lon: 10.4, lat: 51.2 },
  NL: { lon: 5.3, lat: 52.1 },
  FR: { lon: 2.2, lat: 46.6 },
  GB: { lon: -1.5, lat: 52.4 },
  FI: { lon: 25.7, lat: 61.9 },
  SE: { lon: 18.6, lat: 60.1 },
  NO: { lon: 8.5, lat: 60.5 },
  CH: { lon: 8.2, lat: 46.8 },
  AT: { lon: 14.6, lat: 47.5 },
  PL: { lon: 19.1, lat: 51.9 },
  IT: { lon: 12.6, lat: 41.9 },
  ES: { lon: -3.7, lat: 40.5 },
  RU: { lon: 37.6, lat: 55.8 },
  TR: { lon: 35.2, lat: 39.0 },
  AE: { lon: 54.4, lat: 24.0 },
  IR: { lon: 53.7, lat: 32.4 },
  US: { lon: -98.6, lat: 39.8 },
  CA: { lon: -106.3, lat: 56.1 },
  BR: { lon: -51.9, lat: -14.2 },
  JP: { lon: 138.3, lat: 36.2 },
  KR: { lon: 127.8, lat: 35.9 },
  SG: { lon: 103.8, lat: 1.35 },
  HK: { lon: 114.1, lat: 22.4 },
  TW: { lon: 121.0, lat: 23.7 },
  IN: { lon: 78.9, lat: 20.6 },
  AU: { lon: 133.8, lat: -25.3 },
  ZA: { lon: 22.9, lat: -30.6 },
};

/** Mid-Atlantic, which is where a pin goes when there is nothing better. */
const NOWHERE: Coords = { lon: 0, lat: 0 };

export function place(code: string): Place {
  const upper = code.trim().toUpperCase();
  const coords = COORDS[upper] ?? NOWHERE;
  const named = /^[A-Z]{2}$/.test(upper) ? REGION_NAMES?.of(upper) : undefined;

  return {
    ...coords,
    // `of` returns the input back when it knows no such region, which is not a name.
    name: named && named !== upper ? named : "Unknown",
    flag: flagFor(upper),
  };
}

/** Whether the map can place this one. A flag needs no coordinates, but a pin does. */
export function isKnown(code: string): boolean {
  return code.trim().toUpperCase() in COORDS;
}

/**
 * Guesses a country from a server's name.
 *
 * A guess, and labelled as one: it reads whatever the provider typed. Where a sweep has measured
 * the real exit, `Server.exit` holds it and the flag comes from there instead.
 */
export function guessCountry(name: string): string {
  const code = name.toUpperCase().match(/\b([A-Z]{2})\b/);
  if (code && isKnown(code[1])) return code[1];

  const lower = name.toLowerCase();
  // Only the countries with coordinates are matched by name: they are the ones worth a pin, and
  // scanning every region on earth for a substring would match "Chad" inside "Chadwick".
  for (const iso of Object.keys(COORDS)) {
    const named = REGION_NAMES?.of(iso);
    if (named && named !== iso && lower.includes(named.toLowerCase())) return iso;
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
