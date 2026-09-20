/**
 * Thin wrapper over the Tauri bridge.
 *
 * Outside a Tauri window — `vite dev` in an ordinary browser — `invoke` and `listen` reject,
 * because there is no backend to talk to. Rather than let the UI die on boot, this reports the
 * absence and lets the interface render in its "core not running" state.
 *
 * That is not a fallback for production; it is so the frontend can be worked on in a browser, with
 * devtools and instant reload, without building the Rust side first.
 */

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";

/** Tauri injects this global into every window it creates. */
export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export class NoBackendError extends Error {
  constructor(command: string) {
    super(`"${command}" is unavailable: the app is running outside Tauri`);
  }
}

export async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!inTauri) throw new NoBackendError(command);
  return tauriInvoke<T>(command, args);
}

export async function listen<T>(
  event: string,
  handler: (payload: T) => void,
): Promise<UnlistenFn | null> {
  if (!inTauri) return null;
  return tauriListen<T>(event, (e) => handler(e.payload));
}
