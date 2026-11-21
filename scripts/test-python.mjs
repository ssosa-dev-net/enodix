// Runs the Python bridge's test suite (`python/tests`) against the built
// headless binary, like the containerlab suite an opt-in one:
//
//   npm run test:python
//
// It needs Python 3.10 or newer (NETTERM_TEST_PYTHON, else `py -3` on
// Windows, else `python3`, else `python`); without one it says SKIPPED and
// succeeds. It builds `netterm-cli` and the `test_ssh_server` example
// (debug), builds the wheel (scripts/build-wheel.mjs), installs it into a
// virtual environment in `target/python-test/venv-<version>` together with Nornir and
// nornir_netmiko and ncclient (from the package index the first time, so that needs the
// network), and runs the tests with that environment's Python.

import { spawnSync } from "node:child_process";
import { existsSync, rmSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { buildWheel, cliName } from "./build-wheel.mjs";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const WORK = join(ROOT, "target", "python-test");
const WINDOWS = process.platform === "win32";
const EXE = WINDOWS ? ".exe" : "";
/** What the tests need beside the package, from the package index. */
const LIBRARIES = [
  "nornir>=3.3,<4",
  "nornir-netmiko>=1,<2",
  "ncclient>=0.6,<1",
];

function fail(message) {
  console.error(`test:python: ${message}`);
  process.exit(1);
}

function run(command, args, options = {}) {
  const done = spawnSync(command, args, { stdio: "inherit", ...options });
  if (done.error) fail(`cannot run ${command}: ${done.error.message}`);
  if (done.status !== 0)
    fail(`${command} ${args.join(" ")} exited with ${done.status}`);
}

/** A Python of at least 3.10 as [command, ...args], or null. */
function findPython() {
  const candidates = [];
  if (process.env.NETTERM_TEST_PYTHON)
    candidates.push([process.env.NETTERM_TEST_PYTHON]);
  if (WINDOWS) candidates.push(["py", "-3"]);
  candidates.push(["python3"], ["python"]);
  for (const [command, ...args] of candidates) {
    const done = spawnSync(
      command,
      [
        ...args,
        "-c",
        "import sys; print(sys.version_info >= (3, 10), '%d.%d' % sys.version_info[:2], sys.executable)",
      ],
      { encoding: "utf8" },
    );
    if (done.status !== 0) continue;
    const [ok, version, ...executable] = done.stdout.trim().split(" ");
    if (ok === "True") return { executable: executable.join(" "), version };
    console.log(`test:python: ${command} is older than 3.10`);
  }
  return null;
}

const found = findPython();
if (!found) {
  console.log(
    "test:python: Python suite SKIPPED: no Python 3.10 or newer found " +
      "(set NETTERM_TEST_PYTHON to one)",
  );
  process.exit(0);
}
const python = found.executable;
console.log(`test:python: using Python ${found.version}, ${python}`);

// Two builds: with `--example`, cargo builds only that example of every
// package named, so the binary was built only when something else had.
run("cargo", ["build", "-p", cliName()]);
run("cargo", ["build", "-p", "netterm-core", "--example", "test_ssh_server"]);
const debug = join(ROOT, "target", "debug");
const cli = join(debug, `${cliName()}${EXE}`);
const server = join(debug, "examples", `test_ssh_server${EXE}`);

const dist = join(WORK, "dist");
rmSync(dist, { recursive: true, force: true });
const wheel = buildWheel(dist);

// One environment per Python version, so that NETTERM_TEST_PYTHON can try
// another one.
const VENV = join(WORK, `venv-${found.version}`);
const venvPython = join(VENV, WINDOWS ? "Scripts" : "bin", `python${EXE}`);
if (!existsSync(venvPython)) run(python, ["-m", "venv", VENV]);
const pip = [venvPython, "-m", "pip", "--disable-pip-version-check"];
const imports = spawnSync(venvPython, [
  "-c",
  "import nornir, nornir_netmiko, ncclient",
]);
if (imports.status !== 0)
  run(pip[0], [...pip.slice(1), "install", "--quiet", ...LIBRARIES]);
run(pip[0], [
  ...pip.slice(1),
  "install",
  "--quiet",
  "--force-reinstall",
  "--no-deps",
  wheel,
]);

run(
  venvPython,
  ["-m", "unittest", "discover", "-s", join(ROOT, "python", "tests"), "-v"],
  {
    // Not the source tree: the tests import the installed wheel.
    cwd: WORK,
    env: {
      ...process.env,
      NETTERM_CLI: cli,
      NETTERM_TEST_SSH_SERVER: server,
      PYTHONDONTWRITEBYTECODE: "1",
    },
  },
);
