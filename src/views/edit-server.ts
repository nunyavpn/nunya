/**
 * The Edit server sheet: a form over the profile, opened from a row's actions menu — extracted
 * out of `main.ts`'s own "sheets" section per `ENGINEERING_STANDARDS.md`.
 *
 * Not a share link in a text box: a link is a serialisation, and changing a port by finding it
 * between an `@` and a `?` makes the user the parser. A typo there does not fail — it produces a
 * different server. The profile is already structured data on disk, so the form edits that; a link
 * is generated from it only when the server is shared, by `openShareServer`.
 *
 * The name is separate from the rest because it is the one field that is not part of the
 * connection, and because renaming is the edit people make most often.
 */
import { h, render } from "../dom";
import { reconnect } from "../features/tunnel";
import { guessCity, guessCountry } from "../geo";
import type { Profile } from "../share";
import { store, type Server } from "../store";
import { ProfileEditor } from "./editor";
import { isLive, openSheet, sheetHead } from "./sheets";

/** What `main.ts` supplies so this module never has to reach back into it directly. */
export interface EditServerHooks {
  log(line: string): void;
  /** The row's Check, run again because a new address is a different place to measure. */
  checkOne(server: Server): void;
}

let hooks: EditServerHooks;

/** Must be called once, during boot, before the Edit server sheet can be opened. */
export function initEditServer(next: EditServerHooks): void {
  hooks = next;
}

export function openEditServer(server: Server) {
  openSheet((close) => {
    const body = h("div", { class: "set-scroll" });
    const problemLine = h("span", { class: "gpick" });

    const nameInput = h("input", {
      class: "field",
      type: "text",
      spellcheck: false,
      "aria-label": "Name",
      placeholder: "Name",
    }) as HTMLInputElement;
    nameInput.value = server.profile.name;

    const saveButton = h("button", { class: "btn brand" }, "Save") as HTMLButtonElement;

    const editor = new ProfileEditor(body, server.profile, (problems) => {
      saveButton.disabled = problems.length > 0;
      // One at a time, and the first one: a list of five complaints about a half-filled form is
      // noise, and the top field is the one to fix first anyway.
      render(problemLine, problems.length ? problems[0].message : "Changes apply on save");
      problemLine.classList.toggle("bad", problems.length > 0);
    });

    saveButton.onclick = () => {
      if (editor.problems().length) return;

      const edited = editor.value();
      const name = nameInput.value.trim() || edited.name;
      const profile: Profile = { ...edited, name };
      const live = isLive(server);

      store.updateServer(server.id, profile, {
        country: guessCountry(name),
        city: guessCity(name),
        // Only a name that differs from the one the profile already carried counts as a rename;
        // re-saving an untouched sheet should not start overriding the country in the list.
        renamed: server.renamed === true || name !== server.profile.name,
      });
      hooks.log(`[ui] edited ${name}`);
      // A new address is a different place: the old entry and exit described the old one, so
      // they go, and the server is measured again — showing the new address's country at once
      // and the new exit after the test.
      if (profile.server !== server.profile.server || profile.port !== server.profile.port) {
        store.clearLocations(server.id);
        const updated = store.get().servers.find((s) => s.id === server.id);
        if (updated) hooks.checkOne(updated);
      }
      // The tunnel is still running against the old settings, so it has to be rebuilt.
      if (live) void reconnect();
      close();
    };

    editor.render();

    return h(
      "div",
      { class: "app sheet editor", role: "dialog", "aria-label": "Edit server" },
      sheetHead("Edit server", close),
      h("label", { class: "flabel" }, "Name", nameInput),
      body,
      h(
        "div",
        { class: "sheet-foot" },
        problemLine,
        h("button", { class: "ghost", onclick: close }, "Cancel"),
        saveButton,
      ),
    );
  });
}
