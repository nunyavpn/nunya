/**
 * Sorting a pasted line into a server, a subscription, or a rejection, and adding a subscription
 * group for one — extracted out of `main.ts`'s own "add servers" section per
 * `ENGINEERING_STANDARDS.md`.
 *
 * The sheet that collects the paste (`buildAddServers`, still in `main.ts`) stays there: it is
 * view code, built with `dom.ts`'s `h`/`render`, and belongs with the rest of the sheets rather
 * than here. This module is the pure decision of what a pasted line turned out to be, and the
 * store change that follows adding a subscription — both self-contained enough to need no
 * callback back into `main.ts` beyond logging and asking for a refresh.
 */
import { guessCity, guessCountry } from "../geo";
import { parseShareLink } from "../share";
import { MANUAL_GROUP_ID, store, type Group, type Server } from "../store";

/**
 * What a pasted line turned out to be.
 *
 * Subscriptions and share links arrive through the same box because that is how users receive
 * them — a provider hands you a page with both on it — and which kind is on the clipboard is a
 * question the app can answer for itself rather than ask.
 */
export type Pasted =
  | { kind: "server"; server: Omit<Server, "id" | "groupId"> }
  | { kind: "subscription"; url: string; name: string }
  | { kind: "rejected"; reason: string };

/**
 * A panel's "import to sing-box" or "import to Clash" link, which wraps the subscription address
 * in `url=`. Kept whole as the group's address; Rust unwraps it on every fetch (`subscription::
 * resolve`), so this only has to recognise one, not take it apart.
 */
export const IMPORT_LINK =
  /^(sing-box:\/\/import-remote-profile|clash:\/\/install-config|clashmeta:\/\/install-config)\b/i;

/**
 * Names a subscription from its URL fragment.
 *
 * Providers put the display name there — `#%F0%9F%92%A6%20BPB%20Normal` is "💦 BPB Normal" — and
 * it is the only name available until the fetch returns, because `profile-title` is a header not
 * every panel sends. Clash's import link carries it as `name=` instead. The host is a weak fallback
 * but an honest one — the host of the address inside, for an import link, whose own "host" is
 * `import-remote-profile` — and the first refresh replaces any of them with whatever the
 * subscription calls itself.
 */
export function subscriptionName(url: string): string {
  try {
    const parsed = new URL(url);
    // A malformed percent-escape throws, which is why this sits inside the try rather than beside
    // it: a name is never worth failing an import over.
    const fragment = decodeURIComponent(parsed.hash.replace(/^#/, "")).trim();
    if (fragment) return fragment;
    if (!IMPORT_LINK.test(url)) return parsed.hostname;
    const carried = parsed.searchParams.get("url") ?? "";
    return parsed.searchParams.get("name")?.trim() || new URL(carried).hostname;
  } catch {
    return url;
  }
}

/**
 * Sorts one pasted line into a server, a subscription, or a rejection carrying its reason.
 *
 * `http://` is checked here as well as in Rust. Leaving it to the backend would mean creating a
 * group and failing it a moment later, when the reason can be given while the user is still
 * looking at what they pasted.
 */
export function classify(line: string): Pasted {
  if (/^https:\/\//i.test(line) || IMPORT_LINK.test(line)) {
    return { kind: "subscription", url: line, name: subscriptionName(line) };
  }

  if (/^http:\/\//i.test(line)) {
    return {
      kind: "rejected",
      reason:
        "A plain HTTP subscription would expose every server credential to the network. " +
        "Ask your provider for an https:// link.",
    };
  }

  try {
    const profile = parseShareLink(line);
    return {
      kind: "server",
      server: {
        profile,
        country: guessCountry(profile.name),
        city: guessCity(profile.name),
        latency: null,
        testedAt: null,
      },
    };
  } catch (e) {
    return { kind: "rejected", reason: e instanceof Error ? e.message : String(e) };
  }
}

/** What `main.ts` supplies so this module never has to reach back into it directly. */
export interface AddServersHooks {
  log(line: string): void;
  refreshSubscription(group: Group): void;
}

let hooks: AddServersHooks;

/** Must be called once, during boot, before the Add servers sheet can be opened. */
export function initAddServers(next: AddServersHooks): void {
  hooks = next;
}

/** The hand-added group, which is where a pasted share link goes. */
export function manualGroupName(): string {
  return store.get().groups.find((g) => g.id === MANUAL_GROUP_ID)?.name ?? "Personal";
}

/**
 * Adds a subscription group and pulls it straight away.
 *
 * The group is created before the fetch rather than after it, so a slow or failing subscription
 * appears as a row that spins and then carries an error — exactly what a later refresh produces.
 * Fetching first would mean a dialog that hangs on a dead endpoint, and a second failure story to
 * write and keep in step with the first.
 *
 * Re-pasting a URL already on the list refreshes it instead of adding a second copy, because
 * pasting it again is the obvious way to ask for an update.
 */
export function addSubscription(url: string, name: string) {
  const existing = store.get().groups.find((g) => g.url === url);
  if (existing) {
    hooks.log(`[ui] ${existing.name} is already on the list; refreshing it instead`);
    hooks.refreshSubscription(existing);
    return;
  }

  const id = store.addSubscription(name, url);
  const group = store.get().groups.find((g) => g.id === id);
  if (group) hooks.refreshSubscription(group);
}
