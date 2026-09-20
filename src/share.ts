/**
 * Parses share links into the profile shape the Rust side expects.
 *
 * Share links are written by panels and other clients, not by a spec, so this is forgiving about
 * what it accepts and strict about what it reports: a link it cannot fully understand is rejected
 * with a reason rather than silently turned into a profile that fails later at connect time.
 *
 * Two protocols, because they are the two that share a shape — a UUID, a TLS layer and a V2Ray
 * transport — and so share almost all of this file. Trojan, Shadowsocks, Hysteria2 and TUIC are
 * different outbound types on the core side and belong in their own parsers beside this one.
 *
 * ## The transport is the part that goes wrong
 *
 * A link carries its transport in `type=` (VLESS) or `net=` (VMess), and the names are not the ones
 * sing-box uses: `h2` is `http`, `raw` is plain TCP, and a TCP link with `headerType=http` is not
 * TCP at all — it is the HTTP transport wearing TCP's name. Getting this wrong does not fail
 * loudly; it produces a config the core accepts and a tunnel that never passes traffic. So the
 * mapping is explicit, and anything not in it is rejected by name.
 */

export interface Reality {
  publicKey: string;
  shortId: string;
}

export interface TlsOptions {
  enabled: boolean;
  sni: string;
  insecure: boolean;
  alpn: string[];
  fingerprint: string;
  reality: Reality | null;
}

export type Protocol = "vless" | "vmess" | "trojan" | "wireguard";

/** The transports sing-box implements. Anything else is rejected rather than approximated. */
export type TransportKind = "tcp" | "ws" | "grpc" | "http" | "httpupgrade" | "quic";

export interface Transport {
  kind: TransportKind;
  /** ws, http, httpupgrade. */
  path: string;
  /** The Host header (ws, httpupgrade) or the :authority (http). */
  host: string;
  /** grpc only. */
  serviceName: string;
  /** http only; empty means the core's default. */
  method: string;
  /**
   * WebSocket early data, in bytes.
   *
   * Written into the path as `?ed=2048` by every panel that supports it, rather than as a field of
   * its own. Left in the path it is a literal part of the URL and the server does not match it.
   */
  maxEarlyData: number;
  earlyDataHeader: string;
}

/**
 * What a WireGuard peer needs and nothing else does.
 *
 * Mirrors `WireguardOptions` in `config.rs`. Kept in its own object rather than flattened into
 * `Profile`, because none of it means anything to VLESS or VMess.
 */
export interface WireguardOptions {
  privateKey: string;
  peerPublicKey: string;
  /** The interface's own addresses, as CIDRs. WireGuard has no DHCP; the peer assigns these. */
  localAddress: string[];
  /** Cloudflare WARP's client identifier: three bytes, or empty for an ordinary peer. */
  reserved: number[];
  mtu: number;
  /** Seconds between keepalives, or 0 to leave it to the core. */
  keepalive: number;
}

export interface Profile {
  protocol: Protocol;
  name: string;
  server: string;
  port: number;
  uuid: string;
  /** VLESS only: `xtls-rprx-vision` or empty. */
  flow: string;
  /** VMess only: the cipher, `auto` unless the link says otherwise. */
  security: string;
  /** VMess only. Non-zero selects the pre-AEAD scheme, which modern servers do not use. */
  alterId: number;
  /** Trojan only: the credential, which sits where VLESS and VMess put a UUID. */
  password: string;
  tls: TlsOptions;
  transport: Transport;
  /** WireGuard only. */
  wireguard: WireguardOptions | null;
}

export class ParseError extends Error {}

/**
 * The protocols this build runs, written out for a rejection message.
 *
 * Derived rather than spelled, because the sentence was wrong within a day of Trojan and
 * WireGuard being added and nothing would have caught it.
 */
function supportedNames(): string {
  const names = SUPPORTED.map((p) => PROTOCOL_LABELS[p]);
  return names.length > 1
    ? `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`
    : names.join("");
}

const SUPPORTED: Protocol[] = ["vless", "vmess", "trojan", "wireguard"];

/** Schemes worth naming in an error, so a rejection says what the link is rather than "unknown". */
const KNOWN_SCHEMES: Record<string, string> = {
  ss: "Shadowsocks",
  ssr: "ShadowsocksR",
  hysteria: "Hysteria",
  hysteria2: "Hysteria2",
  hy2: "Hysteria2",
  tuic: "TUIC",
  snell: "Snell",
  juicity: "Juicity",
  socks: "SOCKS",
  socks5: "SOCKS",
  http: "HTTP",
  https: "HTTPS",
};

/**
 * Link transport names to the ones sing-box uses.
 *
 * `raw` is Xray's newer name for plain TCP. `h2` and `http` are the same transport. `kcp` and
 * `xhttp` are deliberately absent: the core cannot run them, and a link carrying one is rejected
 * rather than quietly downgraded to TCP, which would connect to the wrong thing.
 */
const TRANSPORTS: Record<string, TransportKind> = {
  "": "tcp",
  tcp: "tcp",
  raw: "tcp",
  none: "tcp",
  ws: "ws",
  websocket: "ws",
  grpc: "grpc",
  http: "http",
  h2: "http",
  h3: "http",
  httpupgrade: "httpupgrade",
  quic: "quic",
};

/** Transports a link may name that this core has no implementation for. */
const UNRUNNABLE: Record<string, string> = {
  kcp: "mKCP",
  mkcp: "mKCP",
  xhttp: "XHTTP",
  splithttp: "SplitHTTP",
  meek: "meek",
};

function emptyTransport(): Transport {
  return {
    kind: "tcp",
    path: "",
    host: "",
    serviceName: "",
    method: "",
    maxEarlyData: 0,
    earlyDataHeader: "",
  };
}

/**
 * Pulls `?ed=N` out of a WebSocket path.
 *
 * The path is what the server matches on, so the marker has to come out of it; left in, the request
 * goes to a path the server has never heard of. The header name is the one every implementation
 * agreed on without it ever being written down.
 */
function splitEarlyData(rawPath: string): { path: string; maxEarlyData: number; earlyDataHeader: string } {
  const at = rawPath.indexOf("?");
  if (at < 0) return { path: rawPath, maxEarlyData: 0, earlyDataHeader: "" };

  const params = new URLSearchParams(rawPath.slice(at + 1));
  const ed = Number(params.get("ed") ?? "");
  if (!Number.isFinite(ed) || ed <= 0) return { path: rawPath, maxEarlyData: 0, earlyDataHeader: "" };

  return {
    path: rawPath.slice(0, at),
    maxEarlyData: ed,
    earlyDataHeader: params.get("eh") || "Sec-WebSocket-Protocol",
  };
}

/** Builds the transport from the fields a link carries, whichever syntax it used to carry them. */
function transportFrom(fields: {
  kind: string;
  headerType?: string;
  path?: string;
  host?: string;
  serviceName?: string;
  method?: string;
}): Transport {
  const named = fields.kind.trim().toLowerCase();

  const unrunnable = UNRUNNABLE[named];
  if (unrunnable) {
    throw new ParseError(`${unrunnable} is not a transport this core can run.`);
  }

  const kind = TRANSPORTS[named];
  if (!kind) throw new ParseError(`Transport "${fields.kind}" is not one this build knows.`);

  const transport = emptyTransport();
  // A TCP link with an HTTP header is the HTTP transport; the link just does not say so.
  transport.kind =
    kind === "tcp" && (fields.headerType ?? "").toLowerCase() === "http" ? "http" : kind;

  const host = (fields.host ?? "").split(",")[0]?.trim() ?? "";

  switch (transport.kind) {
    case "ws": {
      const { path, maxEarlyData, earlyDataHeader } = splitEarlyData(fields.path || "/");
      transport.path = path || "/";
      transport.host = host;
      transport.maxEarlyData = maxEarlyData;
      transport.earlyDataHeader = earlyDataHeader;
      break;
    }
    case "grpc":
      // Panels put the service name in `serviceName`; the VMess JSON form has only `path`.
      transport.serviceName = (fields.serviceName || fields.path || "").replace(/^\//, "");
      break;
    case "http":
      transport.path = fields.path || "/";
      transport.host = host;
      transport.method = fields.method ?? "";
      break;
    case "httpupgrade":
      transport.path = fields.path || "/";
      transport.host = host;
      break;
    case "quic":
    case "tcp":
      break;
  }

  return transport;
}

function tlsFrom(
  security: string,
  q: { sni?: string; host?: string; alpn?: string; fp?: string; pbk?: string; sid?: string; insecure?: boolean },
): TlsOptions {
  const kind = security.trim().toLowerCase();
  const enabled = kind === "tls" || kind === "reality" || kind === "xtls";

  if (kind === "reality" && !q.pbk) {
    // Without the server's public key a Reality handshake cannot even be attempted, and the failure
    // at connect time is opaque. Better to say so now.
    throw new ParseError("A Reality link needs a public key (pbk) and this one has none.");
  }

  return {
    enabled,
    // Falling back to the transport host matches what every other client does: a link that sets
    // only `host` for a CDN expects it to be the SNI too.
    sni: q.sni || q.host || "",
    insecure: q.insecure ?? false,
    alpn: (q.alpn ?? "").split(",").map((a) => a.trim()).filter(Boolean),
    fingerprint: q.fp ?? "",
    reality: kind === "reality" ? { publicKey: q.pbk ?? "", shortId: q.sid ?? "" } : null,
  };
}

function port(value: string | number, fallback = 443): number {
  const n = typeof value === "number" ? value : Number(value || fallback);
  if (!Number.isInteger(n) || n < 1 || n > 65535) {
    throw new ParseError(`Port ${value} is not valid.`);
  }
  return n;
}

/** `1`, `true` and `yes` all appear in the wild for the same flag. */
function truthy(value: string | null | undefined): boolean {
  const v = (value ?? "").trim().toLowerCase();
  return v === "1" || v === "true" || v === "yes";
}

// ---------------------------------------------------------------- vless / vmess-as-url

/**
 * The `protocol://uuid@host:port?params#name` form.
 *
 * VLESS always uses it, and VMess does when a panel writes the newer "AEAD" style link instead of
 * the base64 blob.
 */
function parseUrlForm(link: string, protocol: Protocol): Profile {
  let url: URL;
  try {
    url = new URL(link);
  } catch {
    throw new ParseError("The link is malformed and could not be read.");
  }

  // Trojan puts its password exactly where VLESS and VMess put a UUID, so the two are read from
  // the same place and told apart only when the profile is assembled.
  const credential = decodeURIComponent(url.username);
  if (!credential) {
    throw new ParseError(
      protocol === "trojan"
        ? "The Trojan link has no password before the @."
        : "The link has no UUID before the @.",
    );
  }

  const server = url.hostname.replace(/^\[|\]$/g, "");
  if (!server) throw new ParseError("The link has no server address.");

  const q = url.searchParams;
  const security = q.get("security") ?? (q.get("tls") === "1" ? "tls" : "none");

  const transport = transportFrom({
    kind: q.get("type") ?? q.get("net") ?? "tcp",
    headerType: q.get("headerType") ?? undefined,
    path: q.get("path") ?? undefined,
    host: q.get("host") ?? undefined,
    serviceName: q.get("serviceName") ?? undefined,
    method: q.get("method") ?? undefined,
  });

  const tls = tlsFrom(security, {
    sni: q.get("sni") ?? undefined,
    host: transport.host || undefined,
    alpn: q.get("alpn") ?? undefined,
    fp: q.get("fp") ?? undefined,
    pbk: q.get("pbk") ?? undefined,
    sid: q.get("sid") ?? undefined,
    insecure: truthy(q.get("allowInsecure")) || truthy(q.get("insecure")),
  });

  return {
    protocol,
    name: decodeURIComponent(url.hash.slice(1)) || server,
    server,
    port: port(url.port),
    uuid: protocol === "trojan" ? "" : credential,
    password: protocol === "trojan" ? credential : "",
    flow: protocol === "vless" ? (q.get("flow") ?? "") : "",
    security: protocol === "vmess" ? q.get("encryption") || "auto" : "",
    alterId: protocol === "vmess" ? Number(q.get("alterId") ?? 0) || 0 : 0,
    tls,
    transport,
    wireguard: null,
  };
}

// ---------------------------------------------------------------- vmess base64

/** Decodes base64 that may be URL-safe, and may have had its padding stripped. */
function decodeBase64(raw: string): string {
  const normalised = raw.replace(/-/g, "+").replace(/_/g, "/").replace(/\s/g, "");
  const padded = normalised + "=".repeat((4 - (normalised.length % 4)) % 4);
  try {
    // Round-trips through UTF-8, so a name with non-Latin characters survives.
    return new TextDecoder().decode(Uint8Array.from(atob(padded), (c) => c.charCodeAt(0)));
  } catch {
    throw new ParseError("The VMess link is not valid base64.");
  }
}

/**
 * The v2rayN form: `vmess://` followed by base64 of a flat JSON object.
 *
 * Every field arrives as a string or a number depending on which tool wrote it, so nothing here
 * trusts the type it is given.
 */
function parseVmessJson(body: string): Profile {
  let raw: Record<string, unknown>;
  try {
    raw = JSON.parse(decodeBase64(body)) as Record<string, unknown>;
  } catch (e) {
    if (e instanceof ParseError) throw e;
    throw new ParseError("The VMess link does not contain readable JSON.");
  }

  const str = (key: string): string => {
    const v = raw[key];
    return v === undefined || v === null ? "" : String(v);
  };

  const server = str("add");
  if (!server) throw new ParseError("The VMess link has no server address (add).");

  const uuid = str("id");
  if (!uuid) throw new ParseError("The VMess link has no UUID (id).");

  const transport = transportFrom({
    kind: str("net") || "tcp",
    headerType: str("type"),
    path: str("path"),
    host: str("host"),
    // The JSON form has no serviceName; gRPC puts it in `path`.
    serviceName: str("net").toLowerCase() === "grpc" ? str("path") : undefined,
  });

  const tls = tlsFrom(str("tls") || "none", {
    sni: str("sni"),
    host: transport.host || str("host"),
    alpn: str("alpn"),
    fp: str("fp"),
    insecure: truthy(str("allowInsecure")) || truthy(str("skip-cert-verify")),
  });

  return {
    protocol: "vmess",
    name: str("ps") || server,
    server,
    port: port(str("port")),
    uuid,
    flow: "",
    security: str("scy") || "auto",
    alterId: Number(str("aid")) || 0,
    password: "",
    tls,
    transport,
    wireguard: null,
  };
}

/**
 * The `wireguard://privateKey@host:port?publickey=…&address=…` form.
 *
 * Shares none of the URL form above beyond the URL itself: there is no UUID, no TLS layer and no
 * V2Ray transport, and the two fields that decide whether it works at all — the interface's own
 * addresses and the peer's public key — have no equivalent in the other protocols.
 *
 * `reserved` is Cloudflare WARP's client identifier. It is accepted either as three comma-separated
 * numbers, which is what a link carries, or base64, which is what some panels export. Getting it
 * wrong fails silently: the server drops the handshake rather than refusing it, so the tunnel comes
 * up and passes nothing.
 */
function parseWireguard(link: string): Profile {
  let url: URL;
  try {
    url = new URL(link);
  } catch {
    throw new ParseError("The link is malformed and could not be read.");
  }

  const privateKey = decodeURIComponent(url.username);
  if (!privateKey) throw new ParseError("The WireGuard link has no private key before the @.");

  const server = url.hostname.replace(/^\[|\]$/g, "");
  if (!server) throw new ParseError("The link has no server address.");

  const q = url.searchParams;
  const peerPublicKey = q.get("publickey") ?? q.get("publicKey") ?? q.get("pbk") ?? "";
  if (!peerPublicKey) {
    throw new ParseError("A WireGuard link needs the peer's public key and this one has none.");
  }

  const localAddress = (q.get("address") ?? q.get("ip") ?? "")
    .split(",")
    .map((a) => a.trim())
    .filter(Boolean);
  if (!localAddress.length) {
    throw new ParseError(
      "A WireGuard link needs the addresses assigned to the interface (address=…) and this one has none.",
    );
  }

  return {
    protocol: "wireguard",
    name: decodeURIComponent(url.hash.slice(1)) || server,
    server,
    port: port(url.port),
    uuid: "",
    flow: "",
    security: "",
    alterId: 0,
    password: "",
    // WireGuard has no TLS layer; "none" is how the existing helper spells its absence.
    tls: tlsFrom("none", {}),
    transport: emptyTransport(),
    wireguard: {
      privateKey,
      peerPublicKey,
      localAddress,
      reserved: parseReserved(q.get("reserved")),
      mtu: Number(q.get("mtu")) || 0,
      keepalive: Number(q.get("keepalive")) || 0,
    },
  };
}

/** WARP's three-byte client id, written either as numbers or as base64. */
function parseReserved(raw: string | null): number[] {
  const value = (raw ?? "").trim();
  if (!value) return [];

  if (/^\d+(\s*,\s*\d+)*$/.test(value)) {
    return value
      .split(",")
      .map((n) => Number(n.trim()))
      .filter((n) => Number.isInteger(n) && n >= 0 && n <= 255);
  }

  try {
    return [...atob(value.replace(/-/g, "+").replace(/_/g, "/"))].map((c) => c.charCodeAt(0));
  } catch {
    // A value that is neither is not worth failing the whole link over: an ordinary peer has none.
    return [];
  }
}

// ---------------------------------------------------------------- entry point

/**
 * Parses one share link.
 *
 * Rejections name the protocol where it is recognisable, because "not supported" on its own leaves
 * someone with a working subscription unable to tell which of forty entries this build skipped.
 */
export function parseShareLink(raw: string): Profile {
  const link = raw.trim();
  const at = link.indexOf("://");
  if (at <= 0) throw new ParseError("That does not look like a share link.");

  const scheme = link.slice(0, at).toLowerCase();

  // Not a protocol: a marker the subscription reader emits for an entry whose outbounds dial
  // through each other. Worth its own sentence, because "unknown protocol" would be wrong — the
  // protocol is one this build runs, it is the topology that is not.
  if (scheme === "chain") {
    throw new ParseError(
      "This entry chains two proxies together, like WARP-over-WARP. This build runs a single hop.",
    );
  }

  if (!SUPPORTED.includes(scheme as Protocol)) {
    const known = KNOWN_SCHEMES[scheme];
    throw new ParseError(
      known
        ? `${known} is not supported yet — this build runs ${supportedNames()}.`
        : `"${scheme}" is not a protocol this build knows.`,
    );
  }

  if (scheme === "wireguard") return parseWireguard(link);

  // Trojan is the URL form with a password where the UUID goes; everything below it is shared.
  if (scheme === "trojan") return parseUrlForm(link, "trojan");

  if (scheme === "vmess") {
    // Both forms are in circulation. The URL one always has an @; the base64 blob never does.
    return link.includes("@") ? parseUrlForm(link, "vmess") : parseVmessJson(link.slice(at + 3));
  }

  return parseUrlForm(link, "vless");
}

/**
 * Writes a profile back out as a share link.
 *
 * The inverse of `parseShareLink`, and the reason editing a server can be a text field rather than
 * a form with a branch per protocol: every field of every protocol already has a place in this
 * syntax, and the parser above is the validator.
 *
 * Round-tripping is the contract — `parseShareLink(toShareLink(p))` must give back `p` — which is
 * why the WebSocket early-data parameter is put back into the path it was lifted out of.
 */
export function toShareLink(profile: Profile): string {
  const name = profile.name ? `#${encodeURIComponent(profile.name)}` : "";
  // An IPv6 literal needs brackets, or the port cannot be told from the address.
  const host =
    profile.server.includes(":") && !profile.server.startsWith("[")
      ? `[${profile.server}]`
      : profile.server;

  if (profile.protocol === "wireguard") {
    const wg = profile.wireguard;
    if (!wg) return `wireguard://${host}:${profile.port}${name}`;
    const q = new URLSearchParams();
    q.set("publickey", wg.peerPublicKey);
    if (wg.localAddress.length) q.set("address", wg.localAddress.join(","));
    if (wg.reserved.length) q.set("reserved", wg.reserved.join(","));
    if (wg.mtu) q.set("mtu", String(wg.mtu));
    if (wg.keepalive) q.set("keepalive", String(wg.keepalive));
    const credential = encodeURIComponent(wg.privateKey);
    return `wireguard://${credential}@${host}:${profile.port}?${q}${name}`;
  }

  const q = new URLSearchParams();
  const t = profile.transport;
  q.set("type", t.kind);
  q.set("security", profile.tls.reality ? "reality" : profile.tls.enabled ? "tls" : "none");

  switch (t.kind) {
    case "ws":
      // `ed` lives inside the path in the link syntax, which is where it was read from.
      q.set("path", t.maxEarlyData ? `${t.path}?ed=${t.maxEarlyData}` : t.path);
      if (t.host) q.set("host", t.host);
      break;
    case "httpupgrade":
      q.set("path", t.path);
      if (t.host) q.set("host", t.host);
      break;
    case "http":
      q.set("path", t.path);
      if (t.host) q.set("host", t.host);
      if (t.method) q.set("method", t.method);
      break;
    case "grpc":
      if (t.serviceName) q.set("serviceName", t.serviceName);
      break;
    case "tcp":
    case "quic":
      break;
  }

  if (profile.tls.enabled) {
    if (profile.tls.sni) q.set("sni", profile.tls.sni);
    if (profile.tls.fingerprint) q.set("fp", profile.tls.fingerprint);
    if (profile.tls.alpn.length) q.set("alpn", profile.tls.alpn.join(","));
    if (profile.tls.insecure) q.set("allowInsecure", "1");
    if (profile.tls.reality) {
      q.set("pbk", profile.tls.reality.publicKey);
      if (profile.tls.reality.shortId) q.set("sid", profile.tls.reality.shortId);
    }
  }

  if (profile.protocol === "vless" && profile.flow) q.set("flow", profile.flow);
  if (profile.protocol === "vmess") {
    q.set("encryption", profile.security || "auto");
    if (profile.alterId) q.set("alterId", String(profile.alterId));
  }

  const credential = encodeURIComponent(
    profile.protocol === "trojan" ? profile.password : profile.uuid,
  );
  return `${profile.protocol}://${credential}@${host}:${profile.port}?${q}${name}`;
}

const PROTOCOL_LABELS: Record<Protocol, string> = {
  vless: "VLESS",
  vmess: "VMess",
  trojan: "Trojan",
  wireguard: "WireGuard",
};

/**
 * How a profile is described in the list and on the status card.
 *
 * One implementation, because three views showed the same sentence and had each grown their own
 * copy of the "Reality or TLS or neither" ladder.
 */
export function describe(profile: Profile): string {
  // WireGuard has no TLS layer to report the absence of, and "no TLS" next to it reads as a
  // warning about something missing rather than a fact about the protocol. What is worth saying
  // is whether the peer is WARP, which the reserved client id is the only reliable sign of.
  if (profile.protocol === "wireguard") {
    return profile.wireguard?.reserved.length ? "WireGuard · WARP" : "WireGuard";
  }

  const security = profile.tls.reality ? "Reality" : profile.tls.enabled ? "TLS" : "no TLS";
  // Spelled the way each protocol spells itself: VLESS is an acronym, VMess is not.
  const name = PROTOCOL_LABELS[profile.protocol] ?? profile.protocol;
  // TCP is the default and saying so adds nothing; any other transport is the interesting part.
  const transport = profile.transport?.kind;
  return transport && transport !== "tcp"
    ? `${name} · ${security} · ${transport}`
    : `${name} · ${security}`;
}
