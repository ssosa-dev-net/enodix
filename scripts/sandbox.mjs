// Runs the smoke test, or the installer check, in Windows Sandbox (#139): a
// throwaway Windows with its own desktop, clipboard and Credential Manager,
// a local account and no Microsoft account, of which nothing is kept.
//
//   npm run smoke:sandbox [-- --no-network] [-- --vgpu]
//   npm run test:installer:sandbox -- --old <older setup.exe> [--new <setup.exe>] [--no-network]
//
// Windows 11 with the Windows Sandbox feature on, and its `wsb` command line
// (it comes with the sandbox's own update, the first time the sandbox
// starts). Before the smoke test: `npm run app:build` and
// `cargo build -p enodix-core --example test_ssh_server` (the sandbox has
// no Rust toolchain). Before the installer check: a release build
// (`npm run release -- --unsigned`).
//
// The sandbox is started with `wsb start`, and its window (the remote
// session `wsb connect` opens, without which nobody is logged on in it) is
// put on a Windows desktop of its own with scripts/run-on-desktop.py:
// nothing appears on the user's screen. Mapped read-only: a copy of what the
// test reads from the tree (`tree/`), the built app's folder, the test
// server, this Node's folder and a Python's folder (`py -3`'s, or
// SANDBOX_PYTHON's folder); writable: the run's `out/` folder, where the
// sandbox writes its log, the pictures and the network logs. Inside,
// scripts/sandbox-inside.ps1 does the work and ends the sandbox. The run
// folder is target/sandbox/<mode>-<time>/; the log is printed at the end and
// the exit code is the test's.
//
// One sandbox at a time, and one app-driving run at a time: the lock
// folder %TEMP%\enodix-app-driving.lock is taken for the run.
//
// vGPU is off unless --vgpu: with it on, the sandbox's own window manager
// (dwm) crashed in a loop and the session was lost on this computer
// (2026-11-21).

import { spawn, spawnSync } from "node:child_process";
import {
  cpSync,
  existsSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  rmdirSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as sleep } from "node:timers/promises";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const LOCK = join(tmpdir(), "enodix-app-driving.lock");
const LOCK_STALE_MS = 40 * 60 * 1000;
const DESKTOP = "enodix-sandbox";
/** Where everything is inside the sandbox. */
const INSIDE = {
  tree: String.raw`C:\enodix-host\tree`,
  app: String.raw`C:\enodix-host\app`,
  server: String.raw`C:\enodix-host\server`,
  node: String.raw`C:\enodix-host\node`,
  python: String.raw`C:\enodix-host\python`,
  oldInstaller: String.raw`C:\enodix-host\old`,
  newInstaller: String.raw`C:\enodix-host\new`,
  out: String.raw`C:\enodix-out`,
};
/** What the tests read from the tree, copied for the sandbox. */
const TREE = [
  "scripts",
  "crates/core/tests/fixtures/keys",
  "src-tauri/tauri.conf.json",
  "Cargo.toml",
];
const TIMEOUT_MS = { smoke: 45 * 60 * 1000, installer: 20 * 60 * 1000 };

function usage(message) {
  console.error(message);
  console.error(
    "usage: node scripts/sandbox.mjs smoke [--no-network] [--vgpu]\n" +
      "       node scripts/sandbox.mjs installer --old <setup.exe> [--new <setup.exe>] [--no-network] [--vgpu]",
  );
  process.exit(2);
}

function options() {
  const [mode, ...rest] = process.argv.slice(2);
  if (mode !== "smoke" && mode !== "installer") usage(`unknown mode ${mode}`);
  const chosen = { mode, network: true, vgpu: false, old: "", new: "" };
  for (let i = 0; i < rest.length; i++) {
    const arg = rest[i];
    if (arg === "--no-network") chosen.network = false;
    else if (arg === "--vgpu") chosen.vgpu = true;
    else if (mode === "installer" && (arg === "--old" || arg === "--new")) {
      const value = rest[++i];
      if (!value) usage(`${arg} needs a file`);
      chosen[arg.slice(2)] = resolve(value);
    } else usage(`unknown option ${arg}`);
  }
  return chosen;
}

/** Runs a program to its end; its output, or a failure. */
function run(program, args, what) {
  const done = spawnSync(program, args, {
    encoding: "utf8",
    windowsHide: true,
  });
  if (done.error) throw new Error(`${what}: ${done.error.message}`);
  if (done.status !== 0) {
    throw new Error(
      `${what} failed (${done.status}): ${(done.stdout + done.stderr).trim()}`,
    );
  }
  return done.stdout;
}

function wsb(...args) {
  return run("wsb", [...args, "--raw"], `wsb ${args[0]}`);
}

function runningSandboxes() {
  return JSON.parse(wsb("list")).WindowsSandboxEnvironments ?? [];
}

/** Waits for the app-driving lock (another agent's run, a smoke run). */
async function takeLock() {
  let said = false;
  for (;;) {
    try {
      mkdirSync(LOCK);
      return;
    } catch (error) {
      if (error.code !== "EEXIST") throw error;
    }
    let age = 0;
    try {
      age = Date.now() - statSync(LOCK).mtimeMs;
    } catch {
      continue; // removed meanwhile
    }
    if (age > LOCK_STALE_MS) {
      console.log(
        `the lock ${LOCK} was left behind (${Math.round(age / 60000)} min): removing it`,
      );
      rmdirSync(LOCK);
      continue;
    }
    if (!said) {
      console.log(`another app-driving run holds ${LOCK}: waiting for it`);
      said = true;
    }
    await sleep(15000);
  }
}

/** The folder of the Python the sandbox gets: SANDBOX_PYTHON (a python.exe
 * or its folder), else the one `py -3` starts. It must be a whole
 * installation that runs from where it is mapped (python.org's, the Python
 * install manager's, uv's). */
function pythonFolder() {
  const given = process.env.SANDBOX_PYTHON;
  if (given)
    return given.toLowerCase().endsWith(".exe") ? dirname(given) : given;
  const prefix = run(
    "py",
    ["-3", "-c", "import sys; print(sys.base_prefix)"],
    "py -3",
  ).trim();
  return prefix;
}

/** The newest setup.exe of this version in target/dist. */
function currentInstaller() {
  const version = JSON.parse(readFileSync(join(ROOT, "package.json"))).version;
  for (const folder of [`${version}-unsigned`, version]) {
    const file = join(
      ROOT,
      "target",
      "dist",
      folder,
      `enodix_${version}_x64-setup.exe`,
    );
    if (existsSync(file)) return file;
  }
  throw new Error(
    `no installer for ${version} in target/dist: build one first (npm run release -- --unsigned)`,
  );
}

function escapeXml(text) {
  return text
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

function configuration({ mappings, network, vgpu }) {
  const folders = mappings
    .map(
      ({ host, inside, writable }) =>
        `<MappedFolder><HostFolder>${escapeXml(host)}</HostFolder>` +
        `<SandboxFolder>${escapeXml(inside)}</SandboxFolder>` +
        `<ReadOnly>${writable ? "false" : "true"}</ReadOnly></MappedFolder>`,
    )
    .join("");
  const command = `powershell.exe -NoProfile -ExecutionPolicy Bypass -File ${INSIDE.tree}\\scripts\\sandbox-inside.ps1`;
  return (
    "<Configuration>" +
    `<vGPU>${vgpu ? "Enable" : "Disable"}</vGPU>` +
    `<Networking>${network ? "Enable" : "Disable"}</Networking>` +
    // Its own clipboard: the user's is never touched.
    "<ClipboardRedirection>Disable</ClipboardRedirection>" +
    "<AudioInput>Disable</AudioInput><VideoInput>Disable</VideoInput>" +
    "<PrinterRedirection>Disable</PrinterRedirection>" +
    "<MemoryInMB>8192</MemoryInMB>" +
    `<MappedFolders>${folders}</MappedFolders>` +
    `<LogonCommand><Command>${escapeXml(command)}</Command></LogonCommand>` +
    "</Configuration>"
  );
}

/** Ends the remote session windows of the sandbox `id` (left on the hidden
 * desktop asking to reconnect once the sandbox is gone). */
function endSessionWindows(id) {
  spawnSync(
    "powershell",
    [
      "-NoProfile",
      "-Command",
      `Get-CimInstance Win32_Process -Filter "Name = 'WindowsSandboxRemoteSession.exe'" | Where-Object { $_.CommandLine -like '*${id}*' } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force }`,
    ],
    { windowsHide: true },
  );
}

/** Prints what is added to a file since the last call. */
function follower(file) {
  let shown = 0;
  return () => {
    if (!existsSync(file)) return;
    const text = readFileSync(file);
    if (text.length > shown) {
      process.stdout.write(text.subarray(shown));
      shown = text.length;
    }
  };
}

async function main() {
  const chosen = options();
  const started = Date.now();
  const stamp = new Date().toISOString().replace(/[:.]/g, "-").slice(0, 19);
  const runFolder = join(ROOT, "target", "sandbox", `${chosen.mode}-${stamp}`);
  const out = join(runFolder, "out");
  const tree = join(runFolder, "tree");
  mkdirSync(out, { recursive: true });
  for (const part of TREE) {
    cpSync(join(ROOT, part), join(tree, part), { recursive: true });
  }

  const app = join(ROOT, "target", "release");
  const mappings = [
    { host: tree, inside: INSIDE.tree },
    { host: dirname(process.execPath), inside: INSIDE.node },
    { host: out, inside: INSIDE.out, writable: true },
  ];
  const job = {
    mode: chosen.mode,
    network: chosen.network,
    vgpu: chosen.vgpu,
    folders: INSIDE,
  };
  if (chosen.mode === "smoke") {
    const server = join(
      ROOT,
      "target",
      "debug",
      "examples",
      "test_ssh_server.exe",
    );
    for (const [file, how] of [
      [join(app, "enodix.exe"), "npm run app:build"],
      [server, "cargo build -p enodix-core --example test_ssh_server"],
    ]) {
      if (!existsSync(file)) throw new Error(`no ${file}: run ${how} first`);
    }
    // Where the test server, built here, reads its fixture keys.
    job.fixtureKeys = join(ROOT, "crates", "core", "tests", "fixtures", "keys");
    mappings.push(
      { host: app, inside: INSIDE.app },
      { host: dirname(server), inside: INSIDE.server },
      { host: pythonFolder(), inside: INSIDE.python },
    );
  } else {
    if (!chosen.old)
      usage("the installer check needs --old <an older release's setup.exe>");
    chosen.new ||= currentInstaller();
    for (const file of [chosen.old, chosen.new]) {
      if (!existsSync(file)) throw new Error(`no installer at ${file}`);
    }
    if (dirname(chosen.old) === dirname(chosen.new)) {
      throw new Error(
        "give the old and the new installer from different folders",
      );
    }
    mappings.push(
      { host: dirname(chosen.old), inside: INSIDE.oldInstaller },
      { host: dirname(chosen.new), inside: INSIDE.newInstaller },
    );
    job.old = basename(chosen.old);
    job.new = basename(chosen.new);
  }
  writeFileSync(join(out, "job.json"), JSON.stringify(job, null, 2));
  const wsbFile = join(runFolder, "sandbox.wsb");
  const config = configuration({ mappings, ...chosen });
  writeFileSync(wsbFile, config);

  if (
    !existsSync(
      join(
        process.env.SystemRoot ?? "C:\\Windows",
        "System32",
        "WindowsSandbox.exe",
      ),
    )
  )
    throw new Error("Windows Sandbox is not turned on on this computer");
  await takeLock();
  let id = null;
  let code = 1;
  try {
    const others = runningSandboxes();
    if (others.length > 0) {
      throw new Error(
        `a sandbox is running already (${others.map((one) => one.Id).join(", ")}): only one can run at a time`,
      );
    }
    console.log(`sandbox run folder: ${runFolder}`);
    id = JSON.parse(wsb("start", "--config", config)).Id;
    console.log(
      `sandbox ${id} started (${Math.round((Date.now() - started) / 1000)} s)`,
    );
    // Nobody is logged on in the sandbox until a remote session connects:
    // its window goes on a desktop of its own.
    const session = spawn(
      "py",
      [
        join(ROOT, "scripts", "run-on-desktop.py"),
        DESKTOP,
        join(runFolder, "connect.log"),
        "wsb",
        "connect",
        "--id",
        id,
      ],
      { stdio: "ignore", windowsHide: true },
    );
    session.on("error", (error) =>
      console.log(`wsb connect: ${error.message}`),
    );
    const done = join(out, "done.json");
    const show = follower(join(out, "sandbox.log"));
    const deadline = Date.now() + TIMEOUT_MS[chosen.mode];
    while (!existsSync(done)) {
      show();
      if (Date.now() > deadline)
        throw new Error("the sandbox run did not end in time");
      if (runningSandboxes().length === 0) {
        throw new Error("the sandbox ended before its run did");
      }
      await sleep(2000);
    }
    await sleep(500);
    show();
    code = JSON.parse(readFileSync(done, "utf8")).code;
  } catch (error) {
    console.log(`the sandbox run stopped: ${error.message}`);
  } finally {
    if (id) {
      // The run ends the sandbox itself; this is for when it did not.
      for (
        let i = 0;
        i < 15 && runningSandboxes().some((one) => one.Id === id);
        i++
      ) {
        await sleep(1000);
      }
      if (runningSandboxes().some((one) => one.Id === id)) {
        try {
          wsb("stop", "--id", id);
        } catch (error) {
          console.log(error.message);
        }
      }
      endSessionWindows(id);
    }
    rmdirSync(LOCK);
  }
  const log = join(
    out,
    chosen.mode === "smoke" ? "smoke.log" : "installer.log",
  );
  console.log("");
  if (existsSync(log)) {
    console.log(`--- ${log}`);
    process.stdout.write(readFileSync(log, "utf8"));
  }
  const kept = existsSync(out) ? readdirSync(out).join(", ") : "nothing";
  console.log(
    `\nsandbox ${chosen.mode} run: exit code ${code}, ${Math.round((Date.now() - started) / 1000)} s; kept in ${out}: ${kept}`,
  );
  process.exit(code);
}

main().catch((error) => {
  console.error(error.message);
  process.exit(1);
});
