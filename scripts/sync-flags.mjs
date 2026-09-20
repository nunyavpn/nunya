/**
 * Copies the flag SVGs out of `flag-icons` and into `public/flags/`.
 *
 * The flags used to be hand-written CSS gradients — a few bands of colour per country. That works
 * for a tricolour and falls apart for anything with a canton or a coat of arms, and it only
 * covered the couple of dozen countries someone had got around to writing. Now that the exit
 * country is measured rather than guessed from a server's name, a server can legitimately be
 * anywhere, and a missing entry means a blank chip.
 *
 * They are copied rather than imported so nothing has to run: a flag is a background image at
 * `/flags/<code>.svg`, which the page's CSP already allows as `img-src 'self'`, and the browser
 * fetches only the handful actually on screen. The whole set is 2.7 MB on disk and about 4 KB per
 * flag in flight.
 *
 * `public/flags/` is generated and gitignored, so this runs before dev, build and the style guide.
 */

import { copyFile, mkdir, readdir, stat } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const source = join(root, "node_modules", "flag-icons", "flags", "4x3");
const destination = join(root, "public", "flags");

async function main() {
  let names;
  try {
    names = (await readdir(source)).filter((n) => n.endsWith(".svg"));
  } catch {
    // Not fatal: the app falls back to a plain chip, and failing the build over a decoration
    // would be worse than shipping without it.
    console.warn("sync-flags: flag-icons is not installed; skipping");
    return;
  }

  await mkdir(destination, { recursive: true });

  let copied = 0;
  await Promise.all(
    names.map(async (name) => {
      const from = join(source, name);
      const to = join(destination, name);
      // Skipped when it is already there and the same size, so this costs nothing on the second
      // run — it is in the path of every `npm run dev`.
      try {
        const [a, b] = await Promise.all([stat(from), stat(to)]);
        if (a.size === b.size) return;
      } catch {
        // Not copied yet.
      }
      await copyFile(from, to);
      copied += 1;
    }),
  );

  if (copied) console.log(`sync-flags: copied ${copied} of ${names.length} flags`);
}

await main();
