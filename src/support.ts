/**
 * How to support Nunya: Buy Me a Coffee and crypto wallets, and nothing else.
 *
 * The one place the channels are written. The Support panel, and the test that guards them, read
 * them from here; the README and `.github/FUNDING.yml` repeat them and must be kept in step.
 *
 * Two channels only, and the panel says so, because the people this client is for are exactly the
 * people a "donate to Nunya" message from somewhere else would be aimed at. A short, fixed list is
 * something a user can check a request against.
 *
 * A channel that is not set up yet is shipped as a placeholder — the Buy Me a Coffee page marked
 * not `live`, a wallet with no `address` — so the panel shows that support is coming without
 * offering anything to pay. A placeholder is never a made-up address: a string that merely looks
 * valid can belong to a stranger, and whatever is sent to it is theirs.
 *
 * Every address is checked against its network's format by `npm test`. A typo in a donation
 * address sends money to nobody, and nothing at runtime would ever notice. A format check cannot
 * prove an address is ours — only reading it against the wallet can — but it catches the dropped
 * character, the pasted space and the address filed under the wrong network.
 *
 * Nothing here imports anything, so it runs under `node --test`.
 */

export type Network = "bitcoin" | "ethereum" | "bsc" | "tron" | "ton" | "solana";

/**
 * The networks a wallet can be on, how the panel names them, and the shape of their addresses.
 *
 * Named by network, not only by coin: USDT on TRON and USDT on Ethereum look alike to someone
 * paying and are not interchangeable — sent to the wrong one, it is lost.
 */
export const NETWORKS: Record<Network, { name: string; pattern: RegExp }> = {
  // Bech32 (`bc1…`, lowercase, no 1/b/i/o after the prefix) or the older base58 forms.
  bitcoin: { name: "Bitcoin", pattern: /^(bc1[ac-hj-np-z02-9]{11,71}|[13][a-km-zA-HJ-NP-Z1-9]{25,34})$/ },
  ethereum: { name: "Ethereum (ERC-20)", pattern: /^0x[0-9a-fA-F]{40}$/ },
  bsc: { name: "BNB Smart Chain (BEP-20)", pattern: /^0x[0-9a-fA-F]{40}$/ },
  tron: { name: "TRON (TRC-20)", pattern: /^T[1-9A-HJ-NP-Za-km-z]{33}$/ },
  // The user-friendly form wallets show: 48 characters of base64url, bounceable or not.
  ton: { name: "TON", pattern: /^(EQ|UQ)[A-Za-z0-9_-]{46}$/ },
  solana: { name: "Solana", pattern: /^[1-9A-HJ-NP-Za-km-z]{32,44}$/ },
};

export interface Wallet {
  /** What to send: `USDT`, `BTC`, `TON`. */
  coin: string;
  network: Network;
  /** `null` until the wallet is set up: listed as coming, with nothing to copy or pay. */
  address: string | null;
}

export interface Support {
  /**
   * The Buy Me a Coffee page, or `null` if there is none. Not `live` until the page is published:
   * the button then says so rather than opening a page that is not there.
   */
  buyMeACoffee: { url: string; live: boolean } | null;
  wallets: Wallet[];
}

/**
 * Hosts the Buy Me a Coffee link may be on. Mirrors `ALLOWED_HOSTS` in `src-tauri/src/external.rs`,
 * which is what actually opens it: a link the Rust side refuses is a button that does nothing.
 */
export const SUPPORT_HOSTS = ["buymeacoffee.com", "www.buymeacoffee.com"];

/**
 * Placeholders until the page is published and the wallets exist (issue #23 stays open until
 * then): the panel shows the channels as coming, and nothing in it can be paid.
 */
export const SUPPORT: Support = {
  buyMeACoffee: { url: "https://buymeacoffee.com/in_alie", live: false },
  wallets: [
    { coin: "USDT", network: "tron", address: null },
    { coin: "BTC", network: "bitcoin", address: null },
    { coin: "TON", network: "ton", address: null },
  ],
};

/** Whether a wallet can be paid: it has a real address. A placeholder has none. */
export function payable(wallet: Wallet): wallet is Wallet & { address: string } {
  return wallet.address !== null;
}

/** Whether any channel can take a donation today. */
export function acceptsDonations(support: Support): boolean {
  return Boolean(support.buyMeACoffee?.live) || support.wallets.some(payable);
}

/** Everything wrong with a set of channels, as sentences; empty when they can ship. */
export function supportProblems(support: Support): string[] {
  const problems: string[] = [];

  if (support.buyMeACoffee !== null) {
    const { url } = support.buyMeACoffee;
    const match = /^https:\/\/([^/?#@]+)(?:[/?#]|$)/.exec(url);
    if (!match || !SUPPORT_HOSTS.includes(match[1].toLowerCase())) {
      problems.push(`${url} is not an https:// page on ${SUPPORT_HOSTS.join(" or ")}`);
    }
  }

  const seen = new Set<string>();
  for (const wallet of support.wallets) {
    const network = NETWORKS[wallet.network];
    if (!network) {
      problems.push(`${wallet.coin}: "${wallet.network}" is not a network this panel knows`);
      continue;
    }
    if (payable(wallet) && !network.pattern.test(wallet.address)) {
      problems.push(`${wallet.coin} on ${network.name}: "${wallet.address}" is not a ${network.name} address`);
    }
    const key = `${wallet.coin}|${wallet.network}`;
    if (seen.has(key)) problems.push(`${wallet.coin} on ${network.name} is listed twice`);
    seen.add(key);
  }

  return problems;
}
