/**
 * Which config a refreshed one is.
 *
 * Every config has an id of its own — a random UUID from `newId` — and everything attached to it
 * hangs off that id: the selection, test results, both locations, the usage history. A
 * subscription refresh hands back a fresh list with no ids at all, so each incoming config has to
 * inherit the id of the config it *is*. Get that wrong and a server's history either vanishes on
 * every update or, worse, is shared between two servers.
 *
 * Identity is the protocol, the endpoint and the credential. Not the name, which providers change
 * constantly and a rename should not read as "removed and re-added"; and not the WebSocket path or
 * the SNI's letter case, which BPB randomises on every fetch.
 *
 * Kept free of the store so it runs under `node --test`; the type import is erased.
 */

import type { Profile } from "./share";

/**
 * The key two configs share when they are the same server.
 *
 * The credential is whichever the protocol has: a UUID for VLESS and VMess, a password for Trojan,
 * the private key for WireGuard. Leaving it out, as an earlier version did for everything but the
 * UUID, made every Trojan and WireGuard config on one host and port the same config.
 */
export function configKey(profile: Profile): string {
  const credential = profile.uuid || profile.password || profile.wireguard?.privateKey || "";
  return [profile.protocol, profile.server.toLowerCase(), profile.port, credential].join("|");
}

/**
 * Pairs each incoming config with the existing one it is, each existing config at most once.
 *
 * One-to-one is the point. A provider that lists a server twice would otherwise have both entries
 * take the same id — two rows the app believes are one server, whose selection, tests and usage
 * land on whichever it finds first.
 */
export function matchExisting<T extends { profile: Profile }>(
  existing: T[],
  incoming: { profile: Profile }[],
): (T | undefined)[] {
  const unclaimed = new Map<string, T[]>();
  for (const server of existing) {
    const key = configKey(server.profile);
    const same = unclaimed.get(key);
    if (same) same.push(server);
    else unclaimed.set(key, [server]);
  }
  return incoming.map((candidate) => unclaimed.get(configKey(candidate.profile))?.shift());
}
