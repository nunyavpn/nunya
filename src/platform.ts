/**
 * Which system the app runs on, as the Rust side reports it.
 *
 * `platform::webview_plugin` sets `window.__NUNYA_OS__` before any of this script runs, so it can be
 * read here at import, where the constants keyed on it live (the tray icon's size, the paste
 * shortcut's modifier). It replaces `navigator.platform`, which is the webview's guess — deprecated,
 * and free to say something else than the binary that is actually running.
 *
 * Only the browser preview (`npm run dev`) has nobody to ask, so there, and only there, the guess
 * stands in: a Mac developer still sees ⌘.
 */

export type Os = "macos" | "linux" | "windows" | "android" | "ios";

declare global {
  interface Window {
    __NUNYA_OS__?: Os;
  }
}

function guess(): Os {
  const p = typeof navigator === "undefined" ? "" : navigator.platform;
  const agent = typeof navigator === "undefined" ? "" : navigator.userAgent;
  if (/Android/.test(agent)) return "android";
  if (/iPhone|iPad/.test(p)) return "ios";
  if (/Mac/.test(p)) return "macos";
  if (/Win/.test(p)) return "windows";
  return "linux";
}

export const OS: Os = (typeof window !== "undefined" && window.__NUNYA_OS__) || guess();

/** A phone: no keyboard shortcuts to mention, no tray, and an app store or the system's installer
 *  for updates. Its layout is the stylesheet's (`@media (max-width: 640px)`), not this. */
export const PHONE = OS === "android" || OS === "ios";
