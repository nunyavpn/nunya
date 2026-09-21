/**
 * The server editor: a form over a profile, not a text box over a share link.
 *
 * A share link is a serialisation, not an interface. Asking someone to change a port by finding it
 * between an `@` and a `?` in a 300-character string is asking them to be a parser, and one typo
 * silently produces a different server rather than an error. The profile is already stored as
 * structured data, so the editor edits that directly. The config the core runs is built from that
 * profile as JSON, and a share link is generated from it only when one is asked for, so the form
 * does not carry a live link alongside it.
 *
 * The form is rebuilt rather than diffed whenever a *structural* choice changes — the protocol, the
 * transport, the security layer — because those decide which fields exist at all. Everything else
 * writes straight into the draft, so a rebuild never loses what was typed.
 *
 * Controls reuse the Advanced panel's vocabulary (`set-group`, `srow`, `slab`, `val`, `seg`) so the
 * two read as one app rather than two people's work.
 */

import { h, render } from "../dom";
import {
  type Profile,
  type Protocol,
  type TransportKind,
  type WireguardOptions,
} from "../share";

/** Everything the form can be in the middle of getting wrong. */
export interface Problem {
  field: string;
  message: string;
}

const PROTOCOLS: [Protocol, string][] = [
  ["vless", "VLESS"],
  ["vmess", "VMess"],
  ["trojan", "Trojan"],
  ["wireguard", "WireGuard"],
];

const TRANSPORTS: [TransportKind, string][] = [
  ["tcp", "TCP"],
  ["ws", "WebSocket"],
  ["grpc", "gRPC"],
  ["http", "HTTP/2"],
  ["httpupgrade", "HTTPUpgrade"],
  ["quic", "QUIC"],
];

/** The ciphers sing-box accepts for VMess. `auto` is right unless a panel says otherwise. */
const VMESS_CIPHERS = ["auto", "aes-128-gcm", "chacha20-poly1305", "none"];

/**
 * ALPN protocols offered as tags. Order is preference order in the ClientHello, so the picker
 * lets the selected ones be rearranged rather than only toggled.
 */
const ALPNS = ["h2", "http/1.1", "h3"];

/**
 * uTLS fingerprints the core implements.
 *
 * Offered as a list rather than a free-text field because a misspelling here does not fail: the
 * core falls back and the connection looks fine while presenting the wrong handshake, which is
 * the one thing a Reality user is choosing a fingerprint to avoid.
 */
const FINGERPRINTS = ["", "chrome", "firefox", "safari", "ios", "android", "edge", "random"];

function emptyWireguard(): WireguardOptions {
  return {
    privateKey: "",
    peerPublicKey: "",
    localAddress: [],
    reserved: [],
    mtu: 0,
    keepalive: 0,
  };
}

/** Which security layer the profile is currently describing. */
type Security = "none" | "tls" | "reality";

function securityOf(profile: Profile): Security {
  if (profile.tls.reality) return "reality";
  return profile.tls.enabled ? "tls" : "none";
}

export class ProfileEditor {
  private draft: Profile;

  constructor(
    private root: HTMLElement,
    profile: Profile,
    /** Called whenever the draft changes, so the sheet can enable or disable its Save button. */
    private onChange: (problems: Problem[]) => void,
  ) {
    // A deep copy, so closing without saving changes nothing. The profile is plain JSON by
    // construction — it is what gets written to the data file.
    this.draft = JSON.parse(JSON.stringify(profile)) as Profile;
  }

  /** The edited profile. Only meaningful when `problems()` is empty. */
  value(): Profile {
    return JSON.parse(JSON.stringify(this.draft)) as Profile;
  }

  /**
   * What is stopping this profile from being saved.
   *
   * Only the fields whose absence makes the server unreachable, not a schema check. A wrong SNI is
   * a mistake the user can see and fix; a missing UUID is a config the core will refuse.
   */
  problems(): Problem[] {
    const p = this.draft;
    const found: Problem[] = [];

    if (!p.server.trim()) found.push({ field: "address", message: "An address is required." });
    if (!Number.isInteger(p.port) || p.port < 1 || p.port > 65535) {
      found.push({ field: "port", message: "The port must be between 1 and 65535." });
    }

    if (p.protocol === "wireguard") {
      const wg = p.wireguard ?? emptyWireguard();
      if (!wg.privateKey.trim()) {
        found.push({ field: "privateKey", message: "A private key is required." });
      }
      if (!wg.peerPublicKey.trim()) {
        found.push({ field: "peerPublicKey", message: "The peer's public key is required." });
      }
      if (!wg.localAddress.length) {
        found.push({
          field: "localAddress",
          message: "WireGuard has no DHCP: the interface addresses have to be stated.",
        });
      }
    } else if (p.protocol === "trojan") {
      if (!p.password.trim()) found.push({ field: "password", message: "A password is required." });
    } else if (!p.uuid.trim()) {
      found.push({ field: "uuid", message: "A UUID is required." });
    }

    if (securityOf(p) === "reality" && !p.tls.reality?.publicKey.trim()) {
      found.push({
        field: "pbk",
        message: "Reality cannot be attempted without the server's public key.",
      });
    }

    return found;
  }

  /** Re-reads validity, without rebuilding the controls. */
  private touched() {
    this.onChange(this.problems());
  }

  /** A structural change: which fields exist has changed, so the form is built again. */
  private rebuild() {
    const scroll = this.root.scrollTop;
    this.render();
    this.root.scrollTop = scroll;
    this.touched();
  }

  render() {
    const p = this.draft;

    render(
      this.root,
      this.group("Server", [
        this.seg("Protocol", null, PROTOCOLS as [string, string][], p.protocol, (v) =>
          this.setProtocol(v as Protocol),
        ),
        this.text("Address", "hostname or IP", p.server, (v) => {
          this.draft.server = v;
        }),
        this.num("Port", null, p.port, 1, 65535, (v) => {
          this.draft.port = v;
        }),
      ]),

      ...this.credentials(),
      ...(p.protocol === "wireguard" ? [] : this.transport()),
      ...(p.protocol === "wireguard" ? [] : this.security()),
    );
  }

  // ---------------------------------------------------------------- sections

  private credentials(): Node[] {
    const p = this.draft;

    if (p.protocol === "wireguard") {
      const wg = p.wireguard ?? emptyWireguard();
      return [
        this.group("Keys", [
          this.text("Private key", "this device's key", wg.privateKey, (v) => {
            wg.privateKey = v;
          }),
          this.text("Peer public key", "the server's key", wg.peerPublicKey, (v) => {
            wg.peerPublicKey = v;
          }),
        ]),
        this.group("Interface", [
          this.text(
            "Addresses",
            "comma separated, with prefix length",
            wg.localAddress.join(", "),
            (v) => {
              wg.localAddress = v
                .split(",")
                .map((a) => a.trim())
                .filter(Boolean);
            },
          ),
          this.text(
            "Reserved",
            "Cloudflare WARP's client id; leave empty otherwise",
            wg.reserved.join(", "),
            (v) => {
              wg.reserved = v
                .split(",")
                .map((n) => Number(n.trim()))
                .filter((n) => Number.isInteger(n) && n >= 0 && n <= 255);
            },
          ),
          this.num("MTU", "0 leaves it to the core", wg.mtu, 0, 9000, (v) => {
            wg.mtu = v;
          }),
          this.num("Keepalive", "seconds; 0 to disable", wg.keepalive, 0, 600, (v) => {
            wg.keepalive = v;
          }),
        ]),
      ];
    }

    if (p.protocol === "trojan") {
      return [
        this.group("Credentials", [
          this.text("Password", null, p.password, (v) => {
            this.draft.password = v;
          }),
        ]),
      ];
    }

    const rows: Node[] = [
      this.text("UUID", null, p.uuid, (v) => {
        this.draft.uuid = v;
      }),
    ];

    if (p.protocol === "vless") {
      rows.push(
        this.seg(
          "Flow",
          "XTLS Vision, where the server offers it",
          [
            ["", "none"],
            ["xtls-rprx-vision", "vision"],
          ],
          p.flow,
          (v) => {
            this.draft.flow = v;
            this.touched();
          },
        ),
      );
    }

    if (p.protocol === "vmess") {
      rows.push(
        this.seg(
          "Cipher",
          null,
          VMESS_CIPHERS.map((c) => [c, c] as [string, string]),
          p.security || "auto",
          (v) => {
            this.draft.security = v;
            this.touched();
          },
        ),
        this.num(
          "Alter ID",
          "0 unless the server is pre-AEAD, which modern ones are not",
          p.alterId,
          0,
          65535,
          (v) => {
            this.draft.alterId = v;
          },
        ),
      );
    }

    return [this.group("Credentials", rows)];
  }

  private transport(): Node[] {
    const t = this.draft.transport;
    const rows: Node[] = [
      this.seg("Type", null, TRANSPORTS as [string, string][], t.kind, (v) => {
        t.kind = v as TransportKind;
        this.rebuild();
      }),
    ];

    switch (t.kind) {
      case "ws":
        rows.push(
          this.text("Path", null, t.path, (v) => {
            t.path = v;
          }),
          this.text("Host header", "defaults to the address", t.host, (v) => {
            t.host = v;
          }),
          this.num("Early data", "0 to disable", t.maxEarlyData, 0, 8192, (v) => {
            t.maxEarlyData = v;
          }),
        );
        break;
      case "httpupgrade":
        rows.push(
          this.text("Path", null, t.path, (v) => {
            t.path = v;
          }),
          this.text("Host header", "defaults to the address", t.host, (v) => {
            t.host = v;
          }),
        );
        break;
      case "http":
        rows.push(
          this.text("Path", null, t.path, (v) => {
            t.path = v;
          }),
          this.text("Host", "the :authority", t.host, (v) => {
            t.host = v;
          }),
          this.text("Method", "empty means the core's default", t.method, (v) => {
            t.method = v;
          }),
        );
        break;
      case "grpc":
        rows.push(
          this.text("Service name", null, t.serviceName, (v) => {
            t.serviceName = v;
          }),
        );
        break;
      case "tcp":
      case "quic":
        rows.push(h("p", { class: "fnote" }, "Nothing to configure for this transport."));
        break;
    }

    return [this.group("Transport", rows)];
  }

  private security(): Node[] {
    const p = this.draft;
    const kind = securityOf(p);

    const rows: Node[] = [
      this.seg(
        "Layer",
        null,
        [
          ["none", "none"],
          ["tls", "TLS"],
          ["reality", "Reality"],
        ],
        kind,
        (v) => this.setSecurity(v as Security),
      ),
    ];

    if (kind !== "none") {
      rows.push(
        this.text("SNI", "the name presented in the handshake", p.tls.sni, (v) => {
          p.tls.sni = v;
        }),
        this.seg(
          "Fingerprint",
          "which client the handshake imitates",
          // A link can carry a fingerprint this list does not name ("randomized", "qq"). It is
          // shown rather than left with nothing highlighted, which would read as "none".
          (FINGERPRINTS.includes(p.tls.fingerprint)
            ? FINGERPRINTS
            : [...FINGERPRINTS, p.tls.fingerprint]
          ).map((f) => [f, f || "none"] as [string, string]),
          p.tls.fingerprint,
          (v) => {
            p.tls.fingerprint = v;
            this.touched();
          },
        ),
      );
    }

    if (kind === "tls") {
      rows.push(
        this.alpn(p.tls),
        this.toggle(
          "Allow insecure",
          "accepts any certificate — only for a server you control",
          p.tls.insecure,
          (v) => {
            p.tls.insecure = v;
            this.touched();
          },
        ),
      );
    }

    if (kind === "reality") {
      const reality = p.tls.reality ?? { publicKey: "", shortId: "" };
      rows.push(
        this.text("Public key", "the server's Reality key", reality.publicKey, (v) => {
          reality.publicKey = v;
        }),
        this.text("Short ID", null, reality.shortId, (v) => {
          reality.shortId = v;
        }),
      );
    }

    return [this.group("Security", rows)];
  }

  // ---------------------------------------------------------------- structural changes

  /**
   * Switching protocol keeps everything the new one can still use.
   *
   * The address, port and TLS layer are properties of where the server is, not of how it
   * authenticates, so they survive. WireGuard is the exception in both directions: it has no TLS
   * and no V2Ray transport, so moving to it puts those out of reach rather than silently carrying
   * settings that will not be emitted.
   */
  private setProtocol(protocol: Protocol) {
    this.draft.protocol = protocol;
    if (protocol === "wireguard" && !this.draft.wireguard) {
      this.draft.wireguard = emptyWireguard();
    }
    if (protocol === "vmess" && !this.draft.security) this.draft.security = "auto";
    // Protocol-specific fields are kept rather than cleared. Both the config builder and the link
    // writer emit them only for the protocol they belong to, so carrying an unused `flow` costs
    // nothing — while clearing it would quietly discard a setting for anyone who clicked through
    // the control to see what was there.
    this.rebuild();
  }

  private setSecurity(kind: Security) {
    const tls = this.draft.tls;
    tls.enabled = kind !== "none";
    // The Reality object is dropped rather than kept aside: a stale public key that reappears
    // when someone toggles back through the control is worse than retyping it.
    tls.reality = kind === "reality" ? (tls.reality ?? { publicKey: "", shortId: "" }) : null;
    this.rebuild();
  }

  // ---------------------------------------------------------------- controls

  private group(title: string, rows: Node[]) {
    return h("div", { class: "set-group" }, h("h4", {}, title), ...rows);
  }

  private label(title: string, hint: string | null) {
    return h(
      "span",
      { class: "slab" },
      h("span", { class: "t" }, title),
      hint ? h("span", { class: "d" }, hint) : null,
    );
  }

  private text(title: string, hint: string | null, value: string, apply: (v: string) => void) {
    return h(
      "div",
      { class: "srow stack" },
      this.label(title, hint),
      h("input", {
        type: "text",
        class: "val",
        spellcheck: false,
        autocapitalize: "off",
        autocomplete: "off",
        value,
        // Per keystroke, unlike the Advanced panel: nothing here is persisted until Save, and
        // the validity line should keep up with what is being typed.
        oninput: (e: Event) => {
          apply((e.target as HTMLInputElement).value);
          this.touched();
        },
      }),
    );
  }

  private num(
    title: string,
    hint: string | null,
    value: number,
    min: number,
    max: number,
    apply: (v: number) => void,
  ) {
    return h(
      "div",
      { class: "srow" },
      this.label(title, hint),
      h("input", {
        type: "number",
        class: "val num",
        value: String(value),
        min: String(min),
        max: String(max),
        oninput: (e: Event) => {
          const parsed = Number((e.target as HTMLInputElement).value);
          // Out of range is left to the validity line rather than snapped back: snapping while
          // someone is still typing "8443" turns an 8 into the maximum.
          apply(Number.isFinite(parsed) ? Math.round(parsed) : NaN);
          this.touched();
        },
      }),
    );
  }

  /**
   * ALPN as tags: click one to add or remove it, drag or use the arrow keys to reorder.
   *
   * A free-text field made the user spell `http/1.1` exactly and gave no sign that the order is
   * meaningful. Selected tags come first, numbered in the order they are offered; unselected ones
   * follow, dimmed. A value this list does not know, from an imported link, is kept as a tag
   * rather than dropped, for the same reason unused protocol fields are kept.
   */
  private alpn(tls: Profile["tls"]) {
    const host = h("span", { class: "tags", role: "listbox", "aria-label": "ALPN" });
    let dragged: string | null = null;

    const move = (value: string, to: number) => {
      const from = tls.alpn.indexOf(value);
      if (from < 0 || to < 0 || to >= tls.alpn.length || to === from) return;
      tls.alpn.splice(from, 1);
      tls.alpn.splice(to, 0, value);
      draw(value);
      this.touched();
    };

    const draw = (focus?: string) => {
      const unselected = ALPNS.filter((a) => !tls.alpn.includes(a));
      render(
        host,
        ...tls.alpn.map((value, i) =>
          h(
            "button",
            {
              type: "button",
              class: "atag on",
              // An enumerated attribute: a bare `draggable` is not "true" and leaves a button undraggable.
              draggable: "true",
              role: "option",
              "aria-selected": "true",
              "data-v": value,
              title: "Click to remove · drag or ← → to reorder",
              onclick: () => {
                tls.alpn = tls.alpn.filter((a) => a !== value);
                draw();
                this.touched();
              },
              onkeydown: (e: Event) => {
                const key = (e as KeyboardEvent).key;
                if (key === "ArrowLeft") move(value, i - 1);
                else if (key === "ArrowRight") move(value, i + 1);
                else return;
                e.preventDefault();
              },
              ondragstart: (e: Event) => {
                dragged = value;
                // WebKitGTK does not start a drag without data set.
                (e as DragEvent).dataTransfer?.setData("text/plain", value);
                (e.currentTarget as HTMLElement).classList.add("dragging");
              },
              ondragend: (e: Event) => {
                dragged = null;
                (e.currentTarget as HTMLElement).classList.remove("dragging");
              },
              ondragover: (e: Event) => {
                if (dragged && dragged !== value) e.preventDefault();
              },
              ondrop: (e: Event) => {
                e.preventDefault();
                if (dragged) move(dragged, i);
              },
            },
            h("span", { class: "n" }, String(i + 1)),
            value,
          ),
        ),
        ...unselected.map((value) =>
          h(
            "button",
            {
              type: "button",
              class: "atag",
              role: "option",
              "aria-selected": "false",
              "data-v": value,
              title: "Click to add",
              onclick: () => {
                tls.alpn = [...tls.alpn, value];
                draw();
                this.touched();
              },
            },
            h("span", { class: "n" }, "+"),
            value,
          ),
        ),
      );
      if (focus) host.querySelector<HTMLElement>(`[data-v="${CSS.escape(focus)}"]`)?.focus();
    };
    draw();

    return h(
      "div",
      { class: "srow stack" },
      this.label("ALPN", "in order of preference · none lets the core decide"),
      host,
    );
  }

  private toggle(title: string, hint: string | null, value: boolean, apply: (v: boolean) => void) {
    return h(
      "label",
      { class: "srow" },
      this.label(title, hint),
      h("input", {
        type: "checkbox",
        class: "sw-input",
        checked: value,
        onchange: (e: Event) => apply((e.target as HTMLInputElement).checked),
      }),
      h("span", { class: "sw", "aria-hidden": "true" }),
    );
  }

  private seg(
    title: string,
    hint: string | null,
    options: [string, string][],
    value: string,
    apply: (v: string) => void,
  ) {
    return h(
      "div",
      { class: "srow stack" },
      this.label(title, hint),
      h(
        "span",
        { class: "seg wrap", role: "radiogroup", "aria-label": title },
        ...options.map(([key, text]) =>
          h(
            "button",
            {
              type: "button",
              class: key === value ? "on" : "",
              role: "radio",
              "aria-checked": String(key === value),
              onclick: (e: Event) => {
                // Structural choices rebuild the form and redraw this for free; the others (flow,
                // cipher, fingerprint) only touch the draft, so the highlight has to move here or
                // the control goes on showing the old choice while the draft holds the new one.
                const picked = e.currentTarget as HTMLElement;
                for (const b of picked.parentElement?.children ?? []) {
                  b.classList.toggle("on", b === picked);
                  b.setAttribute("aria-checked", String(b === picked));
                }
                apply(key);
              },
            },
            text,
          ),
        ),
      ),
    );
  }
}
