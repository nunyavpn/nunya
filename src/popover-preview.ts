/**
 * The popover in a browser, against the mock fixture: `VITE_MOCK=1 npm run dev`, then
 * `/popover.html`. Only loaded under `VITE_MOCK` (see `popover.ts`), so it never ships.
 *
 * It plays the main window's part — keeps the connection, answers intents — just far enough to
 * design the panel and take its screenshot. The model is built by the real `popoverModel`, so what
 * is shown here is what the app would send.
 */

import { BLOCK_LISTS, SWITCH, type BlockList, type ListState } from "./blocking";
import { popoverModel, type PopoverState } from "./popover-build";
import type { PopoverIntent, PopoverModel } from "./popover-model";
import { shieldState } from "./shield";
import { store } from "./store";

export function preview(show: (model: PopoverModel) => void): (intent: PopoverIntent) => void {
  const lists: Record<BlockList, ListState> = {
    ads: { updatedAt: Date.now() - 3 * 60 * 60 * 1000, busy: false, error: null },
    trackers: { updatedAt: null, busy: false, error: null },
  };
  let connection: PopoverState["connection"] = "on";
  let connectedAt: number | null = Date.now() - 754_000;
  let query = "";

  const paint = () => {
    const mode = store.settings().mode;
    const exit = connection === "on" ? { ipv4: "192.0.2.4", ipv6: null } : null;
    show(
      popoverModel({
        connection,
        shield: shieldState({ connection, mode, fault: null, exit }),
        connectedAt,
        exit: exit?.ipv4 ?? null,
        problem: null,
        canConnect: Boolean(store.selected()),
        quickBusy: null,
        quickNotes: {},
        blockLists: lists,
        query,
      }),
    );
  };

  const connect = () => {
    connection = "connecting";
    paint();
    setTimeout(() => {
      connection = "on";
      connectedAt = Date.now();
      paint();
    }, 700);
  };

  void store.load().then(paint);
  store.subscribe(paint);

  return (intent) => {
    switch (intent.kind) {
      case "hello":
        break;
      case "toggle":
        if (connection === "on") {
          connection = "off";
          connectedAt = null;
        } else connect();
        break;
      case "quick": {
        const pick = store.quickPicks(Date.now())[intent.pick];
        if (pick) {
          store.select(pick.item.id);
          connect();
        }
        break;
      }
      case "select":
        store.select(intent.id);
        query = "";
        connect();
        break;
      case "mode":
        store.updateSettings({ mode: intent.mode });
        break;
      case "block": {
        store.updateSettings({ [SWITCH[intent.list]]: intent.on });
        const state = lists[intent.list];
        if (intent.on && state.updatedAt === null && BLOCK_LISTS.includes(intent.list)) {
          state.busy = true;
          setTimeout(() => {
            state.busy = false;
            state.updatedAt = Date.now();
            paint();
          }, 900);
        }
        break;
      }
      case "search":
        query = intent.query;
        break;
    }
    paint();
  };
}
