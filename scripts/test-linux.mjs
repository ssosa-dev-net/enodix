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
// the distribution's own file system (NETTERM_WSL_DIR, default
// ~/netterm-linux-check; building on /mnt is slow and would put a Linux
// target/ into the checkout), and the checks run there. NETTERM_WSL_DISTRO
// picks the distribution (default: WSL's default one). The distribution
// needs rustup's Rust, build-essential, pkg-config, libudev-dev, rsync,
// python3 with venv and Node.js: see docs/linux.md.

import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const CRATES = ["-p", "netterm-core", "-p", "netterm-cli"];
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
  const distro = process.env.NETTERM_WSL_DISTRO;
  const wsl = (script, options) =>
    run(
      "wsl",
      [...(distro ? ["-d", distro] : []), "--", "bash", "-lc", script],
      options,
    );
  const where = spawnSync(
    "wsl",
    // Forward slashes: WSL takes backslashes in arguments as escapes.
    [
      ...(distro ? ["-d", distro] : []),
      "--",
      "wslpath",
      "-a",
      ROOT.replaceAll("\\", "/"),
    ],
    { encoding: "utf8" },
  );
  if (where.status !== 0)
    fail(
      "WSL cannot run here (is a distribution installed? `wsl -l -v`): " +
        `${(where.stderr || where.stdout || "").trim()}`,
    );
  const source = where.stdout.trim().replace(/\/?$/, "/");
  const target = process.env.NETTERM_WSL_DIR || "~/netterm-linux-check";
  const quoted = (text) => `'${text.replaceAll("'", "'\\''")}'`;
  const into = target.startsWith("~/")
    ? `"$HOME"/${quoted(target.slice(2))}`
    : quoted(target);
  console.log(`test:linux: copying ${source} to ${target} in WSL`);
  wsl(
    `set -e; mkdir -p ${into}; ` +
      // The Windows checkout's NUL files are artefacts of Windows tools.
      `rsync -a --delete --filter=':- .gitignore' --exclude=/.git/ ` +
      `--exclude=NUL ${quoted(source)} ${into}/`,
  );
  // A login shell, so that rustup's ~/.cargo/env is on the PATH.
  wsl(`set -e; cd ${into}; ${CHECKS.join("; ")}`);
} else {
  fail(`not written for ${process.platform} yet`);
}
console.log("test:linux: every check passed");
