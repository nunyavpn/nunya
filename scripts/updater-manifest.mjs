#!/usr/bin/env node
// The updater's manifest for one release: `latest.json`, published beside the packages.
//
//   node scripts/updater-manifest.mjs <dir> <version> <owner/repo>   prints it
//
// The app finds the release itself (src-tauri/src/update.rs) and reads this file from it: which
// package to download for its platform, and the signature the package must match. The signature
// is the `.sig` the build wrote beside each package, signed with the key whose public half is
// compiled into the app; a package that does not match it is never installed.
//
// Node rather than inline shell so the shape is unit-tested (`updater-manifest.test.mjs`); it
// imports nothing outside Node.

import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

/** Which package each updater platform installs, by how the release names it. */
const PLATFORMS = [
  ["darwin-aarch64", (name) => name.endsWith("_aarch64.app.tar.gz")],
  ["windows-x86_64", (name) => name.endsWith("_x64-setup.exe")],
];

/**
 * The manifest, from the release's file names and each package's signature (`read(name)` returns
 * a file's text). A package without its signature is an error, not a platform left out: a
 * release that silently stopped updating one platform would go unnoticed.
 */
export function manifest({ version, repo, names, read, date = new Date() }) {
  const platforms = {};
  for (const [platform, matches] of PLATFORMS) {
    const found = names.filter(matches);
    if (found.length > 1) throw new Error(`${platform}: more than one package (${found.join(", ")})`);
    if (!found.length) continue;
    const [name] = found;
    if (!names.includes(`${name}.sig`)) throw new Error(`${name} has no ${name}.sig beside it`);
    platforms[platform] = {
      signature: read(`${name}.sig`).trim(),
      url: `https://github.com/${repo}/releases/download/v${version}/${encodeURIComponent(name)}`,
    };
  }
  if (!Object.keys(platforms).length) throw new Error("no package the updater can install");
  return {
    // The number the app will report once installed: a suffix (`-rc.1`) lives only in the tag.
    version: version.split("-")[0],
    notes: `https://github.com/${repo}/releases/tag/v${version}`,
    pub_date: date.toISOString(),
    platforms,
  };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [dir, version, repo] = process.argv.slice(2);
  if (!dir || !version || !repo) {
    console.error("usage: updater-manifest.mjs <dir> <version> <owner/repo>");
    process.exit(1);
  }
  const names = readdirSync(dir);
  const read = (name) => readFileSync(join(dir, name), "utf8");
  console.log(JSON.stringify(manifest({ version, repo, names, read }), null, 2));
}
