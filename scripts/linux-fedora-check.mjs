// The .rpm on Fedora (#58), without a Fedora computer: a Fedora container
// (podman, rootless) with the .rpm installed by dnf, so that its
// dependencies resolve as on a Fedora desktop, and the app in it driven
// by the desktop check (scripts/linux-desktop-check.mjs) on a desktop of
// its own out of sight.
//
//   npm run test:linux-fedora [-- --rpm <file>] [--desktop xfce|gnome]
//                             [--fedora <release>] [--pictures <folder>]
//
// On Windows it copies the tree into WSL (as `test:linux` does) and runs
// there. The .rpm defaults to the newest in target/dist/ (built with
// `npm run release:linux -- --unsigned`). The image
// (scripts/linux-fedora/Containerfile: the .rpm, Python, xdg-utils,
// Thunar and GNOME Web) is built each run as `enodix-fedora-check`.
//
// The container shares the host's network (the check's servers on
// 127.0.0.1, the virtual display's abstract socket), /tmp (the check's
// folders, the desktop's sockets and bus) and /dev/pts (the serial
// console's pseudo terminal), and runs as the user (--userns=keep-id).
// WebDriver's side (tauri-driver, WebKitWebDriver) stays on the host and
// talks to the Fedora WebKitGTK over its inspector port. The menu entry
// is checked in the image (desktop-file-validate, the icons); the
// desktop's menu, on the host, lists the host's entries, not the
// container's.
//
// Needs: podman (and uidmap) besides what `test:linux-desktop` needs.

import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";

import { quoted, ROOT, syncIntoWsl, wsl, WSL_FOLDER } from "./wsl.mjs";

const IMAGE = "enodix-fedora-check";

function stop(message) {
  console.error(`test:linux-fedora: ${message}`);
  process.exit(1);
}

const options = { rpm: null, desktop: "xfce", fedora: "45", pictures: null };
for (let at = 2; at < process.argv.length; at++) {
  const name = process.argv[at].replace(/^--/, "");
  if (!(name in options)) stop(`unknown option ${process.argv[at]}`);
  options[name] = process.argv[++at];
}

if (process.platform === "win32") {
  syncIntoWsl((message) => console.log(`test:linux-fedora: ${message}`), stop);
  const args = process.argv.slice(2).map(quoted).join(" ");
  wsl(
    `set -e; cd ${WSL_FOLDER}; node scripts/linux-fedora-check.mjs ${args}`,
    stop,
  );
  process.exit(0);
} else if (process.platform !== "linux")
  stop(`not written for ${process.platform}`);

function run(command, args, { quiet = false } = {}) {
  const done = spawnSync(command, args, {
    encoding: "utf8",
    stdio: quiet ? "pipe" : ["ignore", "pipe", "inherit"],
  });
  if (done.error) stop(`${command}: ${done.error.message}`);
  if (done.status !== 0)
    stop(
      `${command} ${args.join(" ")} failed: ${done.stdout}${done.stderr ?? ""}`,
    );
  return done.stdout;
}

/** The newest .rpm under target/dist/. */
function newestRpm() {
  const dist = join(ROOT, "target", "dist");
  const found = [];
  for (const folder of readdirSync(dist, { withFileTypes: true }))
    if (folder.isDirectory())
      for (const name of readdirSync(join(dist, folder.name)))
        if (name.endsWith(".rpm")) found.push(join(dist, folder.name, name));
  found.sort((a, b) => statSync(b).mtimeMs - statSync(a).mtimeMs);
  if (!found.length)
    stop("no .rpm in target/dist/ (npm run release:linux -- --unsigned)");
  return found[0];
}

const rpm = options.rpm ?? newestRpm();
const temp = mkdtempSync(join(tmpdir(), "enodix-fedora-"));
try {
  // The image: the .rpm installed by dnf.
  const context = join(temp, "image");
  mkdirSync(context);
  copyFileSync(
    join(ROOT, "scripts", "linux-fedora", "Containerfile"),
    join(context, "Containerfile"),
  );
  copyFileSync(rpm, join(context, basename(rpm)));
  console.log(`test:linux-fedora: building ${IMAGE} with ${basename(rpm)}`);
  run("podman", [
    "build",
    "--network",
    "host",
    "--build-arg",
    `FEDORA=${options.fedora}`,
    "-t",
    IMAGE,
    context,
  ]);
  const inImage = (script) =>
    run(
      "podman",
      ["run", "--rm", "--network", "host", IMAGE, "sh", "-c", script],
      {
        quiet: true,
      },
    ).trim();
  console.log(
    `ok - in ${inImage("cat /etc/fedora-release")}: ${inImage("rpm -q enodix webkit2gtk4.1 gtk3 | tr '\\n' ' '")}`,
  );
  // What the package put where (a menu entry, its icons, the binaries).
  const entry = inImage(
    "desktop-file-validate /usr/share/applications/enodix.desktop && echo valid; " +
      "grep -E '^(Exec|Icon|Categories)=' /usr/share/applications/enodix.desktop; " +
      "ls /usr/share/icons/hicolor/*/apps/enodix.png; ls /usr/bin/enodix /usr/bin/enodix-cli /usr/lib/enodix/package.txt",
  );
  if (!entry.startsWith("valid")) stop(`the menu entry in the .rpm: ${entry}`);
  console.log(
    `ok - the .rpm's menu entry is valid, with its icons and binaries: ${entry.split("\n").slice(1).join(", ")}`,
  );

  // The app and the headless binary, started in the container as the
  // host's programs would be.
  const home = join(temp, "home");
  mkdirSync(home);
  // What podman and the programs in the container say, shown on failure.
  const log = join(temp, "container.log");
  // podman itself keeps its images under the user's own XDG folders: it
  // runs with those, and the program in the container gets the desktop
  // session's.
  const own = (name) =>
    process.env[name] ? `${name}=${quoted(process.env[name])}` : `-u ${name}`;
  const xdg = [
    "XDG_RUNTIME_DIR",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
  ];
  const container = (program) =>
    [
      "#!/bin/sh",
      // env takes its options (-u) before the assignments.
      `exec env ${xdg
        .map(own)
        .sort((a, b) => Number(!a.startsWith("-")) - Number(!b.startsWith("-")))
        .join(" ")} \\`,
      // cgroupfs: with the desktop's session bus in its environment,
      // crun would ask that bus for systemd, which it does not have.
      `  podman --cgroup-manager=cgroupfs run --rm -i --network host \\`,
      `  --userns=keep-id --env-host \\`,
      ...xdg.map((name) => `  -e ${name}="$${name}" \\`),
      `  -e HOME=${quoted(home)} -v /tmp:/tmp -v /dev/pts:/dev/pts \\`,
      `  ${IMAGE} ${program} "$@" 2>> ${quoted(log)}`,
      "",
    ].join("\n");
  const app = join(temp, "enodix-in-fedora");
  const cli = join(temp, "enodix-cli-in-fedora");
  writeFileSync(app, container("/usr/bin/enodix"));
  writeFileSync(cli, container("/usr/bin/enodix-cli"));
  chmodSync(app, 0o755);
  chmodSync(cli, 0o755);

  const check = spawnSync(
    process.execPath,
    [
      join(ROOT, "scripts", "linux-desktop-check.mjs"),
      "--app",
      app,
      "--cli",
      cli,
      "--packaged",
      "rpm",
      "--desktop",
      options.desktop,
      ...(options.pictures ? ["--pictures", options.pictures] : []),
    ],
    { stdio: "inherit" },
  );
  process.exitCode = check.status ?? 1;
  if (process.exitCode && existsSync(log))
    console.log(
      `test:linux-fedora: the container said:\n${readFileSync(log, "utf8").slice(-3000)}`,
    );
} finally {
  // Containers of the image still running (an app WebDriver left).
  const left = spawnSync(
    "podman",
    ["ps", "-q", "--filter", `ancestor=localhost/${IMAGE}:latest`],
    { encoding: "utf8" },
  )
    .stdout?.split("\n")
    .filter(Boolean);
  if (left?.length) spawnSync("podman", ["rm", "-f", ...left]);
  rmSync(temp, { recursive: true, force: true });
}
