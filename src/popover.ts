/**
 * The menu-bar popover's page (`popover.html`): shows the main window's model and sends back what
 * the user asked for. The window is `popover.rs`; the contract is `popover-model.ts`.
 *
 * In a browser with `VITE_MOCK=1` there is no main window to talk to, so `popover-preview.ts`
 * plays its part against the mock fixture. `MOCK_CORE` is the literal `false` in any other build,
 * and that import is dropped.
 */

import { emitTo, inTauri, invoke, listen } from "./bridge";
import { MOCK_CORE } from "./mockcore";
import type { PopoverIntent, PopoverModel } from "./popover-model";
import { PopoverView } from "./views/popover";

let send: (intent: PopoverIntent) => void = (intent) =>
  void emitTo("main", "popover-intent", intent);

const view = new PopoverView(document.getElementById("popover")!, {
  onToggle: () => send({ kind: "toggle" }),
  onQuick: (pick) => send({ kind: "quick", pick }),
  onSelect: (id) => {
    send({ kind: "select", id });
    // Back to the connection, which is what picking a config was for.
    view.clearSearch();
  },
  onMode: (mode) => send({ kind: "mode", mode }),
  onBlock: (list, on) => send({ kind: "block", list, on }),
  onSearch: (query) => send({ kind: "search", query }),
  onOpen: () => void invoke("show_main_window").catch(() => {}),
  onQuit: () => void invoke("quit").catch(() => {}),
});

document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  // Escape clears a search first, as a search box's does, and only then closes.
  const box = document.querySelector<HTMLInputElement>(".pv-search input");
  if (box?.value) view.clearSearch();
  else void invoke("hide_popover").catch(() => {});
});

setInterval(() => view.tick(), 1000);

if (inTauri) {
  void listen<PopoverModel>("popover-model", (model) => view.show(model));
  // Shown again: the model may have been sent while hidden, but ask anyway — a window hidden for
  // an hour has nothing to lose by one more.
  void listen<null>("popover-shown", () => send({ kind: "hello", shown: true }));
  // Loaded hidden, with the tray: one model now, so the first click opens a panel already drawn.
  send({ kind: "hello", shown: false });
} else if (MOCK_CORE) {
  void import("./popover-preview").then(({ preview }) => {
    send = preview((model) => view.show(model));
  });
}
