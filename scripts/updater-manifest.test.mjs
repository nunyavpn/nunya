import { test } from "node:test";
import assert from "node:assert/strict";

import { manifest } from "./updater-manifest.mjs";

const files = {
  "Nunya_0.2.0_aarch64.dmg": "",
  "Nunya_0.2.0_aarch64.app.tar.gz": "",
  "Nunya_0.2.0_aarch64.app.tar.gz.sig": "mac-signature\n",
  "Nunya_0.2.0_x64-setup.exe": "",
  "Nunya_0.2.0_x64-setup.exe.sig": "windows-signature\n",
  "Nunya_0.2.0_amd64.deb": "",
  "Nunya_0.2.0_amd64.AppImage": "",
  "Nunya_0.2.0_amd64.AppImage.sig": "linux-signature\n",
  "Nunya_0.2.0_aarch64.AppImage": "",
  "Nunya_0.2.0_aarch64.AppImage.sig": "linux-arm-signature\n",
};
const read = (name) => files[name];
const at = new Date("2026-09-27T12:00:00Z");

test("each platform gets its package and that package's signature", () => {
  const m = manifest({ version: "0.2.0", repo: "o/r", names: Object.keys(files), read, date: at });
  assert.deepEqual(m, {
    version: "0.2.0",
    notes: "https://github.com/o/r/releases/tag/v0.2.0",
    pub_date: "2026-09-27T12:00:00.000Z",
    platforms: {
      "darwin-aarch64": {
        signature: "mac-signature",
        url: "https://github.com/o/r/releases/download/v0.2.0/Nunya_0.2.0_aarch64.app.tar.gz",
      },
      "windows-x86_64": {
        signature: "windows-signature",
        url: "https://github.com/o/r/releases/download/v0.2.0/Nunya_0.2.0_x64-setup.exe",
      },
      "linux-x86_64": {
        signature: "linux-signature",
        url: "https://github.com/o/r/releases/download/v0.2.0/Nunya_0.2.0_amd64.AppImage",
      },
      "linux-aarch64": {
        signature: "linux-arm-signature",
        url: "https://github.com/o/r/releases/download/v0.2.0/Nunya_0.2.0_aarch64.AppImage",
      },
    },
  });
});

test("a package without its signature fails the release", () => {
  const names = Object.keys(files).filter((n) => !n.endsWith("setup.exe.sig"));
  assert.throws(() => manifest({ version: "0.2.0", repo: "o/r", names, read }), /has no .*\.sig/);
});

test("a release with nothing to install fails", () => {
  assert.throws(
    () => manifest({ version: "0.2.0", repo: "o/r", names: ["SHA256SUMS"], read }),
    /no package/,
  );
});

test("the version is the number the installed app reports, without the tag's suffix", () => {
  const m = manifest({ version: "1.0.0-rc.1", repo: "o/r", names: Object.keys(files), read });
  assert.equal(m.version, "1.0.0");
  assert.match(m.platforms["windows-x86_64"].url, /\/v1\.0\.0-rc\.1\//);
});
