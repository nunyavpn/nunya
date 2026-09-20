/**
 * Parses `vless://` share links into the profile shape the Rust side expects.
 *
 * Share links are written by panels and other clients, not by a spec, so this is forgiving about
 * what it accepts and strict about what it reports: a link it cannot fully understand is rejected
 * with a reason rather than silently turned into a profile that fails later at connect time.
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

export interface VlessProfile {
  name: string;
  server: string;
  port: number;
  uuid: string;
  flow: string;
  tls: TlsOptions;
}

export class ParseError extends Error {}

/** Protocols this build can actually run. Anything else is rejected by name, not ignored. */
const SUPPORTED = ["vless"];

const KNOWN_SCHEMES = [
  "vless",
  "vmess",
  "trojan",
  "ss",
  "hysteria2",
  "hy2",
  "tuic",
  "snell",
  "juicity",
  "socks",
  "http",
];

export function parseVless(raw: string): VlessProfile {
  const link = raw.trim();
  const scheme = link.slice(0, Math.max(link.indexOf("://"), 0)).toLowerCase();

  if (!scheme || !link.includes("://")) {
    throw new ParseError("That does not look like a share link.");
  }
  if (!SUPPORTED.includes(scheme)) {
    const known = KNOWN_SCHEMES.includes(scheme) ? scheme : "That protocol";
    throw new ParseError(`${known} is not supported yet — this build runs VLESS only so far.`);
  }

  let url: URL;
  try {
    url = new URL(link);
  } catch {
    throw new ParseError("The link is malformed and could not be read.");
  }

  const uuid = decodeURIComponent(url.username);
  if (!uuid) throw new ParseError("The link has no UUID before the @.");

  const server = url.hostname;
  if (!server) throw new ParseError("The link has no server address.");

  const port = Number(url.port || 443);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    throw new ParseError(`Port ${url.port} is not valid.`);
  }

  const q = url.searchParams;
  const security = (q.get("security") ?? "none").toLowerCase();
  const publicKey = q.get("pbk") ?? "";

  if (security === "reality" && !publicKey) {
    // Without the server's public key a Reality handshake cannot even be attempted, and the
    // failure at connect time is opaque. Better to say so now.
    throw new ParseError("A Reality link needs a public key (pbk) and this one has none.");
  }

  const tls: TlsOptions = {
    enabled: security === "tls" || security === "reality",
    sni: q.get("sni") ?? q.get("host") ?? "",
    insecure: q.get("allowInsecure") === "1",
    alpn: (q.get("alpn") ?? "").split(",").filter(Boolean),
    fingerprint: q.get("fp") ?? "",
    reality: security === "reality" ? { publicKey, shortId: q.get("sid") ?? "" } : null,
  };

  const transport = (q.get("type") ?? "tcp").toLowerCase();
  if (transport !== "tcp" && transport !== "raw") {
    throw new ParseError(`Transport "${transport}" is not wired up yet — only plain TCP is.`);
  }

  return {
    name: decodeURIComponent(url.hash.slice(1)) || server,
    server,
    port,
    uuid,
    flow: q.get("flow") ?? "",
    tls,
  };
}

/** Best-effort two-letter code for the server-row chip, from a name like "DE-4 Frankfurt". */
export function guessCountry(name: string): string {
  const m = name.match(/\b([A-Z]{2})\b/);
  return m ? m[1] : "··";
}
