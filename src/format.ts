/** Formatting for the numbers the UI shows. All of them change in place, so they use tabular figures. */

const RATE_UNITS = ["B/s", "KB/s", "MB/s", "GB/s"] as const;
const SIZE_UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

function scale(value: number, units: readonly string[]): [string, string] {
  let v = Math.max(0, value);
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  // Keep the digit count roughly constant so the column does not jitter each second.
  const text = i === 0 || v >= 100 ? Math.round(v).toString() : v.toFixed(v >= 10 ? 1 : 2);
  return [text, units[i]];
}

/** Throughput, as a value and its unit, so the unit can be styled separately. */
export function rate(bytesPerSecond: number): [string, string] {
  return scale(bytesPerSecond, RATE_UNITS);
}

export function size(bytes: number): string {
  const [value, unit] = scale(bytes, SIZE_UNITS);
  return `${value} ${unit}`;
}

/** hh:mm:ss since a timestamp. */
export function elapsed(sinceMs: number): string {
  const total = Math.max(0, Math.floor((Date.now() - sinceMs) / 1000));
  const pad = (n: number) => n.toString().padStart(2, "0");
  return [Math.floor(total / 3600), Math.floor((total % 3600) / 60), total % 60].map(pad).join(":");
}

/** "4 minutes ago". Coarse on purpose: a subscription's exact refresh second is never interesting. */
export function ago(timestampMs: number | null): string {
  if (!timestampMs) return "never";

  const seconds = Math.max(0, Math.floor((Date.now() - timestampMs) / 1000));
  if (seconds < 60) return "just now";

  const steps: [number, string][] = [
    [60, "minute"],
    [60, "hour"],
    [24, "day"],
    [7, "week"],
  ];

  let value = seconds;
  let unit = "second";
  for (const [factor, name] of steps) {
    if (value < factor) break;
    value = Math.floor(value / factor);
    unit = name;
  }
  return `${value} ${unit}${value === 1 ? "" : "s"} ago`;
}

/** Latency for the list. `null` is an untested server, `-1` one that did not answer. */
export function latency(ms: number | null): { text: string; grade: "good" | "mid" | "bad" | "none" } {
  if (ms === null) return { text: "—", grade: "none" };
  if (ms < 0) return { text: "—", grade: "bad" };
  if (ms < 100) return { text: String(ms), grade: "good" };
  if (ms < 300) return { text: String(ms), grade: "mid" };
  return { text: String(ms), grade: "bad" };
}

/** Signal bars, derived from the same thresholds as the latency colour. */
export function bars(ms: number | null): 0 | 1 | 2 | 3 {
  if (ms === null || ms < 0) return 1;
  if (ms < 100) return 3;
  if (ms < 300) return 2;
  return 1;
}
