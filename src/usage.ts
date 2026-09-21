/**
 * How much traffic each config has carried, by day.
 *
 * The source is the tunnel's own byte counters, which the status card already polls for its rate:
 * cumulative since the tunnel came up, so what one poll adds is the difference from the last. Those
 * differences are filed under the config the tunnel was running on and the local day, and kept on
 * the config itself — so the history lives exactly as long as the config does, travels with it
 * across a subscription refresh (which keeps its id), and goes when it is deleted, with no second
 * table to keep in step.
 *
 * Days rather than finer buckets: the questions this answers are "how much this month" and "which
 * config do I actually use", and a day per config is a few dozen bytes however long the app runs.
 *
 * Nothing here imports anything, so it runs under `node --test` without the app around it.
 */

/** Bytes up and down. */
export interface Bytes {
  up: number;
  down: number;
}

/** A config's history: local day (`2026-09-21`) to what it carried that day. */
export type Usage = Record<string, Bytes>;

/** One day of a chart. */
export interface DayBytes extends Bytes {
  day: string;
}

/** The tunnel's cumulative counters, as `query_stats` reports them. */
export interface Counters {
  uplink: number;
  downlink: number;
}

/**
 * The local calendar day of a timestamp.
 *
 * Local, not UTC: "today" in a chart should be the user's today, and a day that turned over at
 * 03:30 local time would split one evening across two bars.
 */
export function dayKey(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/**
 * What moved between two readings of the counters.
 *
 * The counters only ever grow while one core runs. A reading lower than the last means the core
 * started again underneath us, and everything it reports now is new since then — so that reading
 * is the whole difference, rather than a negative number that would subtract from a day.
 */
export function advance(previous: Counters, current: Counters): Bytes {
  const step = (before: number, now: number) => (now >= before ? now - before : Math.max(0, now));
  return { up: step(previous.uplink, current.uplink), down: step(previous.downlink, current.downlink) };
}

/** Adds traffic to a history under the day `at` falls on. Returns the history, created if needed. */
export function addUsage(usage: Usage | undefined, at: number, bytes: Bytes): Usage {
  const history = usage ?? {};
  if (bytes.up <= 0 && bytes.down <= 0) return history;
  const day = dayKey(at);
  const today = history[day] ?? { up: 0, down: 0 };
  history[day] = { up: today.up + Math.max(0, bytes.up), down: today.down + Math.max(0, bytes.down) };
  return history;
}

/** Everything in one or more histories, optionally only from `sinceDay` on. */
export function total(histories: (Usage | undefined)[], sinceDay?: string): Bytes {
  const sum: Bytes = { up: 0, down: 0 };
  for (const history of histories) {
    for (const [day, bytes] of Object.entries(history ?? {})) {
      if (sinceDay && day < sinceDay) continue;
      sum.up += bytes.up;
      sum.down += bytes.down;
    }
  }
  return sum;
}

/** The earliest day any of the histories has, or `null` for none. */
export function firstDay(histories: (Usage | undefined)[]): string | null {
  let first: string | null = null;
  for (const history of histories) {
    for (const day of Object.keys(history ?? {})) {
      if (first === null || day < first) first = day;
    }
  }
  return first;
}

/**
 * The day `back` calendar days before the day `now` falls on.
 *
 * Stepping the calendar date rather than subtracting 24-hour strides, which land on the wrong day
 * either side of a daylight-saving change.
 */
export function daysAgo(back: number, now: number): string {
  const today = new Date(now);
  return dayKey(new Date(today.getFullYear(), today.getMonth(), today.getDate() - back, 12).getTime());
}

/**
 * The last `days` days up to and including today, oldest first, summed across the histories.
 *
 * Every day is present, zero when nothing moved: a chart that skipped quiet days would put last
 * Tuesday next to today and read as a busy week.
 */
export function lastDays(histories: (Usage | undefined)[], days: number, now: number): DayBytes[] {
  const series: DayBytes[] = [];
  for (let back = days - 1; back >= 0; back--) {
    const day = daysAgo(back, now);
    const entry: DayBytes = { day, up: 0, down: 0 };
    for (const history of histories) {
      entry.up += history?.[day]?.up ?? 0;
      entry.down += history?.[day]?.down ?? 0;
    }
    series.push(entry);
  }
  return series;
}

/**
 * A round number at or above `value`, for the top of a chart's scale: 1, 2 or 5 times a power of
 * ten in whichever 1024-based unit the value is in, so gridlines land on sizes that read cleanly
 * ("500 MB", "2 GB"). Rounding up to a thousand of a unit becomes one of the next — "1 GB", not
 * "1000 MB".
 */
export function niceCeiling(value: number): number {
  if (value <= 0) return 1024 * 1024;
  let unit = 1;
  while (value >= unit * 1024 && unit < 1024 ** 4) unit *= 1024;
  const scaled = value / unit;
  const magnitude = 10 ** Math.floor(Math.log10(scaled));
  const step = [1, 2, 5, 10].find((s) => s * magnitude >= scaled) ?? 10;
  const round = step * magnitude;
  return round >= 1000 && unit < 1024 ** 4 ? unit * 1024 : round * unit;
}
