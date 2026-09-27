/**
 * The update notice across the top of the map pane.
 *
 * The map pane is on screen whichever panel is open (panels replace the list, not the map), so a
 * notice here is seen on every screen without a modal in the way of whatever the user came to do.
 * It appears only once the update is downloaded and verified: "an update exists" with nothing to
 * press yet would be a notice to come back later. It has no close button, because the point is
 * that nobody misses a release; installing is the only way it goes.
 */
import { h, render } from "../dom";
import type { Updates } from "../features/updates";

export function renderUpdateBar(
  host: HTMLElement,
  updates: Updates,
  connected: boolean,
  onInstall: () => void,
) {
  const found = updates.available;
  if (!found || !(updates.ready || updates.installing)) {
    host.hidden = true;
    render(host);
    return;
  }
  host.hidden = false;
  const name = `Nunya ${found.version}${found.prerelease ? " beta" : ""}`;
  render(
    host,
    h(
      "span",
      { class: "t" },
      updates.installing ? `Installing ${name}…` : `${name} is ready to install.`,
      h(
        "span",
        { class: "d" },
        // An install failure is shown where it happened, not only in the log.
        updates.error ?? (connected ? "Updating disconnects, then restarts Nunya." : "Nunya restarts to finish."),
      ),
    ),
    h(
      "button",
      { class: "btn brand", disabled: updates.installing, onclick: onInstall },
      updates.installing ? "Restarting…" : "Restart to update",
    ),
  );
}

/** One line on where updating stands, for the settings group. */
export function updateLine(updates: Updates, now: number): string {
  const found = updates.available;
  if (updates.installing && found) return `installing ${found.version}…`;
  if (updates.progress && found) {
    const { received, total } = updates.progress;
    const mb = (n: number) => (n / 1_000_000).toFixed(1);
    return total
      ? `downloading ${found.version} · ${mb(received)} of ${mb(total)} MB`
      : `downloading ${found.version}…`;
  }
  if (updates.checking) return "checking…";
  if (updates.error) return `last check failed: ${updates.error}`;
  if (updates.ready && found) return `${found.version} is ready to install`;
  if (updates.checkedAt === null) return "not checked yet";
  const minutes = Math.round((now - updates.checkedAt) / 60_000);
  return `up to date · checked ${minutes < 1 ? "just now" : `${minutes} min ago`}`;
}
