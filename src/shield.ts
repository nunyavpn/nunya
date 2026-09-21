/**
 * What the shield at the top of the rail says: connected, connecting, off, or not working.
 *
 * It is the one piece of the interface visible on every screen — Settings, Rules and Diagnostics
 * all take the list's place, and the status card's detail goes with it — so it answers the one
 * question worth answering everywhere: is the tunnel working?
 *
 * **Not working** is a tunnel that is up and carries nothing (no public address could be found
 * through it), an attempt to connect that failed, or a core that stopped while connected. It stays
 * until the next attempt or a disconnect: a red that went grey by itself would hide the very thing
 * it is for.
 *
 * **Not ready is not a failure.** A core still starting, or a permission not yet granted, leaves
 * the shield grey — the status card says why — because red at every launch would teach users to
 * ignore red.
 *
 * The words follow the modes' rule (CLAUDE.md, *Modes*): only VPN mode speaks for the device; in
 * proxy mode the shield says what the listener covers.
 *
 * Nothing here imports anything, so it runs under `node --test`.
 */

export type ShieldTone = "on" | "connecting" | "off" | "failed";

export interface ShieldInput {
  connection: "off" | "connecting" | "on";
  mode: "vpn" | "proxy";
  /** Why the last attempt failed, or why a running tunnel stopped; `null` for neither. */
  fault: string | null;
  /** The tunnel's public addresses: `null` while being found, `failed` when none could be. */
  exit: { ipv4: string | null; ipv6: string | null; failed?: string } | null;
}

export interface Shield {
  tone: ShieldTone;
  /** The icon: a check, a plain shield, a slash, an exclamation mark. */
  glyph: "shield-check" | "shield" | "shield-off" | "shield-alert";
  /** The tooltip and accessible name. */
  label: string;
}

export function shieldState(input: ShieldInput): Shield {
  if (input.connection === "connecting") {
    return { tone: "connecting", glyph: "shield", label: "Connecting…" };
  }

  if (input.connection === "on") {
    if (input.exit?.failed) {
      return {
        tone: "failed",
        glyph: "shield-alert",
        label: `Not working: connected, but no public address could be reached through it (${input.exit.failed})`,
      };
    }
    const ip = input.exit?.ipv4 ?? input.exit?.ipv6 ?? null;
    const where = ip ? `exit from ${ip}` : "checking where they exit…";
    return {
      tone: "on",
      glyph: "shield-check",
      label:
        input.mode === "vpn"
          ? `Connected: this device's connections ${where}`
          : `Proxy on: apps set to use it ${where}`,
    };
  }

  if (input.fault) {
    return { tone: "failed", glyph: "shield-alert", label: `Not working: ${input.fault}` };
  }
  return { tone: "off", glyph: "shield-off", label: "Not connected" };
}
