/**
 * Thin wrapper over the Tauri bridge.
 *
 * Outside a Tauri window — `vite dev` in an ordinary browser — `invoke` and `listen` reject,
 * because there is no backend to talk to. Rather than let the UI die on boot, this reports the
 * absence and lets the interface render in its "core not running" state.
 *
 * That is not a fallback for production; it is so the frontend can be worked on in a browser, with
 * devtools and instant reload, without building the Rust side first.
 *
 * With `VITE_MOCK=1` the browser gets a simulated core instead (`mockcore.ts`), so it can connect;
 * in any other build `MOCK_CORE` is the literal `false` and that path is dropped.
 */

import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { emitTo as tauriEmitTo, listen as tauriListen, type UnlistenFn } from "@tauri-apps/api/event";

import { MOCK_CORE, mockInvoke, mockListen } from "./mockcore";

/** Tauri injects this global into every window it creates. */
export const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

/** Whether anything answers `invoke`: the Rust side, or the browser preview's simulated core. */
export const hasBackend = inTauri || MOCK_CORE;

export class NoBackendError extends Error {
  constructor(command: string) {
    super(`"${command}" is unavailable: the app is running outside Tauri`);
  }
}

export async function invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  if (!inTauri && MOCK_CORE) return mockInvoke<T>(command, args);
  if (!inTauri) throw new NoBackendError(command);
  return tauriInvoke<T>(command, args);
}

export async function listen<T>(
  event: string,
  handler: (payload: T) => void,
): Promise<UnlistenFn | null> {
  if (!inTauri && MOCK_CORE) return mockListen(event, (payload) => handler(payload as T));
  if (!inTauri) return null;
  return tauriListen<T>(event, (e) => handler(e.payload));
}

/**
 * Sends an event to one webview; the main window and the menu-bar popover talk to each other this
 * way. Outside Tauri there is no other webview, so it goes nowhere.
 */
export async function emitTo(target: string, event: string, payload: unknown): Promise<void> {
  if (!inTauri) return;
  await tauriEmitTo(target, event, payload);
}
