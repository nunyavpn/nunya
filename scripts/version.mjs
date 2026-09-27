#!/usr/bin/env node
// The app's version: which one comes next, and writing it everywhere it is kept.
//
//   node scripts/version.mjs next         prints the version the next release gets
//   node scripts/version.mjs set 0.2.0    writes 0.2.0 into every file that carries it
//
// Every merge to main is a release (release.yml), so the number is worked out, not typed: from
// the newest `vX.Y.Z` tag, a merge that adds a feature (`feat:`) bumps the middle number and
// anything else the last. The first number is never bumped here. It moves only when a person tags
// a stable release by hand (`v1.0.0`), because "this is stable" is a decision, not a commit type.
//
// Tags are the source of truth, not the files: a release that failed to build leaves a tag and no
// release, and the next merge still moves on from it rather than reusing a number. The files follow
// (the workflow commits them back to main) so a local build reports the version it came after.
//
// Node rather than a shell script so the rule is unit-tested (`version.test.mjs`) and runs the same
// on a Mac and a runner; it imports nothing outside Node.

import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

/** The bump commit this workflow pushes back to main. Not a change of its own. */
export const RELEASE_COMMIT = /^chore\(release\):/;

/** `feat:`, `feat(scope):`, and anything marked breaking (`fix!:`), which in beta is a feature. */
const FEATURE = /^(feat(\([^)]*\))?!?|[a-z]+(\([^)]*\))?!):/;

const PLAIN = /^(\d+)\.(\d+)\.(\d+)$/;

/**
 * The version after `last`, given the subjects of the commits since it; null when there is nothing
 * to release (only the workflow's own bump commit, or nothing at all).
 */
export function nextVersion(last, subjects) {
  const m = PLAIN.exec(last);
  if (!m) throw new Error(`${last} is not X.Y.Z`);
  const changes = subjects.filter((s) => s.trim() && !RELEASE_COMMIT.test(s));
  if (!changes.length) return null;
  const [x, y, z] = m.slice(1).map(Number);
  return changes.some((s) => FEATURE.test(s)) ? `${x}.${y + 1}.0` : `${x}.${y}.${z + 1}`;
}

/** The highest plain `vX.Y.Z` tag; suffixed ones (`v1.0.0-rc.1`) are someone's experiment. */
export function newestTag(tags) {
  const key = (t) => PLAIN.exec(t.slice(1)).slice(1).map(Number);
  const plain = tags.filter((t) => t.startsWith("v") && PLAIN.test(t.slice(1)));
  plain.sort((a, b) => {
    const [p, q] = [key(a), key(b)];
    return p[0] - q[0] || p[1] - q[1] || p[2] - q[2];
  });
  return plain.at(-1) ?? null;
}

const root = fileURLToPath(new URL("..", import.meta.url));
const git = (...args) => execFileSync("git", args, { cwd: root, encoding: "utf8" }).trim();

function next() {
  const tag = newestTag(git("tag", "--list", "v*", "--merged", "HEAD").split("\n"));
  // No release yet: the first one is whatever the files say, so the beta starts where they do.
  if (!tag) return JSON.parse(readFileSync(`${root}/package.json`, "utf8")).version;
  const subjects = git("log", "--format=%s", `${tag}..HEAD`).split("\n");
  return nextVersion(tag.slice(1), subjects);
}

/** One replacement that must happen exactly once, so a changed file format fails loudly. */
function replaceOnce(path, pattern, replacement) {
  const file = `${root}/${path}`;
  const text = readFileSync(file, "utf8");
  const found = text.match(new RegExp(pattern.source, pattern.flags + "g"))?.length ?? 0;
  if (found !== 1) throw new Error(`${path}: expected one version to replace, found ${found}`);
  writeFileSync(file, text.replace(pattern, replacement));
}

/**
 * Where the version is kept, each anchored on what surrounds it, so a dependency that happens to
 * share the number is untouched. Line breaks are `\r?\n`: git on a Windows runner checks files out
 * with CRLF, and a plain `\n` there matched nothing, which failed the Windows release build.
 */
export const VERSION_FIELDS = [
  ["package.json", /^( {2}"version": )"[^"]*"/m],
  ["src-tauri/tauri.conf.json", /^( {2}"version": )"[^"]*"/m],
  ["src-tauri/Cargo.toml", /^(\[package\]\r?\nname = "nunya"\r?\nversion = )"[^"]*"/m],
  ["src-tauri/Cargo.lock", /^(name = "nunya"\r?\nversion = )"[^"]*"/m],
  // The lockfile names the root package twice: at the top, and as the "" entry under packages.
  ["package-lock.json", /^( {2}"version": )"[^"]*"/m],
  ["package-lock.json", /^( {4}"": \{\r?\n {6}"name": "[^"]*",\r?\n {6}"version": )"[^"]*"/m],
];

function set(version) {
  if (!PLAIN.test(version)) throw new Error(`${version} is not X.Y.Z`);
  for (const [path, pattern] of VERSION_FIELDS) replaceOnce(path, pattern, `$1"${version}"`);
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [command, arg] = process.argv.slice(2);
  if (command === "next") console.log(next() ?? "");
  else if (command === "set" && arg) set(arg);
  else {
    console.error("usage: version.mjs next | set X.Y.Z");
    process.exit(2);
  }
}
