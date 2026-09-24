/**
 * The Add servers sheet: paste a link, read a QR code, or fill in a form — the last sheet still in
 * `main.ts`, extracted out per `ENGINEERING_STANDARDS.md`. `features/add-servers.ts` (#67) is the
 * pure classification this sheet's Link tab calls into; this module is its DOM.
 *
 * All three tabs end up in the same place. A QR code is only a link in another form, so reading
 * one hands its text to the Link tab, where it is parsed, previewed and rejected by name exactly
 * like a paste — there is no second import path to keep honest. Manual entry is the editor's form
 * over an empty profile, for a server someone was given as a list of settings rather than a link.
 *
 * What was pasted survives switching tabs, so looking at the QR tab does not cost a paste.
 */
import { h, render } from "../dom";
import { addSubscription, classify, manualGroupName } from "../features/add-servers";
import { guessCity, guessCountry, place } from "../geo";
import { extractWgQuick, type Profile } from "../share";
import { MANUAL_GROUP_ID, store, type Server } from "../store";
import { ProfileEditor } from "./editor";
import { icon } from "./icons";
import { readQrCode } from "./qr";
import { openSheet, sheetHead } from "./sheets";

/** What `main.ts` supplies so this module never has to reach back into it directly. */
export interface AddServersSheetHooks {
  log(line: string): void;
  /** Measures servers just added, up to the auto-check limit. */
  autoCheck(servers: Server[]): void;
  /** Measures a hand-added server right away — there is no limit to weigh for just one. */
  checkServers(servers: Server[]): void;
}

let hooks: AddServersSheetHooks;

/** Must be called once, during boot, before the Add servers sheet can be opened. */
export function initAddServersSheet(next: AddServersSheetHooks): void {
  hooks = next;
}

export function openAddServers() {
  openSheet((close) => buildAddServers(close));
}

/** The paste shortcut's modifier as this platform spells it. */
const MOD_KEY = /Mac|iPhone|iPad/.test(navigator.platform) ? "⌘" : "Ctrl ";

/** The three ways in. A link is the common case, so it is where the sheet opens. */
type AddTab = "link" | "qr" | "manual";

const ADD_TABS: [AddTab, string][] = [
  ["link", "Link"],
  ["qr", "QR code"],
  ["manual", "Manual"],
];

function buildAddServers(close: () => void) {
  let tab: AddTab = "link";
  let pasted = "";

  const sheet = h("div", { class: "app sheet add", role: "dialog", "aria-label": "Add servers" });

  function show(next: AddTab) {
    tab = next;
    // The form needs the editor's height; a paste box and a drop zone do not.
    sheet.classList.toggle("editor", tab === "manual");
    const pane =
      tab === "link" ? linkPane() : tab === "qr" ? qrPane() : manualPane();
    render(
      sheet,
      sheetHead("Add servers", close),
      h(
        "div",
        { class: "addtabs" },
        h(
          "span",
          { class: "seg", role: "tablist", "aria-label": "How to add" },
          ...ADD_TABS.map(([key, text]) =>
            h(
              "button",
              {
                type: "button",
                role: "tab",
                class: key === tab ? "on" : "",
                "aria-selected": String(key === tab),
                onclick: () => key !== tab && show(key),
              },
              text,
            ),
          ),
        ),
      ),
      ...pane,
    );
    sheet.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }

  // ------------------------------------------------------------ link

  function linkPane(): Node[] {
    let servers: Omit<Server, "id" | "groupId">[] = [];
    let subscriptions: { url: string; name: string }[] = [];
    let rejected: string[] = [];

    const preview = h("div", { class: "parsed" });
    const footNote = h("span", { class: "gpick" });

    const addButton = h(
      "button",
      {
        class: "btn brand",
        disabled: true,
        onclick: () => {
          for (const sub of subscriptions) addSubscription(sub.url, sub.name);
          // MANUAL_GROUP_ID rather than groups[0]: the hand-added group is first only by insertion
          // order, and subscriptions append to the same array.
          if (servers.length) hooks.autoCheck(store.addServers(MANUAL_GROUP_ID, servers));
          close();
        },
      },
      "Add servers",
    );

    const input = h("textarea", {
      class: "paste",
      spellcheck: false,
      "data-autofocus": true,
      placeholder: "vless://uuid@host:443?security=reality&sni=…&pbk=…#Name\nhttps://example.com/sub",
      oninput: (e: Event) => reparse((e.target as HTMLTextAreaElement).value),
    }) as HTMLTextAreaElement;
    input.value = pasted;

    function reparse(raw: string) {
      pasted = raw;
      servers = [];
      subscriptions = [];
      rejected = [];

      // A wg-quick config spans lines and has spaces in it, so it is lifted out whole before the
      // rest is read a word at a time; see `extractWgQuick`.
      const { configs, rest } = extractWgQuick(raw);
      for (const line of [...configs, ...rest.split(/\s+/).filter(Boolean)]) {
        const item = classify(line);
        if (item.kind === "server") servers.push(item.server);
        else if (item.kind === "subscription") subscriptions.push({ url: item.url, name: item.name });
        else rejected.push(item.reason);
      }

      const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? "" : "s"}`;

      const found = [
        subscriptions.length ? plural(subscriptions.length, "subscription") : null,
        servers.length ? plural(servers.length, "server") : null,
        rejected.length ? `${rejected.length} unsupported` : null,
      ].filter(Boolean);

      const willAdd = [
        subscriptions.length ? plural(subscriptions.length, "subscription") : null,
        servers.length ? plural(servers.length, "server") : null,
      ].filter(Boolean);

      addButton.disabled = willAdd.length === 0;
      addButton.textContent = willAdd.length ? `Add ${willAdd.join(" and ")}` : "Add servers";

      // Only a subscription-only paste needs the destination explained; anything with servers in
      // it still lands in the hand-added group.
      render(
        footNote,
        ...(subscriptions.length && !servers.length
          ? ["Each subscription brings its own group"]
          : ["Add to ", h("span", { class: "g" }, manualGroupName())]),
      );

      render(
        preview,
        ...(found.length
          ? [
              h("p", { class: "plabel" }, "Found", h("span", {}, found.join(" · "))),
              // A subscription is listed before its servers exist, because they only arrive with
              // the first fetch. All it can promise at this point is a name and a URL.
              ...subscriptions.map((sub) =>
                h(
                  "div",
                  { class: "prow" },
                  h("span", { class: "pmark ok" }, icon("check", 11)),
                  h("span", { class: "flag dim glyph" }, icon("globe", 11)),
                  h(
                    "span",
                    { class: "pmain" },
                    h("b", {}, sub.name),
                    h("span", {}, "Subscription · servers arrive on the first update"),
                  ),
                ),
              ),
              ...servers.map((server) =>
                h(
                  "div",
                  { class: "prow" },
                  h("span", { class: "pmark ok" }, icon("check", 11)),
                  h("span", { class: "flag", style: `background:${place(server.country).flag}` }),
                  h(
                    "span",
                    { class: "pmain" },
                    h("b", {}, server.profile.name),
                    h("span", {}, `${server.profile.server}:${server.profile.port}`),
                  ),
                ),
              ),
              // Rejected links are named, never silently dropped: this build runs six protocols,
              // so users will paste things it cannot.
              ...rejected.map((reason) =>
                h(
                  "div",
                  { class: "prow" },
                  h("span", { class: "pmark no" }, icon("close", 11)),
                  h("span", { class: "flag dim" }),
                  h("span", { class: "pmain dim" }, h("b", {}, "Not supported"), h("span", {}, reason)),
                ),
              ),
            ]
          : []),
      );
    }

    // Fills the footer and the preview before anything is typed, including after a tab switch.
    reparse(pasted);

    return [
      h(
        "button",
        {
          class: "clipbar",
          onclick: async () => {
            try {
              const text = await navigator.clipboard.readText();
              input.value = text;
              reparse(text);
            } catch {
              hooks.log("[ui] clipboard read was refused");
            }
          },
        },
        icon("clipboard", 16),
        h("span", { class: "ct" }, "Paste from clipboard"),
        h("span", { class: "kbd" }, `${MOD_KEY}V`),
      ),
      input,
      preview,
      h(
        "div",
        { class: "sheet-foot" },
        footNote,
        h("button", { class: "ghost", onclick: close }, "Cancel"),
        addButton,
      ),
    ];
  }

  // ------------------------------------------------------------ QR code

  function qrPane(): Node[] {
    const note = h("p", { class: "fnote qrnote" });
    const picker = h("input", {
      type: "file",
      accept: "image/*",
      hidden: true,
      onchange: () => {
        const file = picker.files?.[0];
        if (file) void read(file);
      },
    }) as HTMLInputElement;

    async function read(image: Blob) {
      render(note, "Reading…");
      note.classList.remove("bad");
      let text: string | null = null;
      try {
        text = await readQrCode(image);
      } catch (e) {
        hooks.log(`[ui] could not read that image: ${String(e)}`);
      }
      if (!text) {
        render(note, "No QR code found in that image. Try a sharper or larger one.");
        note.classList.add("bad");
        return;
      }
      // Appended rather than replacing, so a code can be read on top of links already pasted.
      pasted = pasted.trim() ? `${pasted.trim()}\n${text}` : text;
      show("link");
    }

    const zone = h(
      "div",
      {
        class: "dropzone",
        tabindex: 0,
        role: "button",
        "data-autofocus": true,
        "aria-label": "Choose an image with a QR code",
        onclick: () => picker.click(),
        onkeydown: (e: Event) => {
          const key = (e as KeyboardEvent).key;
          if (key === "Enter" || key === " ") {
            e.preventDefault();
            picker.click();
          }
        },
        // A screenshot on the clipboard is the usual source, so Ctrl+V works here directly.
        onpaste: (e: Event) => {
          const file = [...((e as ClipboardEvent).clipboardData?.files ?? [])].find((f) =>
            f.type.startsWith("image/"),
          );
          if (!file) return;
          e.preventDefault();
          void read(file);
        },
        ondragover: (e: Event) => {
          e.preventDefault();
          zone.classList.add("over");
        },
        ondragleave: () => zone.classList.remove("over"),
        ondrop: (e: Event) => {
          e.preventDefault();
          zone.classList.remove("over");
          const file = (e as DragEvent).dataTransfer?.files?.[0];
          if (file) void read(file);
        },
      },
      icon("scan", 30),
      h("span", { class: "dz-t" }, "Drop an image with a QR code"),
      h("span", { class: "dz-d" }, `or click to choose one · ${MOD_KEY}V pastes a screenshot`),
    );

    render(note, "The code is read on this machine; the image is not kept or sent anywhere.");

    return [
      zone,
      picker,
      note,
      h(
        "div",
        { class: "sheet-foot" },
        h("span", { class: "gpick" }),
        h("button", { class: "ghost", onclick: close }, "Cancel"),
      ),
    ];
  }

  // ------------------------------------------------------------ manual

  function manualPane(): Node[] {
    const body = h("div", { class: "set-scroll" });
    const problemLine = h("span", { class: "gpick" });

    const nameInput = h("input", {
      class: "field",
      type: "text",
      spellcheck: false,
      "aria-label": "Name",
      placeholder: "Name — optional",
      "data-autofocus": true,
    }) as HTMLInputElement;

    const addButton = h("button", { class: "btn brand" }, "Add server") as HTMLButtonElement;

    // The first problem only, as in the editor; with none left, the line says where it goes.
    const showProblems = (problems: { message: string }[]) => {
      addButton.disabled = problems.length > 0;
      problemLine.classList.toggle("bad", problems.length > 0);
      if (problems.length) render(problemLine, problems[0].message);
      else render(problemLine, "Add to ", h("span", { class: "g" }, manualGroupName()));
    };

    const editor = new ProfileEditor(body, blankProfile(), showProblems);

    addButton.onclick = () => {
      if (editor.problems().length) return;
      const profile = editor.value();
      const typed = nameInput.value.trim();
      // Something has to be in the list; the address is the one thing certain to be filled in.
      const name = typed || profile.server;
      const added = store.addServers(MANUAL_GROUP_ID, [
        {
          profile: { ...profile, name },
          country: guessCountry(name),
          city: guessCity(name),
          latency: null,
          testedAt: null,
          // A typed name is the user's, and the row should show it rather than a country guess.
          renamed: typed ? true : undefined,
        },
      ]);
      hooks.log(`[ui] added ${name} by hand`);
      hooks.checkServers(added);
      close();
    };

    editor.render();
    // Shown from the start: an empty form is not yet addable, and saying why beats a disabled
    // button with no explanation.
    showProblems(editor.problems());

    return [
      h("label", { class: "flabel" }, "Name", nameInput),
      body,
      h(
        "div",
        { class: "sheet-foot" },
        problemLine,
        h("button", { class: "ghost", onclick: close }, "Cancel"),
        addButton,
      ),
    ];
  }

  show(tab);
  return sheet;
}

/**
 * What manual entry starts from: VLESS over TLS on 443, which is what most servers handed out as a
 * list of settings are. Everything else is empty, so nothing plausible-looking is invented.
 */
function blankProfile(): Profile {
  return {
    protocol: "vless",
    name: "",
    server: "",
    port: 443,
    uuid: "",
    flow: "",
    security: "",
    alterId: 0,
    password: "",
    tls: { enabled: true, sni: "", insecure: false, alpn: [], fingerprint: "", reality: null },
    transport: {
      kind: "tcp",
      path: "",
      host: "",
      serviceName: "",
      method: "",
      maxEarlyData: 0,
      earlyDataHeader: "",
    },
    wireguard: null,
  };
}
