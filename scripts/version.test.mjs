import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { newestTag, nextVersion, VERSION_FIELDS } from "./version.mjs";

test("a fix bumps the last number", () => {
  assert.equal(nextVersion("0.1.0", ["fix(net): move to ureq 3 (#79)"]), "0.1.1");
});

test("a feature bumps the middle number and resets the last", () => {
  assert.equal(nextVersion("0.1.4", ["feat(macos): explain the prompt (#83)"]), "0.2.0");
  assert.equal(nextVersion("0.1.4", ["feat: no scope"]), "0.2.0");
});

test("one feature among fixes is still a feature", () => {
  assert.equal(nextVersion("0.3.2", ["fix: a", "feat(ui): b", "docs: c"]), "0.4.0");
});

test("anything that is not a feature is a patch", () => {
  for (const subject of ["refactor(frontend): split (#78)", "docs: x", "ci: y", "Revert \"feat: z\"", "no type at all"]) {
    assert.equal(nextVersion("0.1.0", [subject]), "0.1.1", subject);
  }
});

test("a breaking change counts as a feature, and never touches the first number", () => {
  assert.equal(nextVersion("0.5.1", ["fix(core)!: drop the old socket"]), "0.6.0");
  assert.equal(nextVersion("1.2.3", ["feat!: new format"]), "1.3.0");
});

test("the workflow's own bump commit is not a release of its own", () => {
  assert.equal(nextVersion("0.2.0", ["chore(release): v0.2.0"]), null);
  assert.equal(nextVersion("0.2.0", []), null);
  assert.equal(nextVersion("0.2.0", ["fix: x", "chore(release): v0.2.0"]), "0.2.1");
});

test("the newest tag is compared as numbers, and suffixed tags are ignored", () => {
  assert.equal(newestTag(["v0.9.0", "v0.10.0", "v0.2.11"]), "v0.10.0");
  assert.equal(newestTag(["v0.3.0", "v1.0.0-rc.1", ""]), "v0.3.0");
  assert.equal(newestTag(["v0.3.0", "v1.0.0"]), "v1.0.0");
  assert.equal(newestTag([""]), null);
});

test("every version field is found once, with LF or with CRLF line endings", () => {
  for (const [path, pattern] of VERSION_FIELDS) {
    const lf = readFileSync(new URL(`../${path}`, import.meta.url), "utf8").replace(/\r\n/g, "\n");
    const crlf = lf.replace(/\n/g, "\r\n");
    for (const [endings, text] of [["LF", lf], ["CRLF", crlf]]) {
      const found = text.match(new RegExp(pattern.source, pattern.flags + "g"))?.length ?? 0;
      assert.equal(found, 1, `${path} with ${endings}: found ${found}`);
    }
  }
});
