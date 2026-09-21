/**
 * The donation channels: that the shipped ones are well formed, and that the checks catch the
 * mistakes that would send money to nobody.
 *
 * Run with `npm test`; see `usage.test.ts` for why imports name the `.ts` file.
 */

import assert from "node:assert/strict";
import { test } from "node:test";

import { SUPPORT, supportProblems, type Support, type Wallet } from "./support.ts";

test("the channels this build ships are well formed", () => {
  assert.deepEqual(supportProblems(SUPPORT), []);
});

/** A Support panel with nothing in it is worse than none: it reads as a broken feature. */
test("this build offers at least one way to donate", () => {
  assert.ok(SUPPORT.buyMeACoffee !== null || SUPPORT.wallets.length > 0);
});

const only = (wallets: Wallet[], buyMeACoffee: string | null = null): Support => ({ buyMeACoffee, wallets });

test("an address of each network's own shape passes", () => {
  assert.deepEqual(
    supportProblems(
      only([
        { coin: "BTC", network: "bitcoin", address: "bc1qar0srrr7xfkvy5l643lydnw9re59gtzzwf5mdq" },
        { coin: "BTC (legacy)", network: "bitcoin", address: "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa" },
        { coin: "ETH", network: "ethereum", address: "0xde0B295669a9FD93d5F28D9Ec85E40f4cb697BAe" },
        { coin: "USDT", network: "bsc", address: "0x55d398326f99059fF775485246999027B3197955" },
        { coin: "USDT", network: "tron", address: "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t" },
        { coin: "TON", network: "ton", address: "EQCD39VS5jcptHL8vMjEXrzGaRcCVYto7HUn4bpAOg8xqB2N" },
        { coin: "SOL", network: "solana", address: "Vote111111111111111111111111111111111111111" },
      ]),
    ),
    [],
  );
});

test("a dropped character is caught", () => {
  assert.equal(
    supportProblems(only([{ coin: "ETH", network: "ethereum", address: "0xde0B295669a9FD93d5F28D9Ec85E40f4cb697BA" }]))
      .length,
    1,
  );
});

test("a pasted space is caught", () => {
  assert.equal(
    supportProblems(only([{ coin: "USDT", network: "tron", address: "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t " }])).length,
    1,
  );
});

/** The mistake that loses money most often: the right address filed under the wrong network. */
test("an address filed under the wrong network is caught", () => {
  assert.equal(
    supportProblems(only([{ coin: "USDT", network: "ethereum", address: "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t" }])).length,
    1,
  );
});

test("one coin on one network listed twice is caught", () => {
  const wallet: Wallet = { coin: "USDT", network: "tron", address: "TR7NHqjeKQxGTCi8q8ZY4pL8otSzgjLj6t" };
  assert.equal(supportProblems(only([wallet, wallet])).length, 1);
});

test("the Buy Me a Coffee link must be an https page on its own host", () => {
  assert.deepEqual(supportProblems(only([], "https://buymeacoffee.com/nunya")), []);
  assert.deepEqual(supportProblems(only([], "https://www.buymeacoffee.com/nunya")), []);
  for (const bad of [
    "http://buymeacoffee.com/nunya",
    "https://buymeacoffee.com.example.net/nunya",
    "https://buymeacoffee.com@example.net/",
    "https://example.net/buymeacoffee.com",
  ]) {
    assert.equal(supportProblems(only([], bad)).length, 1, bad);
  }
});
