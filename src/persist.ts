/**
 * Where the app's data is kept.
 *
 * Three backends behind one interface. In the app it is a file the Rust side owns, written with
 * owner-only permissions and replaced atomically, because the payload contains every server
 * credential the user has. In a browser — the frontend preview — there is no backend, so
 * `localStorage` stands in. Under `VITE_MOCK=1` a fixture stands in for both.
 *
 * The store does not know which is in use.
 */

import { inTauri, invoke } from "./bridge";
import { MOCK_ENABLED, mockData } from "./mock";

export interface Backend {
  load(): Promise<string | null>;
  save(json: string): Promise<void>;
  /** Shown in Diagnostics, so it is clear where the data actually lives. */
  readonly name: string;
}

const LOCAL_KEY = "nunya.data.v1";

const browserBackend: Backend = {
  name: "browser storage",
  async load() {
    try {
      return localStorage.getItem(LOCAL_KEY);
    } catch {
      // Private windows and blocked site data both throw; an empty start is the right fallback.
      return null;
    }
  },
  async save(json) {
    try {
      localStorage.setItem(LOCAL_KEY, json);
    } catch {
      // A full or blocked store must not stop the app working for this session.
    }
  },
};

const fileBackend: Backend = {
  name: "data file",
  load: () => invoke<string | null>("load_data"),
  save: (json) => invoke<void>("save_data", { json }),
};

/**
 * Serves the fixture and throws every write away.
 *
 * Discarding saves is the whole point, not a shortcut. The fixture is meant to be clicked through —
 * selecting servers, collapsing groups, adding bypass rules — and each of those is a store mutation
 * that would otherwise be written straight over the real data file. The session stays live in
 * memory; nothing survives the window closing.
 */
const mockBackend: Backend = {
  name: "mock fixture (changes are not saved)",
  async load() {
    return JSON.stringify(mockData());
  },
  async save() {
    // Deliberately nothing.
  },
};

export const backend: Backend = MOCK_ENABLED
  ? mockBackend
  : inTauri
    ? fileBackend
    : browserBackend;

/**
 * Wraps a backend so bursts of changes become one write.
 *
 * Every mutation notifies the store, and dragging a toggle or typing in a field produces a run of
 * them. Writing the file each time would mean a rename per keystroke for data nobody has finished
 * editing.
 */
export class DebouncedWriter {
  private timer: ReturnType<typeof setTimeout> | null = null;
  private pending: string | null = null;
  private inFlight = false;
  private lastError: string | null = null;

  constructor(
    private target: Backend,
    private delayMs = 400,
  ) {
    // A quit mid-debounce would otherwise drop the last change.
    if (typeof window !== "undefined") {
      window.addEventListener("beforeunload", () => void this.flush());
    }
  }

  get error(): string | null {
    return this.lastError;
  }

  queue(json: string) {
    this.pending = json;
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(() => void this.flush(), this.delayMs);
  }

  /** Writes whatever is pending now. */
  async flush(): Promise<void> {
    if (this.timer) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    // A second flush while one is running would race; the newer payload is picked up by the write
    // that is already in progress, since `pending` is read after the await.
    if (this.inFlight || this.pending === null) return;

    this.inFlight = true;
    try {
      while (this.pending !== null) {
        const json = this.pending;
        this.pending = null;
        await this.target.save(json);
      }
      this.lastError = null;
    } catch (e) {
      this.lastError = String(e);
      console.warn("could not save data", e);
    } finally {
      this.inFlight = false;
    }
  }
}
