/**
 * The Edit sheet for a group: its name, and for a subscription its address — opened from the
 * group header's ⋯ menu.
 *
 * The address is checked by `classify`, the rule the Add servers box applies, so an address that
 * could not have been added cannot be edited in either: `http://` is refused by name, a share link
 * is not a subscription, and an address already on the list belongs to that other group rather
 * than becoming a second copy of it.
 *
 * A changed address is fetched at once, through the ordinary refresh. Until it answers, the old
 * servers stay, as they do during any update; a config the new address still serves keeps its id
 * and its history (`matchExisting`). A rename is kept across refreshes (`Group.renamed`), since the
 * provider's own title would otherwise replace it at the next update.
 */
import { h, render } from "../dom";
import { classify } from "../features/add-servers";
import { store, type Group } from "../store";
import { openSheet, sheetHead } from "./sheets";

/** What `main.ts` supplies so this module never has to reach back into it directly. */
export interface EditGroupHooks {
  log(line: string): void;
  refreshSubscription(group: Group): void;
}

let hooks: EditGroupHooks;

/** Must be called once, during boot, before the Edit sheet can be opened. */
export function initEditGroup(next: EditGroupHooks): void {
  hooks = next;
}

/** Why `url` cannot be this group's address, or null when it can. */
export function addressProblem(url: string, group: Group): string | null {
  if (!url) return "The subscription needs an address.";
  const pasted = classify(url);
  if (pasted.kind === "rejected") return pasted.reason;
  if (pasted.kind === "server") return "That is a server's link, not a subscription address.";
  const other = store.get().groups.find((g) => g.url === url && g.id !== group.id);
  return other ? `That address is already on the list, as ${other.name}.` : null;
}

export function openEditGroup(group: Group) {
  const subscription = group.kind === "subscription";

  openSheet((close) => {
    const field = (label: string, value: string) => {
      const input = h("input", {
        class: "field",
        type: "text",
        spellcheck: false,
        "aria-label": label,
        placeholder: label,
      }) as HTMLInputElement;
      input.value = value;
      return input;
    };
    const nameInput = field("Name", group.name);
    const urlInput = subscription ? field("Address", group.url ?? "") : null;
    const problemLine = h("span", { class: "gpick" });
    const saveButton = h("button", { class: "btn brand" }, "Save") as HTMLButtonElement;

    const problem = () => {
      if (!nameInput.value.trim()) return "The group needs a name.";
      return urlInput ? addressProblem(urlInput.value.trim(), group) : null;
    };
    const check = () => {
      const p = problem();
      saveButton.disabled = p !== null;
      const changedUrl = urlInput && urlInput.value.trim() !== group.url;
      render(problemLine, p ?? (changedUrl ? "The new address is fetched on save" : "Changes apply on save"));
      problemLine.classList.toggle("bad", p !== null);
    };
    nameInput.oninput = check;
    if (urlInput) urlInput.oninput = check;

    saveButton.onclick = () => {
      if (problem()) return;
      const name = nameInput.value.trim();
      const url = urlInput ? urlInput.value.trim() : null;
      const moved = url !== null && url !== group.url;
      store.editGroup(group.id, { name, url });
      hooks.log(`[ui] edited ${name}${moved ? ", with a new address" : ""}`);
      if (moved) {
        const updated = store.get().groups.find((g) => g.id === group.id);
        if (updated) hooks.refreshSubscription(updated);
      }
      close();
    };
    check();

    return h(
      "div",
      { class: "app sheet group-edit", role: "dialog", "aria-label": subscription ? "Edit subscription" : "Edit group" },
      sheetHead(subscription ? "Edit subscription" : "Edit group", close),
      h("label", { class: "flabel" }, "Name", nameInput),
      urlInput ? h("label", { class: "flabel" }, "Address", urlInput) : null,
      urlInput
        ? h("p", { class: "fnote" }, "The address is your account with the provider. Keep it private.")
        : null,
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
