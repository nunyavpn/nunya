/**
 * The app's data model and its store.
 *
 * Persistence sits behind `persist.ts`: a file the Rust side owns when running in the app, or
 * browser storage in the frontend preview. Nothing here knows which, and nothing in the views knows
 * persistence exists at all.
 */

import { backend, DebouncedWriter } from "./persist";
import type { Profile } from "./share";

/** Where a server came from. Hand-added servers live in their own group, which sorts first. */
export type GroupKind = "manual" | "subscription";

export interface Server {
  id: string;
  groupId: string;
  profile: Profile;
  /** Two-letter country code for the row chip. */
  country: string;
  city: string;
  /** Milliseconds, `-1` for unreachable, `null` for never tested. */
  latency: number | null;
  testedAt: number | null;
  /**
   * Why the last measurement failed.
   *
   * Kept because the list has room for a number and nothing else: both "never tested" and
   * "unreachable" render as a dash, and without the reason a user cannot tell a blocked server
   * from a dead one, or from a test endpoint their proxy happens not to be able to reach.
   */
  latencyError?: string;
  /** A refresh dropped this server, but it is kept because the tunnel is running on it. */
  retired?: boolean;
  /**
   * Where a sweep found this server actually exits, as a country code.
   *
   * Kept apart from `country` on purpose. `country` is read off the server's name and is what the
   * row is *labelled* with; this is measured, and is what the row is *flagged* with. They are
   * frequently different — a provider's "Germany" routinely exits in the United States — and
   * overwriting the label with the measurement would silently rename the user's servers, which
   * is not what a latency sweep is for.
   */
  exitCountry?: string;
  /**
   * The user named this one themselves, so the row shows that name instead of the country.
   *
   * Recorded rather than guessed. A share link's own name is usually the country and city again,
   * which is why the row prefers the country — but a name someone typed is the one thing that is
   * certainly not redundant, and there is no way to tell the two apart after the fact.
   */
  renamed?: boolean;
}

/** Traffic allowance, as reported by a subscription's `subscription-userinfo` header. */
export interface Quota {
  usedBytes: number;
  totalBytes: number;
  /** Unix ms when the allowance resets. */
  resetsAt: number | null;
}

export interface Group {
  id: string;
  kind: GroupKind;
  name: string;
  /** Only for subscriptions. */
  url: string | null;
  updatedAt: number | null;
  /** Set when the last refresh failed; the row shows it instead of a timestamp. */
  lastError: string | null;
  refreshing: boolean;
  quota: Quota | null;
  collapsed: boolean;
}

/** One entry in the bypass list: traffic that leaves on the physical link. */
export type BypassKind = "domain" | "address" | "range";

export interface BypassRule {
  id: string;
  kind: BypassKind;
  value: string;
}

/**
 * Everything the Advanced panel can change.
 *
 * Small on purpose. The Qt build persists 240 settings; almost all of them belong to transports and
 * protocols this client does not have. These are the ones that genuinely change how the tunnel
 * behaves, and every default here is one a user should never need to touch.
 */
/**
 * How traffic reaches the core.
 *
 * Not a preference between two equivalent things. `vpn` puts a TUN in front of the whole device,
 * so everything is carried whether or not it knows about the proxy. `proxy` opens a local
 * SOCKS/HTTP port, which covers only what is configured to use it — and needs no privilege, which
 * is why it works today on a machine where the TUN path does not.
 */
export type Mode = "vpn" | "proxy";

export interface Settings {
  mode: Mode;
  /** Proxy mode: the local port the mixed SOCKS/HTTP listener binds. */
  proxyPort: number;
  /** Proxy mode: bind every interface rather than loopback, so other machines can use it. */
  allowLan: boolean;
  /** "system" is faster; "gvisor" is the portable fallback for odd kernels. */
  stack: "system" | "gvisor";
  mtu: number;
  ipv4Cidr: string;
  /** Blocks traffic that tries to leave outside the tunnel. This is the kill switch. */
  strictRoute: boolean;
  ipv6: boolean;
  /** Resolver for names that are not bypassed. Queries go through the tunnel. */
  dns: string;
  logLevel: "trace" | "debug" | "info" | "warn" | "error";
}

export const DEFAULT_SETTINGS: Settings = {
  // Proxy, because it is the mode that works. A TUN needs privilege this build cannot obtain on
  // macOS without a signed packet tunnel extension, so defaulting to VPN mode would hand a new
  // user a Connect button that fails. Switching is one control in Advanced.
  mode: "proxy",
  // What the other clients in this family listen on, so an existing browser profile or shell
  // alias pointed at one of them keeps working.
  proxyPort: 2080,
  allowLan: false,
  stack: "system",
  mtu: 1500,
  // Matches the Qt build's default, so an existing user's routing assumptions still hold.
  ipv4Cidr: "172.19.0.1/24",
  strictRoute: true,
  ipv6: false,
  dns: "https://1.1.1.1/dns-query",
  logLevel: "info",
};

export interface AppData {
  groups: Group[];
  servers: Server[];
  bypass: BypassRule[];
  settings: Settings;
  /** The server the user last chose, so a restart reopens on it. */
  selectedServerId: string | null;
}

export const MANUAL_GROUP_ID = "manual";

function emptyData(): AppData {
  return {
    // The hand-added group always exists: it is where an imported link goes when the user has not
    // picked a subscription, and the UI shows it first.
    groups: [
      {
        id: MANUAL_GROUP_ID,
        kind: "manual",
        name: "Personal",
        url: null,
        updatedAt: null,
        lastError: null,
        refreshing: false,
        quota: null,
        collapsed: false,
      },
    ],
    servers: [],
    bypass: [],
    settings: { ...DEFAULT_SETTINGS },
    selectedServerId: null,
  };
}

export function newId(): string {
  return crypto.randomUUID();
}

/** Sorts hand-added servers first, then subscriptions by name. */
export function orderGroups(groups: Group[]): Group[] {
  return [...groups].sort((a, b) => {
    if (a.kind !== b.kind) return a.kind === "manual" ? -1 : 1;
    return a.name.localeCompare(b.name);
  });
}

type Listener = (data: AppData) => void;

/**
 * Holds the data and tells the views when it changes.
 *
 * Every mutation goes through `update`, which persists and notifies in one place — so a view can
 * never change state without the rest of the app hearing about it.
 */
class Store {
  private data: AppData = emptyData();
  private listeners = new Set<Listener>();
  private writer = new DebouncedWriter(backend);
  /** Set when saved data could not be read, so the UI can say so rather than look empty. */
  private loadError: string | null = null;

  /**
   * Reads saved data.
   *
   * A failure leaves the app usable with an empty list and records why. It deliberately does not
   * overwrite: if a file exists but could not be parsed, saving over it would destroy whatever is
   * recoverable.
   */
  async load(): Promise<void> {
    try {
      const raw = await backend.load();
      if (raw) {
        const parsed = JSON.parse(raw) as AppData;
        // A payload from an older version must not take the app down on launch.
        this.data = {
          ...emptyData(),
          ...parsed,
          groups: parsed.groups?.length ? parsed.groups : emptyData().groups,
          // Merged rather than replaced, so a payload written before a setting existed still gets
          // its default instead of `undefined` reaching the config builder.
          settings: { ...DEFAULT_SETTINGS, ...(parsed.settings ?? {}) },
        };
      }
      this.loadError = null;
    } catch (e) {
      this.loadError = String(e);
      console.warn("could not read saved data", e);
      this.data = emptyData();
    }
    this.notify();
  }

  /** Why the saved data could not be read, if it could not. */
  get storageError(): string | null {
    return this.loadError ?? this.writer.error;
  }

  get storageName(): string {
    return backend.name;
  }

  /** Writes any pending change immediately; used before the window closes. */
  flush(): Promise<void> {
    return this.writer.flush();
  }

  get(): AppData {
    return this.data;
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    listener(this.data);
    return () => this.listeners.delete(listener);
  }

  update(change: (data: AppData) => void) {
    change(this.data);
    this.persist();
    this.notify();
  }

  private persist() {
    // Never save over data that failed to load: the file on disk may still be recoverable.
    if (this.loadError) return;
    this.writer.queue(JSON.stringify(this.data));
  }

  private notify() {
    for (const listener of this.listeners) listener(this.data);
  }

  // ------------------------------------------------------------- queries

  group(id: string): Group | undefined {
    return this.data.groups.find((g) => g.id === id);
  }

  server(id: string | null): Server | undefined {
    if (!id) return undefined;
    return this.data.servers.find((s) => s.id === id);
  }

  serversIn(groupId: string): Server[] {
    return this.data.servers.filter((s) => s.groupId === groupId);
  }

  selected(): Server | undefined {
    return this.server(this.data.selectedServerId);
  }

  /** The lowest-latency reachable server, which is what Quick Connect picks. */
  fastest(): Server | undefined {
    return this.data.servers
      .filter((s) => s.latency !== null && s.latency >= 0)
      .sort((a, b) => (a.latency ?? 0) - (b.latency ?? 0))[0];
  }

  // ------------------------------------------------------------- mutations

  addServers(groupId: string, servers: Omit<Server, "id" | "groupId">[]) {
    this.update((data) => {
      for (const s of servers) {
        data.servers.push({ ...s, id: newId(), groupId });
      }
      // Selecting the first import saves a click on the most common first run.
      if (!data.selectedServerId && data.servers.length) {
        data.selectedServerId = data.servers[0].id;
      }
    });
  }

  /**
   * Replaces a subscription's servers with what a refresh returned.
   *
   * Two things this deliberately does not do. It does not drop the server the tunnel is currently
   * running on, even when the refresh no longer lists it — pulling that out mid-session would kill
   * a working connection to apply a list update. And it does not reset latency for servers that
   * survived, so a refresh does not throw away every test result.
   */
  replaceSubscriptionServers(
    groupId: string,
    incoming: Omit<Server, "id" | "groupId">[],
    protectedId: string | null,
  ): { added: number; removed: number; retired: number; kept: number } {
    // Identity is the endpoint plus credentials, not the display name: providers rename servers
    // constantly, and a rename should not read as "removed and re-added".
    const key = (s: { profile: Profile }) =>
      `${s.profile.server}:${s.profile.port}:${s.profile.uuid}`;

    let added = 0;
    let removed = 0;
    let retired = 0;
    let kept = 0;

    this.update((data) => {
      const existing = data.servers.filter((s) => s.groupId === groupId);
      const others = data.servers.filter((s) => s.groupId !== groupId);
      const byKey = new Map(existing.map((s) => [key(s), s]));
      const incomingKeys = new Set(incoming.map(key));

      const next: Server[] = incoming.map((candidate) => {
        const previous = byKey.get(key(candidate));
        if (!previous) {
          added += 1;
          return { ...candidate, id: newId(), groupId };
        }
        kept += 1;
        // Keep the id so the selection survives, and the last test result with it.
        return {
          ...candidate,
          id: previous.id,
          groupId,
          latency: previous.latency,
          testedAt: previous.testedAt,
          latencyError: previous.latencyError,
          // A measured country outranks the name the provider gave this refresh. Letting the
          // guess win here would undo the measurement every time the subscription updated,
          // which is often enough that the flag would never settle.
          // A measured exit survives a refresh; the label and city follow the provider, which
          // is what a refresh is for.
          exitCountry: previous.exitCountry,
          renamed: previous.renamed,
          retired: false,
        };
      });

      for (const old of existing) {
        if (incomingKeys.has(key(old))) continue;
        if (old.id === protectedId) {
          // Kept alive, but marked so the UI can say why it is still there.
          next.push({ ...old, retired: true });
          retired += 1;
        } else {
          removed += 1;
        }
      }

      data.servers = [...others, ...next];
      if (!data.servers.some((s) => s.id === data.selectedServerId)) {
        data.selectedServerId = data.servers[0]?.id ?? null;
      }
    });

    return { added, removed, retired, kept };
  }

  /**
   * Records where servers were found to actually exit.
   *
   * This overrides the guess taken from the server's name, and records that it did, so a later
   * subscription refresh does not quietly put the guess back.
   */
  applyCountries(updates: { id: string; country: string }[]) {
    const byId = new Map(updates.map((u) => [u.id, u.country]));
    this.update((data) => {
      for (const server of data.servers) {
        const country = byId.get(server.id);
        if (!country) continue;
        // The flag only. The name, the city and the label are the user's and the provider's;
        // this is the one thing the measurement is entitled to change.
        server.exitCountry = country;
      }
    });
  }

  /** Records latency results from a test sweep. */
  applyLatencies(updates: { id: string; latency: number; error?: string | null }[]) {
    const byId = new Map(updates.map((u) => [u.id, u]));
    this.update((data) => {
      const now = Date.now();
      for (const server of data.servers) {
        const update = byId.get(server.id);
        if (update === undefined) continue;
        const latency = update.latency;
        server.latencyError = update.error ?? undefined;
        server.latency = latency;
        server.testedAt = now;
      }
    });
  }

  /**
   * Replaces a server's profile after an edit.
   *
   * The latency is cleared because it was measured against the old address, and a stale number
   * beside a changed server is worse than no number: it is the one thing Quick Connect ranks on.
   * The country and city are re-derived by the caller, which is where the guessing lives.
   */
  updateServer(
    id: string,
    profile: Profile,
    place: { country: string; city: string; renamed: boolean },
  ) {
    this.update((data) => {
      const server = data.servers.find((s) => s.id === id);
      if (!server) return;
      server.profile = profile;
      server.country = place.country;
      server.city = place.city;
      server.renamed = place.renamed || undefined;
      server.latency = null;
      server.testedAt = null;
    });
  }

  removeServer(id: string) {
    this.update((data) => {
      data.servers = data.servers.filter((s) => s.id !== id);
      if (data.selectedServerId === id) {
        data.selectedServerId = data.servers[0]?.id ?? null;
      }
    });
  }

  select(id: string) {
    this.update((data) => {
      data.selectedServerId = id;
    });
  }

  toggleGroup(id: string) {
    this.update((data) => {
      const group = data.groups.find((g) => g.id === id);
      if (group) group.collapsed = !group.collapsed;
    });
  }

  addSubscription(name: string, url: string): string {
    const id = newId();
    this.update((data) => {
      data.groups.push({
        id,
        kind: "subscription",
        name,
        url,
        updatedAt: null,
        lastError: null,
        refreshing: false,
        quota: null,
        collapsed: false,
      });
    });
    return id;
  }

  removeGroup(id: string) {
    if (id === MANUAL_GROUP_ID) return; // the hand-added group is not removable
    this.update((data) => {
      data.groups = data.groups.filter((g) => g.id !== id);
      data.servers = data.servers.filter((s) => s.groupId !== id);
      if (!data.servers.some((s) => s.id === data.selectedServerId)) {
        data.selectedServerId = data.servers[0]?.id ?? null;
      }
    });
  }

  setRefreshing(id: string, refreshing: boolean) {
    this.update((data) => {
      const group = data.groups.find((g) => g.id === id);
      if (group) group.refreshing = refreshing;
    });
  }

  settings(): Settings {
    return this.data.settings;
  }

  updateSettings(change: Partial<Settings>) {
    this.update((data) => {
      data.settings = { ...data.settings, ...change };
    });
  }

  addBypass(kind: BypassKind, value: string) {
    this.update((data) => {
      // Re-adding an existing rule should be a no-op, not a duplicate row.
      if (data.bypass.some((r) => r.value === value)) return;
      data.bypass.push({ id: newId(), kind, value });
    });
  }

  removeBypass(id: string) {
    this.update((data) => {
      data.bypass = data.bypass.filter((r) => r.id !== id);
    });
  }
}

export const store = new Store();

/**
 * Classifies a bypass entry from what the user typed.
 *
 * One field for three kinds: a slash makes it a range, four dotted numbers an address, anything
 * else a domain. Returning `null` is what keeps the Add button disabled.
 */
export function classifyBypass(raw: string): BypassKind | null {
  const value = raw.trim();
  if (!value) return null;

  if (value.includes("/")) {
    const [addr, bits] = value.split("/");
    const prefix = Number(bits);
    if (!isIPv4(addr) || !Number.isInteger(prefix) || prefix < 0 || prefix > 32) return null;
    return "range";
  }

  if (isIPv4(value)) return "address";

  // A domain, optionally with the wildcard prefix people copy out of other clients.
  const bare = value.startsWith("*.") ? value.slice(2) : value;
  return /^(?!-)[a-z0-9-]+(\.[a-z0-9-]+)+$/i.test(bare) ? "domain" : null;
}

function isIPv4(value: string): boolean {
  const parts = value.split(".");
  if (parts.length !== 4) return false;
  return parts.every((p) => /^\d{1,3}$/.test(p) && Number(p) <= 255);
}
