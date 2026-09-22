/**
 * A simulated core for the browser preview, so `VITE_MOCK=1 npm run dev` can connect.
 *
 * The fixture in `mock.ts` fills the list; this answers the commands the Rust side would, so the
 * rest of the interface can be worked on — and photographed — without a Tauri window: connecting
 * and disconnecting, live traffic on the status card, the exit on the map, Quick Connect's
 * re-test, a row's Check. It is deliberately shallow: nothing is proxied, and every answer is made
 * up from the fixture.
 *
 * Only in a browser (`bridge.ts` asks for it only outside Tauri), and only with the flag: like
 * `mock.ts`, `MOCK_CORE` is the literal `false` in an ordinary build and all of this is dropped.
 *
 * Kept apart from `mock.ts`, and importing the store only when a command runs: `bridge.ts` is loaded
 * before anything else, and a static import of the store from here would evaluate it — and the
 * fixture it reads — before they exist.
 */

import type { Server } from "./store";

/** Compared with a string for the reason `MOCK_ENABLED` is; see `mock.ts`. */
export const MOCK_CORE = import.meta.env.VITE_MOCK === "1";

type Handler = (payload: unknown) => void;
const handlers = new Map<string, Set<Handler>>();

let connectedAt = 0;
let running = false;

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

export function mockListen(event: string, handler: Handler): () => void {
  const set = handlers.get(event) ?? new Set<Handler>();
  set.add(handler);
  handlers.set(event, set);
  return () => set.delete(handler);
}

function emit(event: string, payload: unknown) {
  for (const handler of handlers.get(event) ?? []) handler(payload);
}

/** A stable number from a string, so a server tests the same every time. */
function spread(text: string, from: number, to: number): number {
  let hash = 2166136261;
  for (const char of text) hash = Math.imul(hash ^ char.charCodeAt(0), 16777619) >>> 0;
  return from + (hash % (to - from));
}

async function selectedServer(): Promise<Server | undefined> {
  const { store } = await import("./store");
  return store.selected();
}

export async function mockInvoke<T>(command: string, args: Record<string, unknown> = {}): Promise<T> {
  await wait(80);
  const answer = (value: unknown) => value as T;

  switch (command) {
    case "core_connected":
      return answer(true);

    case "tunnel_readiness":
      return answer({ mode: args.mode, transport: "subprocess", ready: true, state: "disconnected", detail: null });

    case "start_tunnel":
      await wait(700);
      running = true;
      connectedAt = Date.now();
      emit("core-log", "INFO inbound/mixed[mixed-in]: tcp server started at 127.0.0.1:2080");
      return answer(null);

    case "stop_tunnel":
      running = false;
      return answer(null);

    // Cumulative, as the real counters are: a steady stream with a little movement in it.
    case "query_stats": {
      if (!running) return answer({ uplink: 0, downlink: 0 });
      const seconds = (Date.now() - connectedAt) / 1000;
      const wave = 1 + 0.35 * Math.sin(seconds / 2.3);
      return answer({
        uplink: Math.round(seconds * 42_000 * wave),
        downlink: Math.round(seconds * 1_680_000 * wave),
      });
    }

    case "locate_me":
      return answer({
        ip: "203.0.113.24",
        country: "IT",
        city: "Milan",
        lat: 45.46,
        lon: 9.19,
        asn: 64501,
        org: "Example Broadband",
      });

    // Where the selected server's traffic comes out: its measured exit, from the fixture.
    case "locate_exit": {
      const exit = (await selectedServer())?.exit;
      if (!exit) throw new Error("no exit measured for this server in the fixture");
      return answer({
        ipv4: exit.ip,
        ipv6: "2001:db8::24",
        cloudflare: null,
        place: {
          ip: exit.ip,
          country: exit.country,
          city: exit.city,
          lat: exit.lat ?? 0,
          lon: exit.lon ?? 0,
          asn: exit.asn ?? null,
          org: exit.org ?? null,
        },
      });
    }

    case "locate_servers": {
      const profiles = (args.profiles as unknown[]) ?? [];
      return answer(profiles.map((_, index) => ({ index, entry: null, exit: null })));
    }

    // Each server answers after its own delay, as the real check streams its results.
    case "check_servers": {
      const profiles = (args.profiles as { name: string }[]) ?? [];
      await Promise.all(
        profiles.map(async (profile, index) => {
          await wait(spread(profile.name, 300, 1400));
          emit("server-checked", {
            run: args.run,
            index,
            latencyMs: spread(profile.name, 24, 260),
            error: null,
            exitIp: null,
            exit: null,
          });
        }),
      );
      return answer(null);
    }

    case "preview_config":
      return answer(
        JSON.stringify(
          { log: { level: "info" }, inbounds: [{ type: "mixed", listen: "127.0.0.1", listen_port: 2080 }], outbounds: ["…"] },
          null,
          2,
        ),
      );

    case "set_system_proxy":
    case "clear_system_proxy":
    case "set_tray_status":
    case "open_external":
      return answer(null);

    default:
      throw new Error(`"${command}" is not simulated in the browser preview`);
  }
}
