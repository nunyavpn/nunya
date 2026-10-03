/**
 * Finding, downloading and installing updates: the frontend half of `update.rs`.
 *
 * Checked a little after launch and then every `CHECK_EVERY_MS`, and again whenever the beta
 * switch changes, so a release reaches everyone within the hour. What a check finds is downloaded
 * at once, and the user is told only when it is ready: an update that needs another wait after
 * "Update" is pressed is one people put off. It is never installed by itself, because installing
 * restarts the app and would drop a tunnel the user is relying on; the notice stays up, on every
 * check, until they choose to.
 *
 * The state lives here rather than in `store.ts`: none of it should survive a restart, which is
 * the very thing an install does.
 */
import { invoke, listen } from "../bridge";
import { OS } from "../platform";
import { store } from "../store";
import { connection, disconnect } from "./tunnel";

/** Mirrors `update::Available`. */
export interface Available {
  version: string;
  prerelease: boolean;
  notes: string | null;
}

export interface Updates {
  checking: boolean;
  checkedAt: number | null;
  /** The newest version this copy may take, once a check has found one. */
  available: Available | null;
  /** Set while downloading it. */
  progress: { received: number; total: number | null } | null;
  /** Downloaded and verified; installing is the user's call. */
  ready: boolean;
  installing: boolean;
  error: string | null;
}

export const updates: Updates = {
  checking: false,
  checkedAt: null,
  available: null,
  progress: null,
  ready: false,
  installing: false,
  error: null,
};

/**
 * Whether the app updates itself. Not on Linux, where Nunya is a package (.deb, Arch) and its
 * package manager updates it; replacing `/usr/bin/Nunya` from here would need root and leave the
 * package manager's records wrong. `platform::IN_APP_UPDATES` is the Rust side of the same rule.
 */
export const IN_APP_UPDATES = OS !== "linux";

/** Hourly: every merge is a release, and a user should hear of one the same day. */
export const CHECK_EVERY_MS = 60 * 60 * 1000;
/** Not at the very first moment: launch is busy starting the core and placing the user. */
const FIRST_CHECK_MS = 30 * 1000;

/** What `main.ts` supplies so this module never has to reach back into it directly. */
export interface UpdateHooks {
  log(line: string): void;
  /** Repaint whatever shows update state. */
  refresh(): void;
}

let hooks: UpdateHooks;

/** Must be called once, after the data file has loaded, so the beta setting read is the saved one. */
export function initUpdates(next: UpdateHooks): void {
  hooks = next;
  if (!IN_APP_UPDATES) return;
  void listen<[number, number | null]>("update-progress", ([received, total]) => {
    if (!updates.progress) return;
    updates.progress = { received, total };
    hooks.refresh();
  });

  setTimeout(() => void checkForUpdates(), FIRST_CHECK_MS);
  setInterval(() => void checkForUpdates(), CHECK_EVERY_MS);

  // Turning betas on should find the newest beta now, not within the hour; turning them off
  // should drop a beta that was waiting.
  let beta = store.settings().betaUpdates;
  store.subscribe(() => {
    const now = store.settings().betaUpdates;
    if (now === beta) return;
    beta = now;
    void checkForUpdates();
  });
}

/** Looks for a newer release and downloads it. One at a time; a request during one is dropped. */
export async function checkForUpdates(): Promise<void> {
  if (!IN_APP_UPDATES || updates.checking || updates.installing) return;
  updates.checking = true;
  updates.error = null;
  hooks.refresh();

  try {
    const settings = store.settings();
    // Through the listener when there is one, like the block lists: on a network that blocks
    // GitHub, the tunnel is how the update arrives. In VPN mode the TUN carries it anyway.
    const proxyPort = connection === "on" && settings.mode === "proxy" ? settings.proxyPort : null;
    const found = await invoke<Available | null>("check_update", {
      beta: settings.betaUpdates,
      proxyPort,
    });
    updates.checkedAt = Date.now();

    if (!found) {
      updates.available = null;
      updates.ready = false;
      return;
    }
    const alreadyHere = updates.ready && updates.available?.version === found.version;
    updates.available = found;
    if (alreadyHere) return;

    hooks.log(`[ui] Nunya ${found.version} is available; downloading it`);
    updates.ready = false;
    updates.progress = { received: 0, total: null };
    hooks.refresh();
    await invoke("download_update");
    updates.ready = true;
    hooks.log(`[ui] Nunya ${found.version} is downloaded and verified`);
  } catch (e) {
    updates.error = e instanceof Error ? e.message : String(e);
    hooks.log(`[ui] update check failed: ${updates.error}`);
  } finally {
    updates.checking = false;
    updates.progress = null;
    hooks.refresh();
  }
}

/**
 * Installs the downloaded update, which restarts the app.
 *
 * Disconnects first, through the tunnel's own queue, so the system proxy is put back the usual
 * way rather than left pointing at a listener that is about to go away.
 */
export async function installUpdate(): Promise<void> {
  if (!updates.ready || updates.installing) return;
  updates.installing = true;
  updates.error = null;
  hooks.refresh();
  try {
    if (connection !== "off") {
      hooks.log("[ui] disconnecting to install the update");
      await disconnect();
    }
    hooks.log(`[ui] installing Nunya ${updates.available?.version}`);
    // Does not come back when it works: the app restarts into the new version.
    await invoke("install_update");
  } catch (e) {
    updates.installing = false;
    updates.error = e instanceof Error ? e.message : String(e);
    hooks.log(`[ui] update failed: ${updates.error}`);
    hooks.refresh();
  }
}
