// Running in WSL from Windows: the checks of #56 (test-linux.mjs) and the
// Linux release (release-linux.mjs) copy the working tree into the
// distribution's own file system and run there. Building from /mnt/<drive>
// would be slow and would write a Linux target/ into the Windows checkout.
//
//   ENODIX_WSL_DISTRO  the distribution (default: WSL's default one)
//   ENODIX_WSL_DIR     the folder there (default ~/enodix-linux-check)

import { spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");

const distro = process.env.ENODIX_WSL_DISTRO;
const distroArgs = distro ? ["-d", distro] : [];

/** `text` as one single-quoted word for bash. */
export function quoted(text) {
  return `'${text.replaceAll("'", "'\\''")}'`;
}

/** The checkout's folder in WSL. */
const folder = process.env.ENODIX_WSL_DIR || "~/enodix-linux-check";
/** `folder` as bash takes it (`~` expanded). */
export const WSL_FOLDER = folder.startsWith("~/")
  ? `"$HOME"/${quoted(folder.slice(2))}`
  : quoted(folder);

/** Runs `script` in a WSL login shell (so that rustup's ~/.cargo/env is
 * on the PATH), started directly (`--exec`: no other shell expands the
 * script first); `fail` is called with a message if it does not succeed. */
export function wsl(script, fail) {
  const done = spawnSync(
    "wsl",
    [...distroArgs, "--exec", "bash", "-lc", script],
    {
      stdio: "inherit",
    },
  );
  if (done.error) fail(`cannot run wsl: ${done.error.message}`);
  if (done.status !== 0) fail(`in WSL, ${script} failed`);
}

/** The WSL path of the Windows path `path`. */
export function wslPath(path, fail) {
  const where = spawnSync(
    "wsl",
    // Forward slashes: WSL takes backslashes in arguments as escapes.
    [...distroArgs, "--", "wslpath", "-a", path.replaceAll("\\", "/")],
    { encoding: "utf8" },
  );
  if (where.status !== 0)
    fail(
      "WSL cannot run here (is a distribution installed? `wsl -l -v`): " +
        `${(where.stderr || where.stdout || "").trim()}`,
    );
  return where.stdout.trim();
}

/** Copies the working tree (as .gitignore leaves it: no target/, no
 * node_modules, no keys) into WSL_FOLDER. */
export function syncIntoWsl(log, fail) {
  const source = wslPath(ROOT, fail).replace(/\/?$/, "/");
  log(`copying ${source} to ${folder} in WSL`);
  wsl(
    `set -e; mkdir -p ${WSL_FOLDER}; ` +
      // The Windows checkout's NUL files are artefacts of Windows tools.
      `rsync -a --delete --filter=':- .gitignore' --exclude=/.git/ ` +
      `--exclude=NUL ${quoted(source)} ${WSL_FOLDER}/`,
    fail,
  );
}
