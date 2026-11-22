// Runs the checks of everything below the window on Linux (#56):
//
//   npm run test:linux
//
// clippy (warnings are errors) and the tests of the core and the headless
// binary, then the Python bridge's suite (scripts/test-python.mjs). The
// window (`src-tauri`) is left out: it needs WebKitGTK, and the interface
// is checked on Windows.
//
// On Linux it runs in place. On Windows it runs in WSL: the working tree
// (as .gitignore leaves it: no target/, no keys) is copied with rsync into
// the distribution's own file system (ENODIX_WSL_DIR, default
// ~/enodix-linux-check; building on /mnt is slow and would put a Linux
// target/ into the checkout), and the checks run there. ENODIX_WSL_DISTRO
// picks the distribution (default: WSL's default one). The distribution
// needs rustup's Rust, build-essential, pkg-config, libudev-dev, rsync,
// python3 with venv and Node.js: see docs/linux.md.

import { spawnSync } from "node:child_process";

import { ROOT, syncIntoWsl, wsl, WSL_FOLDER } from "./wsl.mjs";

const CRATES = ["-p", "enodix-core", "-p", "enodix-cli"];
const CHECKS = [
  `cargo clippy ${CRATES.join(" ")} --all-targets -- -D warnings`,
  `cargo test ${CRATES.join(" ")}`,
  "node scripts/test-python.mjs",
];

function fail(message) {
  console.error(`test:linux: ${message}`);
  process.exit(1);
}

function run(command, args, options = {}) {
  const done = spawnSync(command, args, { stdio: "inherit", ...options });
  if (done.error) fail(`cannot run ${command}: ${done.error.message}`);
  if (done.status !== 0) fail(`${[command, ...args].join(" ")} failed`);
}

if (process.platform === "linux") {
  for (const check of CHECKS) run("sh", ["-c", check], { cwd: ROOT });
} else if (process.platform === "win32") {
  syncIntoWsl((message) => console.log(`test:linux: ${message}`), fail);
  wsl(`set -e; cd ${WSL_FOLDER}; ${CHECKS.join("; ")}`, fail);
} else {
  fail(`not written for ${process.platform} yet`);
}
console.log("test:linux: every check passed");
