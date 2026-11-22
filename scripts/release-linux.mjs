// Builds the Linux release in WSL from Windows (#57):
//
//   npm run release:linux
//
// Copies the working tree into WSL (scripts/wsl.mjs), installs the
// JavaScript dependencies there, runs `npm run release` on Linux (the
// .deb, .rpm and .AppImage), and copies those packages into this
// checkout's `target/dist/<version>/`, beside the Windows release, whose
// SHA256SUMS.txt then lists both. It reads only ENODIX_UPDATE_ENDPOINT
// (required: the update check's address, the same as the Windows build's),
// unless given `--unsigned` (`npm run release:linux -- --unsigned`): a
// pre-release with no update check, in `target/dist/<version>-unsigned/`.
// On Linux itself, run `npm run release`.
// The distribution needs WebKitGTK and the other Tauri packages: see
// docs/linux.md.

import { mkdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

import { writeChecksums } from "./release-files.mjs";
import { quoted, ROOT, syncIntoWsl, wsl, WSL_FOLDER, wslPath } from "./wsl.mjs";

function fail(message) {
  console.error(`release:linux: ${message}`);
  process.exit(1);
}

if (process.platform !== "win32")
  fail("this builds in WSL from Windows; on Linux run `npm run release`");
const unsigned = process.argv.includes("--unsigned");
const endpoint = process.env.ENODIX_UPDATE_ENDPOINT?.trim();
if (!endpoint && !unsigned)
  fail("ENODIX_UPDATE_ENDPOINT is not set (see docs/releasing.md)");
const version = readFileSync(join(ROOT, "Cargo.toml"), "utf8").match(
  /\[workspace\.package\][^[]*?\nversion\s*=\s*"([^"]+)"/,
)?.[1];
if (!version) fail("no version in Cargo.toml's [workspace.package]");

const folder = unsigned ? `${version}-unsigned` : version;
const dist = join(ROOT, "target", "dist", folder);
mkdirSync(dist, { recursive: true });
const into = wslPath(dist, fail);

syncIntoWsl((message) => console.log(`release:linux: ${message}`), fail);
wsl(
  `set -e; cd ${WSL_FOLDER}; npm ci --no-audit --no-fund; ` +
    (unsigned
      ? "node scripts/release.mjs --unsigned; "
      : `env ENODIX_UPDATE_ENDPOINT=${quoted(endpoint)} node scripts/release.mjs; `) +
    `rm -f ${quoted(into)}/*.deb ${quoted(into)}/*.rpm ${quoted(into)}/*.AppImage; ` +
    `cp target/dist/${quoted(folder)}/*.deb target/dist/${quoted(folder)}/*.rpm ` +
    `target/dist/${quoted(folder)}/*.AppImage ${quoted(into)}/`,
  fail,
);

console.log(`\nrelease:linux: ${dist} now holds:`);
for (const name of writeChecksums(dist)) console.log(`  ${name}`);
console.log("\nNothing was uploaded (see docs/releasing.md).");
