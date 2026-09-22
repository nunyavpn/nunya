/**
 * The queue that keeps connecting and disconnecting from overlapping.
 *
 * Run with `npm test`; see `usage.test.ts` for why imports name the `.ts` file.
 */

import assert from "node:assert/strict";
import { test } from "node:test";

import { Serial } from "./serial.ts";

const tick = () => new Promise((resolve) => setTimeout(resolve, 5));

test("a job does not start until the one before it has finished", async () => {
  const serial = new Serial();
  const seen: string[] = [];
  const first = serial.run(async () => {
    seen.push("connect: start");
    await tick();
    seen.push("connect: system proxy set");
  });
  const second = serial.run(async () => {
    seen.push("disconnect: start");
  });
  await Promise.all([first, second]);
  assert.deepEqual(seen, ["connect: start", "connect: system proxy set", "disconnect: start"]);
});

test("a failed job does not stop the ones after it, and its caller still hears of it", async () => {
  const serial = new Serial();
  const failed = serial.run(async () => {
    throw new Error("the core refused the config");
  });
  let ran = false;
  const after = serial.run(async () => {
    ran = true;
  });
  await assert.rejects(failed, /refused/);
  await after;
  assert.ok(ran);
});

test("reconnects asked for while one waits are the one reconnect", async () => {
  const serial = new Serial();
  let reconnects = 0;
  const busy = serial.run(tick);
  const asks = [1, 2, 3].map(() =>
    serial.once("reconnect", async () => {
      reconnects++;
    }),
  );
  await Promise.all([busy, ...asks]);
  assert.equal(reconnects, 1);
});

test("once it has started, a reconnect asked for again is a new one", async () => {
  const serial = new Serial();
  let reconnects = 0;
  let second: Promise<void> | null = null;
  await serial.once("reconnect", async () => {
    reconnects++;
    // Asked for while the first is running: that one is already applying older settings.
    second = serial.once("reconnect", async () => {
      reconnects++;
    });
  });
  await second;
  assert.equal(reconnects, 2);
});
