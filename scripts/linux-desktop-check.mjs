// Drives the Linux app on a Linux desktop (#114): the points of
// docs/linux.md's "Checking the packages on a Linux desktop" that a script
// can see. Written for WSLg (WSL 2 on Windows 11), where it was run; it
// should run unchanged on a real Linux desktop.
//
//   npm run test:linux-desktop [-- --app <program>] [--backend wayland|x11]
//                               [--pictures <folder>]
//
// On Windows it copies the tree into WSL (scripts/wsl.mjs, as
// `test:linux` does) and runs there. On Linux it runs in place.
//
// The app is driven over WebDriver: `tauri-driver` (cargo install
// tauri-driver) in front of WebKitGTK's WebKitWebDriver (Ubuntu's package
// webkitgtk-webdriver, webkit2gtk-driver on older ones). WebDriver clicks,
// double-clicks and types through the web view's own input path, and its
// clicks fail when something is drawn over the control. The app is the
// installed package's `/usr/bin/netterm` unless `--app` names another (an
// AppImage: its menu entry is then not checked, and its notice must call
// it portable). `--backend` sets GDK_BACKEND (default: what the session
// has; under WSLg that is Wayland). Under X11 the pictures include the
// window as the X server has it (ImageMagick's `import`, found by
// `xdotool`).
//
// It starts, on ports 24722-24728 of 127.0.0.1: the example SSH server
// (with telnet), a release server for the update check, and tauri-driver.
// It starts an ssh-agent of its own holding the test key. The app gets a
// temporary data folder (NETTERM_DATA_DIR) and temporary XDG folders, so
// the user's ~/.local/share/dev.netterm.app is never touched. The
// desktop's openers are replaced on the app's PATH by scripts that record
// what they were asked to open (see `fakeOpener`). Copy and paste use the
// session's clipboard (under WSLg, Windows' clipboard). Pictures of the
// web view are written to `--pictures` (default: a folder in the
// temporary one, kept).
//
// A serial console is opened on a pseudo terminal that `socat` forwards
// to a TCP port the script plays the device on (as a console server's
// port reached with `socat pty,... tcp:...` looks).
//
// `--desktop xfce|gnome` (#58) runs all of it inside a real desktop
// session out of sight (scripts/linux-desktop-session.mjs: XFCE on a
// virtual X display, or GNOME Shell headless on Wayland), and adds what
// only a desktop shows: the menu entry found by the menu's search and
// started from it, light and dark switched by the desktop's own setting
// while the app runs, the frame moved, resized from its edges and
// corners, tiled at the screen's edge, maximized by a double click and
// minimized and brought back with the desktop's real pointer (geometry
// read from the window manager), and "Open log folder" and web links
// starting a real file manager (Thunar) and browser (GNOME Web), with
// the environment those programs got. Pictures of the whole screen are
// added (`*-desktop.png`). `--gallium-driver <name>` sets Mesa's
// GALLIUM_DRIVER for the app (under WSL `d3d12` reaches the computer's
// GPU through /dev/dxg) and requires WebGL's renderer to name it.
//
// `--cli <program>` runs the headless binary from the shell (default
// /usr/bin/netterm-cli) and `--packaged rpm` says `--app` is a package's
// copy: scripts/linux-fedora-check.mjs runs both in a Fedora container.
//
// Without --desktop, not checked here (see docs/linux.md): dragging the
// window, resizing it from its edges, Minimize's effect, edge tiling, and
// what the desktop's opener then does.
//
// Needs, besides those two: Rust (to build the example server),
// openssh-client (ssh-agent, ssh-add), python3 with venv, socat, and the
// app's package installed (/usr/bin/netterm-cli, used from the shell).

import { spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  readlinkSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { connect } from "node:net";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";

import {
  BROWSER,
  FILE_MANAGER,
  SCREEN,
  startDesktop,
} from "./linux-desktop-session.mjs";
import { quoted, ROOT, syncIntoWsl, wsl, WSL_FOLDER } from "./wsl.mjs";

const SSH_PORT = 24722;
const TELNET_PORT = 24723;
const RELEASE_PORT = 24724;
const DRIVER_PORT = 24727;
const NATIVE_PORT = 24728;
const CONSOLE_PORT = 24729;
const USERNAME = "engineer";
const PASSWORD = "correct horse";
const MASTER_PASSWORD = "linux desktop master";
const NEWER_VERSION = "99.0.0";
// Served by the release server, so a real browser opening them is seen.
const RELEASE_PAGE = `http://127.0.0.1:${RELEASE_PORT}/releases/99.0.0`;
const WEB_LINK = `http://127.0.0.1:${RELEASE_PORT}/status`;
const TIMEOUT_MS = 20000;

function fail(message) {
  console.error(`test:linux-desktop: FAILED: ${message}`);
  process.exitCode = 1;
  throw Object.assign(new Error(message), { reported: true });
}

const options = {
  app: "/usr/bin/netterm",
  backend: null,
  pictures: null,
  desktop: null,
  "gallium-driver": null,
  // The headless binary run from a shell, and whether `--app` is a
  // package's copy (`--packaged rpm`: scripts/linux-fedora-check.mjs).
  cli: "/usr/bin/netterm-cli",
  packaged: null,
};
for (let at = 2; at < process.argv.length; at++) {
  const name = process.argv[at].replace(/^--/, "");
  if (!(name in options)) {
    console.error(`test:linux-desktop: unknown option ${process.argv[at]}`);
    process.exit(2);
  }
  options[name] = process.argv[++at];
}
/** Whether the app is a portable copy (an AppImage), not a package's. */
const portable = !(options.packaged || options.app.startsWith("/usr/"));

if (process.platform === "win32") {
  const stop = (message) => {
    console.error(`test:linux-desktop: ${message}`);
    process.exit(1);
  };
  syncIntoWsl((message) => console.log(`test:linux-desktop: ${message}`), stop);
  const args = process.argv.slice(2).map(quoted).join(" ");
  wsl(
    `set -e; cd ${WSL_FOLDER}; node scripts/linux-desktop-check.mjs ${args}`,
    stop,
  );
  process.exit(0);
} else if (process.platform !== "linux") {
  console.error(`test:linux-desktop: not written for ${process.platform}`);
  process.exit(2);
}

const temp = mkdtempSync(join(tmpdir(), "netterm-desktop-"));
const dataFolder = join(temp, "data");
const workspace = join(temp, "workspace");
const keyFile = join(temp, "keys", "rsa_client");
const agentSocket = join(temp, "agent.sock");
const openedFile = join(temp, "opened.txt");
/** What the desktop's opener got as its environment (with --desktop). */
const openerEnvironmentFile = join(temp, "opener-environment.txt");
const consolePort = join(temp, "ttyCONSOLE");
const pictures = options.pictures ?? join(temp, "pictures");
const children = [];
/** The desktop session, with --desktop (startDesktop). */
let desktop = null;
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
const step = (message) => console.log(`ok - ${message}`);
/** A fault that does not stop the run. */
const problem = (message) => {
  console.log(`not ok - ${message}`);
  process.exitCode = 1;
};
const note = (message) => console.log(`   ${message}`);

async function until(description, probe, timeout = TIMEOUT_MS) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    try {
      last = await probe();
      if (last) return last;
    } catch (error) {
      last = error;
    }
    await sleep(150);
  }
  fail(`timed out waiting for ${description} (last: ${String(last)})`);
}

/** Whether `probe` comes true within `timeout` (no failure if not). */
async function settles(probe, timeout = 5000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (
      await Promise.resolve()
        .then(probe)
        .catch(() => false)
    )
      return true;
    await sleep(150);
  }
  return false;
}

/** The processes whose parent is `pid`. */
function childProcesses(pid) {
  const listed = spawnSync("ps", ["-o", "pid=", "--ppid", String(pid)], {
    encoding: "utf8",
  });
  return listed.stdout.split(/\s+/).filter(Boolean).map(Number);
}

/** `pid` and every process below it, parents first. */
function descendants(pid) {
  const found = [pid];
  for (let at = 0; at < found.length; at++)
    found.push(...childProcesses(found[at]));
  return found;
}

/** Whether the process runs (an exited child not yet reaped does not). */
function alive(pid) {
  try {
    return !/^\d+ \(.*\) Z/.test(readFileSync(`/proc/${pid}/stat`, "utf8"));
  } catch {
    return false;
  }
}

/** The workspace: hosts logging in with a password, a key and the agent
 * (profiles of those names), a telnet host, one with a web link, and a
 * Jobs folder with a script. */
function createWorkspace() {
  mkdirSync(join(workspace, "Lab"), { recursive: true });
  mkdirSync(join(workspace, "Jobs"), { recursive: true });
  writeFileSync(join(workspace, "Lab", "_folder.yaml"), `port: ${SSH_PORT}\n`);
  for (const [name, profile] of [
    ["r1-password", "lab-password"],
    ["r1-key", "lab-key"],
    ["r1-agent", "lab-agent"],
  ])
    writeFileSync(
      join(workspace, "Lab", `${name}.yaml`),
      `name: ${name}\naddress: 127.0.0.1\ncredential_profile: ${profile}\n` +
        (name === "r1-password" ? `web_links:\n  status: ${WEB_LINK}\n` : ""),
    );
  writeFileSync(
    join(workspace, "Lab", "sw-telnet.yaml"),
    `name: sw-telnet\naddress: 127.0.0.1\nport: ${TELNET_PORT}\nprotocol: telnet\n`,
  );
  writeFileSync(
    join(workspace, "Jobs", "jobs-r1.yaml"),
    `name: jobs-r1\naddress: 127.0.0.1\nport: ${SSH_PORT}\ncredential_profile: lab-password\n`,
  );
  writeFileSync(
    join(workspace, "Jobs", "linux_check.py"),
    [
      "import platform",
      "import netterm",
      'hosts = netterm.inventory()["hosts"]',
      'print("hosts", len(hosts))',
      'print("system", platform.system())',
      'print("login", netterm.credentials(None, hosts[0]["id"])["username"])',
      "",
    ].join("\n"),
  );
  mkdirSync(dataFolder);
}

/** Stand-ins for the desktop's openers: they record what they are asked
 * to open. The app opens through the `open` crate, which tries xdg-open
 * first, then gio and others; under WSL it first hands the target to
 * Windows through `powershell.exe` (in OPEN_RS_TARGET), which would open
 * the Windows browser: that one is replaced too.
 *
 * With --desktop, xdg-open records and then runs the real one (which
 * starts the desktop's file manager or browser), with its environment
 * written to `openerEnvironmentFile`; `powershell.exe` records and fails,
 * so the crate goes on to xdg-open as on a Linux computer. */
function fakeOpener() {
  const bin = join(temp, "bin");
  mkdirSync(bin);
  const record = (name, what) =>
    `#!/bin/sh\nprintf '%s %s\\n' ${name} "${what}" >> ${quoted(openedFile)}\n`;
  for (const name of ["xdg-open", "gio", "gnome-open", "kde-open"])
    writeFileSync(join(bin, name), record(name, "$*"));
  writeFileSync(
    join(bin, "powershell.exe"),
    record("windows", "$OPEN_RS_TARGET") + (desktop ? "exit 1\n" : ""),
  );
  if (desktop)
    writeFileSync(
      join(bin, "xdg-open"),
      record("xdg-open", "$*") +
        `{ echo "--- $*"; env; } >> ${quoted(openerEnvironmentFile)}\n` +
        `PATH=$(printf '%s' "$PATH" | sed "s#^${bin}:##")\n` +
        // What WebDriver sets in the app only: a browser that is itself
        // WebKit (GNOME Web) would take it for its own and fail to start.
        // And --gallium-driver's, which is for the app (GNOME Web shows
        // blank pages with Mesa's D3D12 driver).
        `unset WEBKIT_INSPECTOR_SERVER TAURI_AUTOMATION TAURI_WEBVIEW_AUTOMATION${options["gallium-driver"] ? " GALLIUM_DRIVER" : ""}\n` +
        `exec /usr/bin/xdg-open "$@" 2>> ${quoted(`${openerEnvironmentFile}.errors`)}\n`,
    );
  for (const name of [
    "xdg-open",
    "gio",
    "gnome-open",
    "kde-open",
    "powershell.exe",
  ])
    chmodSync(join(bin, name), 0o755);
  return bin;
}

function startProcess(command, args, env = process.env, ready = null) {
  const child = spawn(command, args, {
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  children.push(child);
  let output = "";
  const collect = (chunk) => (output += chunk);
  child.stdout.on("data", collect);
  child.stderr.on("data", collect);
  if (!ready) return child;
  return until(
    `${command} to start`,
    () => {
      if (child.exitCode !== null)
        fail(`${command} exited: ${output.trim().slice(-400)}`);
      return ready(output) && child;
    },
    120000,
  );
}

async function startServers() {
  const build = spawnSync(
    "cargo",
    ["build", "-p", "netterm-core", "--example", "test_ssh_server"],
    { stdio: "inherit", cwd: ROOT },
  );
  if (build.status !== 0) fail("could not build the example SSH server");
  await startProcess(
    join(ROOT, "target/debug/examples/test_ssh_server"),
    [String(SSH_PORT), "--telnet", String(TELNET_PORT)],
    process.env,
    (output) => output.includes("telnet device listening"),
  );
  // The test key, which the example server accepts, in an agent of our own.
  mkdirSync(join(temp, "keys"));
  copyFileSync(
    join(ROOT, "crates/core/tests/fixtures/keys/rsa_client"),
    keyFile,
  );
  chmodSync(keyFile, 0o600);
  await startProcess("ssh-agent", ["-D", "-a", agentSocket], process.env, () =>
    existsSync(agentSocket),
  );
  const added = spawnSync("ssh-add", [keyFile], {
    env: { ...process.env, SSH_AUTH_SOCK: agentSocket },
    encoding: "utf8",
  });
  if (added.status !== 0) fail(`ssh-add: ${added.stderr}`);
  // The update check's manifest, and the pages a browser opens (each
  // visit recorded with the browser's name for itself).
  const release = { requests: 0, visits: [] };
  const manifest = JSON.stringify({
    version: NEWER_VERSION,
    notes: "A newer release.",
    pub_date: "2026-11-01T10:00:00Z",
    page: RELEASE_PAGE,
    platforms: {},
  });
  const server = createServer((request, response) => {
    if (request.url === "/latest.json") {
      release.requests += 1;
      response.writeHead(200, { "Content-Type": "application/json" });
      response.end(manifest);
      return;
    }
    release.visits.push({
      path: request.url,
      agent: request.headers["user-agent"] ?? "",
    });
    response.writeHead(200, { "Content-Type": "text/html" });
    response.end(
      `<!doctype html><title>netterm check ${request.url}</title><h1>${request.url}</h1>`,
    );
  });
  await new Promise((resolve) =>
    server.listen(RELEASE_PORT, "127.0.0.1", resolve),
  );
  release.close = () => server.close();
  return release;
}

/** The app's environment: temporary data and XDG folders, the agent, the
 * release server, the recording opener first on the PATH. With --desktop
 * the XDG folders are the desktop session's (temporary too), where its
 * settings and default applications are. */
function appEnvironment(bin, extra = {}) {
  const env = {
    ...process.env,
    NETTERM_DATA_DIR: dataFolder,
    ...(!desktop && {
      XDG_DATA_HOME: join(temp, "xdg-data"),
      XDG_CACHE_HOME: join(temp, "xdg-cache"),
      XDG_CONFIG_HOME: join(temp, "xdg-config"),
    }),
    SSH_AUTH_SOCK: agentSocket,
    NETTERM_UPDATE_ENDPOINT: `http://127.0.0.1:${RELEASE_PORT}/latest.json`,
    PATH: `${bin}:${process.env.PATH}`,
    ...extra,
  };
  if (options.backend) env.GDK_BACKEND = options.backend;
  if (options["gallium-driver"]) env.GALLIUM_DRIVER = options["gallium-driver"];
  return env;
}

// --- WebDriver ---------------------------------------------------------

const ELEMENT = "element-6066-11e4-a52e-4f735466cecf";
// WebDriver's codes for these keys.
const CTRL = String.fromCodePoint(0xe009);
const SHIFT = String.fromCodePoint(0xe008);
const ESCAPE = String.fromCodePoint(0xe00c);
const ENTER = String.fromCodePoint(0xe007);
const PAGE_UP = String.fromCodePoint(0xe00e);

/** Starts tauri-driver and the app under it; resolves to a page object. */
async function startApp(env) {
  const driver = await startProcess(
    "tauri-driver",
    ["--port", String(DRIVER_PORT), "--native-port", String(NATIVE_PORT)],
    env,
  );
  const base = `http://127.0.0.1:${DRIVER_PORT}`;
  const call = async (method, path, body) => {
    const response = await fetch(base + path, {
      method,
      headers: { "content-type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      // A session the app never connects to would wait for ever.
      signal: AbortSignal.timeout(120000),
    });
    const answer = await response.json();
    if (!response.ok)
      throw new Error(
        `${method} ${path}: ${answer.value?.error}: ${answer.value?.message}`,
      );
    return answer.value;
  };
  const session = await until("tauri-driver and the app", () => {
    // Another driver on the port would answer in its place.
    if (driver.exitCode !== null)
      fail(
        `tauri-driver exited (is port ${DRIVER_PORT} or ${NATIVE_PORT} taken?)`,
      );
    return call("POST", "/session", {
      capabilities: {
        alwaysMatch: { "tauri:options": { application: options.app } },
      },
    });
  });
  const at = (path) => `/session/${session.sessionId}${path}`;
  const page = {
    driver,
    /** Runs `body` (a function body; `args` as `arguments`) in the page. */
    js: (body, ...args) =>
      call("POST", at("/execute/sync"), { script: body, args }),
    /** Runs `body`, which ends by calling `done(value)`. */
    jsAsync: (body, ...args) =>
      call("POST", at("/execute/async"), {
        script: `const done = arguments[arguments.length - 1];\n${body}`,
        args,
      }),
    url: () => call("GET", at("/url")),
    navigate: (url) => call("POST", at("/url"), { url }),
    exists: (selector) =>
      page.js("return !!document.querySelector(arguments[0])", selector),
    text: (selector) =>
      page.js(
        "return document.querySelector(arguments[0])?.textContent ?? ''",
        selector,
      ),
    find: async (selector) =>
      (
        await call("POST", at("/element"), {
          using: "css selector",
          value: selector,
        })
      )[ELEMENT],
    /** A WebDriver click: refused if the control is covered. */
    click: async (selector) => {
      const element = await until(`${selector} to click`, () =>
        page.find(selector),
      );
      await call("POST", at(`/element/${element}/click`), {});
    },
    /** Types into a field (WebDriver's key events). */
    type: async (selector, text) => {
      const element = await until(`${selector} to type in`, () =>
        page.find(selector),
      );
      await call("POST", at(`/element/${element}/clear`), {});
      await call("POST", at(`/element/${element}/value`), { text });
    },
    /** Pointer actions at an element (offset from its centre). */
    pointer: async (selector, steps) => {
      const element = await page.find(selector);
      const origin = { [ELEMENT]: element };
      await call("POST", at("/actions"), {
        actions: [
          {
            type: "pointer",
            id: "mouse",
            parameters: { pointerType: "mouse" },
            actions: steps(origin),
          },
        ],
      });
      await call("DELETE", at("/actions"));
    },
    doubleClick: (selector) =>
      page.pointer(selector, (origin) => [
        { type: "pointerMove", origin, x: 0, y: 0 },
        { type: "pointerDown", button: 0 },
        { type: "pointerUp", button: 0 },
        { type: "pointerDown", button: 0 },
        { type: "pointerUp", button: 0 },
      ]),
    /** Key actions to the focused element; `keys` as WebDriver codes
     * (CTRL, SHIFT, ... or characters); chords hold the earlier keys. */
    keys: async (...keys) => {
      const actions = [];
      for (const key of keys) actions.push({ type: "keyDown", value: key });
      for (const key of [...keys].reverse())
        actions.push({ type: "keyUp", value: key });
      await call("POST", at("/actions"), {
        actions: [{ type: "key", id: "keyboard", actions }],
      });
      await call("DELETE", at("/actions"));
    },
    typeKeys: async (text) => {
      const actions = [];
      for (const char of text)
        actions.push(
          { type: "keyDown", value: char === "\r" ? ENTER : char },
          { type: "keyUp", value: char === "\r" ? ENTER : char },
        );
      await call("POST", at("/actions"), {
        actions: [{ type: "key", id: "keyboard", actions }],
      });
      await call("DELETE", at("/actions"));
    },
    picture: async (name) => {
      mkdirSync(pictures, { recursive: true });
      const png = await call("GET", at("/screenshot"));
      writeFileSync(join(pictures, `${name}.png`), Buffer.from(png, "base64"));
      // In a desktop session, the whole screen.
      if (desktop) desktop.picture(join(pictures, `${name}-desktop.png`));
      // Under WSLg's X11 also the window as the X server has it
      // (ImageMagick's import; WSLg's XWayland cannot capture its root).
      else if ((options.backend ?? process.env.GDK_BACKEND) === "x11") {
        const title = await page.js("return document.title");
        const found = spawnSync(
          "xdotool",
          ["search", "--onlyvisible", "--name", `^${title}$`],
          { encoding: "utf8" },
        );
        const window = found.stdout?.trim().split("\n").at(-1);
        if (window)
          spawnSync("import", [
            "-window",
            window,
            join(pictures, `${name}-x11.png`),
          ]);
      }
    },
    /** A window command of Tauri's window plugin, on this window. */
    window: (command, extra = {}) =>
      page.jsAsync(
        `const internals = window.__TAURI_INTERNALS__;
         internals.invoke("plugin:window|" + arguments[0],
           { label: internals.metadata.currentWindow.label, ...arguments[1] })
           .then((value) => done({ value }), (error) => done({ error: String(error) }));`,
        command,
        extra,
      ),
    /** The app's processes: below WebKitWebDriver, tauri-driver's child. */
    app: () =>
      childProcesses(driver.pid).flatMap((native) =>
        descendants(native).slice(1),
      ),
    /** Ends the session, the app (WebDriver leaves it running) and the
     * driver. */
    quit: async () => {
      await call("DELETE", at("")).catch(() => {});
      const left = descendants(driver.pid);
      for (const pid of left.reverse()) {
        try {
          process.kill(pid);
        } catch {
          // Gone already.
        }
      }
      await until(
        "tauri-driver to stop",
        () => driver.exitCode !== null || driver.signalCode,
      );
    },
  };
  return page;
}

/** The screen of the session in `slot` (its whole buffer), read from xterm. */
function screen(page, slot) {
  return page.js(
    `const terminal = document.querySelector(arguments[0] + " .terminal")?.xterm;
     if (!terminal) return "";
     const buffer = terminal.buffer.active;
     let text = "";
     for (let y = 0; y < buffer.length; y++)
       text += buffer.getLine(y).translateToString(true) + "\\n";
     return text;`,
    slot,
  );
}

async function showSideView(page, view) {
  if (!(await page.exists(`[data-side-view=${view}]`)))
    await page.click(`[data-rail=${view}]`);
  await until(`the ${view} view`, () =>
    page.exists(`[data-side-view=${view}]`),
  );
}

async function openPreferences(page) {
  await page.click("[data-open-preferences]");
  await until("Preferences", () =>
    page.js(
      "const v = document.querySelector('[data-view=preferences]'); return !!v && !v.closest('[hidden]');",
    ),
  );
}

/** A tool of the active pane's bar: its button, or in a narrow pane
 * (#100) the entry of the same name in the bar's "…" menu. */
async function paneTool(page, selector, label) {
  const shown = await page.js(
    `const bars = [...document.querySelectorAll("[data-tab-bar]")];
     const active = bars.find((bar) => bar.closest(".pane")?.querySelector(".slot[data-active]")) ?? bars[0];
     for (const bar of bars) delete bar.dataset.checkBar;
     active.dataset.checkBar = "";
     const button = active.querySelector(arguments[0]);
     return !!button && button.checkVisibility() && !button.closest("[inert]");`,
    selector,
  );
  if (shown) return page.click(`[data-check-bar] ${selector}`);
  await page.click("[data-check-bar] [data-pane-tools-more]");
  await until(`"${label}" in the pane's menu`, () =>
    page.js(
      `const entry = [...document.querySelectorAll("[role=menuitem]")].find((item) => item.textContent.trim() === arguments[0]);
       if (entry) entry.dataset.checkEntry = "";
       return !!entry;`,
      label,
    ),
  );
  await page.click("[data-check-entry]");
}

async function closeTab(page, selector) {
  if (await page.exists(`${selector} [data-tab-close]`))
    await page.click(`${selector} [data-tab-close]`);
}

// --- The points of docs/linux.md ----------------------------------------

/** 1. The package's menu entry and icon; the app starts and shows its
 * window. */
async function started(page) {
  if (options.app === "/usr/bin/netterm") {
    const entry = readFileSync(
      "/usr/share/applications/netterm.desktop",
      "utf8",
    );
    const field = (name) => entry.match(new RegExp(`^${name}=(.*)$`, "m"))?.[1];
    if (field("Exec") !== "netterm")
      fail(`the menu entry's Exec: ${field("Exec")}`);
    const icon = field("Icon");
    for (const size of ["32x32", "128x128"])
      if (!existsSync(`/usr/share/icons/hicolor/${size}/apps/${icon}.png`))
        fail(`no ${size} icon ${icon}`);
    if (!field("Categories"))
      problem("the menu entry has no category (Categories= is empty)");
    step(
      `the menu entry runs netterm with its icon (32 and 128 px), in ${field("Categories")}`,
    );
  }
  await until("the welcome view", () =>
    page.exists("[data-welcome] [data-welcome-form=open]"),
  );
  const info = await page.js(`
    const canvas = document.createElement("canvas");
    return {
      title: document.title,
      size: [innerWidth, innerHeight],
      webgl2: !!canvas.getContext("webgl2"),
    };`);
  // The renderer behind the web view's GL: WebKit hides it from pages
  // (WebGL names an "Apple GPU" whatever draws), so it is read from
  // WebKitGTK's own webkit://gpu page, then the app is loaded again.
  const appUrl = await page.url();
  await page.navigate("webkit://gpu");
  const gpu = await until("webkit://gpu", async () => {
    const text = await page.js("return document.body?.innerText ?? ''");
    return text.includes("Renderer") && text;
  }).catch(() => "");
  mkdirSync(pictures, { recursive: true });
  writeFileSync(join(pictures, "1-webkit-gpu.txt"), gpu);
  await page.navigate(appUrl);
  await until("the welcome view again", () =>
    page.exists("[data-welcome] [data-welcome-form=open]"),
  );
  // The page is a table: each name on a line, its value on the next.
  const lines = gpu.split("\n").map((line) => line.trim());
  const field = (name) => {
    const at = lines.indexOf(name);
    return at >= 0 ? lines[at + 1] : undefined;
  };
  const renderer = field("GL_RENDERER") ?? "unknown";
  step(
    `the app started (${options.app}, GDK_BACKEND ${options.backend ?? process.env.GDK_BACKEND ?? "unset"}` +
      `${desktop ? `, in ${desktop.name}` : ""}): ` +
      `window "${info.title}" ${info.size.join("x")}, WebGL 2 ${info.webgl2 ? "available" : "NOT available"}; ` +
      `webkit://gpu: GL_RENDERER "${renderer}", acceleration ${field("Policy")}, ` +
      `WebGL ${field("WebGL enabled")}, buffers ${field("Renderer")}, ${field("Platform")}`,
  );
  const driver = options["gallium-driver"];
  if (driver && !renderer.toLowerCase().includes(driver))
    problem(`with GALLIUM_DRIVER=${driver} the renderer is "${renderer}"`);
  await page.picture("1-welcome");
}

// --- A desktop session (--desktop) ----------------------------------------

const APP_CLASSES = ["netterm", "Netterm"];

/** The app's windows as the desktop's window manager has them. */
function appWindows() {
  return desktop.windows(APP_CLASSES);
}

/** The one window of the app, once the window manager has it. */
function appWindow() {
  return until("the app's window at the window manager", () => {
    const found = appWindows();
    return found.length === 1 && found[0];
  });
}

/** The window once two reads a moment apart agree. */
async function settledWindow() {
  let last = await appWindow();
  for (let tries = 0; tries < 30; tries++) {
    await sleep(250);
    const now = await appWindow();
    if (JSON.stringify(now) === JSON.stringify(last)) return now;
    last = now;
  }
  return last;
}

const near = (value, expected, slack = 3) =>
  Math.abs(value - expected) <= slack;
const box = (window) =>
  `${window.width}x${window.height} at ${window.x},${window.y}`;

/** 1. The menu entry: valid, its icon at every size a menu asks, found by
 * the desktop's menu search and started from it (before the check's own
 * copy of the app). The package's only. */
async function desktopMenu() {
  const validated = spawnSync(
    "desktop-file-validate",
    ["/usr/share/applications/netterm.desktop"],
    { encoding: "utf8" },
  );
  if (validated.error)
    fail("desktop-file-validate is needed (desktop-file-utils)");
  if (validated.status !== 0 || validated.stdout.trim())
    problem(
      `desktop-file-validate: ${validated.stdout}${validated.stderr}`.trim(),
    );
  else step("desktop-file-validate accepts the menu entry");

  // What GTK's icon theme (and so the menu and the task list) finds for
  // each size, through the hicolor theme and its cache.
  const icons = spawnSync(
    "python3",
    [
      "-c",
      [
        "import gi, sys",
        "gi.require_version('Gtk', '3.0')",
        "from gi.repository import Gtk",
        "theme = Gtk.IconTheme.get_default()",
        "for size in (16, 24, 32, 48, 64, 128, 256):",
        "    info = theme.lookup_icon('netterm', size, 0)",
        "    print(size, info.get_filename() if info else None, info.get_base_size() if info else 0)",
      ].join("\n"),
    ],
    { env: desktop.env, encoding: "utf8" },
  );
  const found = icons.stdout.trim().split("\n");
  if (icons.status !== 0 || found.some((line) => line.includes("None")))
    problem(`the icon theme lacks the icon: ${icons.stdout}${icons.stderr}`);
  else
    step(
      `the icon theme finds the icon at every size: ${found.map((line) => line.replace(/ \/usr\/share\/icons\/hicolor\/(\S+)\/apps\/netterm.png/, " $1")).join(", ")}`,
    );
  const cache = "/usr/share/icons/hicolor/icon-theme.cache";
  if (!existsSync(cache) || !readFileSync(cache).includes("netterm"))
    problem("the hicolor icon cache does not list netterm");

  if (desktop.menuEntry) {
    const entry = desktop.menuEntry("netterm.desktop");
    if (!entry?.shown) problem(`GNOME's app list: ${JSON.stringify(entry)}`);
    else step(`GNOME's app list has it: ${JSON.stringify(entry)}`);
  }

  mkdirSync(pictures, { recursive: true });
  // A second try if the first search found nothing to start (a menu
  // still loading on a busy computer).
  let window = null;
  for (let attempt = 1; !window && attempt <= 2; attempt++) {
    await desktop.searchMenu("netterm");
    desktop.picture(join(pictures, "0-menu-search-desktop.png"));
    await desktop.launchMenuHit();
    window = await settles(
      () => appWindows().find((window) => window.width > 0),
      30000,
    ).then((started) =>
      started ? appWindows().find((window) => window.width > 0) : null,
    );
    if (!window) await desktop.key("Escape");
  }
  if (!window) fail("the menu's search did not start the app");
  await sleep(2000);
  desktop.picture(join(pictures, "0-menu-started-desktop.png"));
  const startedBy = readFileSync(`/proc/${window.pid}/cmdline`, "utf8")
    .split("\0")
    .filter(Boolean)
    .join(" ");
  if (window.app !== undefined && window.app !== "netterm.desktop")
    problem(
      `GNOME ties the window to ${window.app}, not netterm.desktop (dock icon, StartupWMClass)`,
    );
  step(
    `the menu's search found netterm and started it ("${startedBy}"): window "${window.title}" ${box(window)}` +
      (window.app ? `, tied to ${window.app}` : ""),
  );
  desktop.close(window);
  const gone = await settles(() => appWindows().length === 0, 10000);
  if (!gone) {
    problem("the app started from the menu did not close");
    try {
      process.kill(window.pid);
    } catch {
      // Gone already.
    }
  }
}

/** 2. Light and dark from the desktop's own switch, with the app's theme
 * on "system", while it runs. The AppImage pins its GTK theme at launch
 * (its GTK hook, and netterm's for the style: #143), so where the desktop's
 * dark look is a style (XFCE) a switch while it runs is not expected to
 * reach it: only what it starts with is (light here, dark in `restart`). */
async function desktopTheme(page) {
  if (portable && desktop.darkBy === "style") return pinnedTheme(page);
  const shown = () =>
    page.js(`return {
      dark: matchMedia("(prefers-color-scheme: dark)").matches,
      applied: document.documentElement.dataset.theme ?? null,
      background: getComputedStyle(document.body).backgroundColor,
    }`);
  const frameTheme = async () => (await page.window("theme")).value ?? null;
  // Whether the page is told (the media query's change event).
  await page.js(`window.__checkChanges = [];
    matchMedia("(prefers-color-scheme: dark)").addEventListener("change",
      (event) => window.__checkChanges.push(event.matches));`);
  const before = await shown();
  if (before.dark)
    problem(`the desktop is light, the app dark: ${JSON.stringify(before)}`);
  const switched = desktop.setDark(true);
  const dark = await settles(async () => {
    const now = await shown();
    return now.dark && now.applied === "dark";
  }, 10000);
  const darkShown = {
    ...(await shown()),
    changeEvents: await page.js("return window.__checkChanges"),
  };
  const darkFrame = await frameTheme();
  await page.picture("2-desktop-dark");
  if (dark)
    step(
      `the desktop switched to dark (${switched}): the running app turned dark (${JSON.stringify(darkShown)}, the window's theme ${darkFrame})`,
    );
  else
    problem(
      `the desktop switched to dark (${switched}), the running app stayed light (${JSON.stringify(darkShown)}, the window's theme ${darkFrame})`,
    );
  const back = desktop.setDark(false);
  const light = await settles(async () => {
    const now = await shown();
    return !now.dark && now.applied === "light";
  }, 10000);
  await page.picture("2-desktop-light");
  if (light) step(`and back to light (${back}): the app turned light`);
  else problem(`the desktop back to light (${back}), the app stayed dark`);
}

/** 2, for an AppImage on a desktop whose dark look is a style: light at
 * launch with the desktop light; a switch while it runs is only noted (it
 * needs a restart: docs/linux.md). */
async function pinnedTheme(page) {
  const shown = () =>
    page.js(`return {
      dark: matchMedia("(prefers-color-scheme: dark)").matches,
      applied: document.documentElement.dataset.theme ?? null,
    }`);
  const before = await shown();
  if (before.dark || before.applied !== "light")
    problem(
      `the desktop is light, the AppImage started dark: ${JSON.stringify(before)}`,
    );
  else
    step(
      `the desktop is light (its style): the AppImage started light (${JSON.stringify(before)})`,
    );
  const switched = desktop.setDark(true);
  const followed = await settles(async () => (await shown()).dark, 5000);
  await page.picture("2-desktop-dark-running");
  note(
    `the desktop switched to dark while the AppImage runs (${switched}): it ${followed ? "turned dark" : "stayed light"}; ` +
      "the AppImage reads the style at launch only (#143)",
  );
  desktop.setDark(false);
  if (followed && !(await settles(async () => !(await shown()).dark, 10000)))
    problem("the desktop back to light, the AppImage stayed dark");
}

/** The screen point of a point of the page: the page's origin is found
 * by moving the desktop's pointer over the window and reading where the
 * page saw it. */
async function pageOrigin(page) {
  await desktop.calm?.();
  await page.js(`window.__checkPointer = null;
    addEventListener("mousemove", (event) => (window.__checkPointer = [event.clientX, event.clientY]));`);
  const window = await settledWindow();
  // Moved a little at a time until the page sees it (the first motion
  // into a window may only enter it).
  for (let step = 0; step < 20; step++) {
    const at = { x: window.x + 300 + step, y: window.y + 300 + step };
    await desktop.to(at.x, at.y);
    await sleep(100);
    const seen = await page.js("return window.__checkPointer");
    if (seen && seen[0] > 250) return { x: at.x - seen[0], y: at.y - seen[1] };
  }
  fail(
    `the page did not see the desktop's pointer over the window (${box(window)})`,
  );
}

/** A drag with the desktop's pointer, in steps, as a hand does. */
async function drag(from, to, { hold = 0 } = {}) {
  await desktop.to(from.x, from.y);
  await sleep(150);
  await desktop.down();
  await sleep(200);
  for (const part of [0.1, 0.3, 0.6, 0.85, 1]) {
    await desktop.to(
      from.x + (to.x - from.x) * part,
      from.y + (to.y - from.y) * part,
    );
    await sleep(80);
  }
  // Held against the target a moment, pushing (a window manager tiles
  // on the pointer pressing the screen edge).
  for (let push = 0; hold && push < 6; push++) {
    await desktop.to(to.x + (push % 2), to.y + push);
    await sleep(100);
  }
  await sleep(200 + hold);
  await desktop.up();
  await sleep(300);
}

async function realClick(point, times = 1) {
  await desktop.to(point.x, point.y);
  await sleep(100);
  for (let count = 0; count < times; count++) {
    await desktop.down();
    await desktop.up();
  }
}

/** 9. The frame with the desktop's own pointer: move, resize from edges
 * and corners, tiling at the screen's edge, a double click on the top
 * bar; the window manager says where the window is. */
async function desktopFrame(page) {
  const start = await settledWindow();
  const origin = await pageOrigin(page);
  note(
    `the window at the window manager: ${box(start)}; the page's origin on the screen ${origin.x},${origin.y}`,
  );
  // The top bar's empty part (a drag region), on the screen now.
  const barOnScreen = async () => {
    const at = await pageOrigin(page);
    const bar = await page.js(`
      const bar = document.querySelector("[data-tab-bar]");
      const box = bar.getBoundingClientRect();
      const y = Math.round(box.top + box.height / 2);
      for (let x = Math.round(box.right) - 8; x > box.left; x -= 4)
        if (document.elementFromPoint(x, y) === bar) return { x, y };
      return null;`);
    if (!bar) fail("no empty part in the top bar");
    return { x: at.x + bar.x, y: at.y + bar.y };
  };
  const pageSize = () => page.js("return [innerWidth, innerHeight]");
  const results = [];
  const expect = async (what, expected) => {
    const now = await settledWindow();
    const wrong = Object.entries(expected).filter(
      ([key, value]) => !near(now[key], value),
    );
    if (wrong.length)
      problem(
        `${what}: the window is ${box(now)}, expected ${JSON.stringify(expected)}`,
      );
    else results.push(what);
    return now;
  };

  // Move: the top bar's empty part dragged by (+140, +90).
  let before = start;
  let from = await barOnScreen();
  await drag(from, { x: from.x + 140, y: from.y + 90 });
  await expect("moved by dragging the top bar", {
    x: before.x + 140,
    y: before.y + 90,
    width: before.width,
    height: before.height,
  });
  await page.picture("9-moved");

  // Resizing from each edge and corner, 2 px inside it.
  const edges = [
    [
      "the right edge",
      (w) => ({ x: w.x + w.width - 2, y: w.y + w.height / 2 }),
      [100, 0],
      (w) => ({ width: w.width + 100 }),
    ],
    [
      "the bottom edge",
      (w) => ({ x: w.x + w.width / 2, y: w.y + w.height - 2 }),
      // Upwards: below, the window may already reach the screen's foot.
      [0, -60],
      (w) => ({ height: w.height - 60 }),
    ],
    [
      "the bottom-right corner",
      (w) => ({ x: w.x + w.width - 2, y: w.y + w.height - 2 }),
      [-120, -80],
      (w) => ({ width: w.width - 120, height: w.height - 80 }),
    ],
    [
      "the left edge",
      (w) => ({ x: w.x + 2, y: w.y + w.height / 2 }),
      [-70, 0],
      (w) => ({ x: w.x - 70, width: w.width + 70 }),
    ],
    [
      "the top edge",
      (w) => ({ x: w.x + w.width / 2, y: w.y + 2 }),
      [0, -40],
      (w) => ({ y: w.y - 40, height: w.height + 40 }),
    ],
    [
      "the top-left corner",
      (w) => ({ x: w.x + 2, y: w.y + 2 }),
      [50, 30],
      (w) => ({
        x: w.x + 50,
        y: w.y + 30,
        width: w.width - 50,
        height: w.height - 30,
      }),
    ],
  ];
  for (const [name, grip, [dx, dy], expected] of edges) {
    before = await settledWindow();
    const pageBefore = await pageSize();
    from = grip(before);
    await drag(from, { x: from.x + dx, y: from.y + dy });
    const now = await expect(`resized from ${name}`, {
      x: before.x,
      y: before.y,
      width: before.width,
      height: before.height,
      ...expected(before),
    });
    const followed = await settles(async () => {
      const size = await pageSize();
      return (
        near(size[0] - pageBefore[0], now.width - before.width) &&
        near(size[1] - pageBefore[1], now.height - before.height)
      );
    });
    if (!followed)
      problem(
        `resized from ${name}: the page is ${(await pageSize()).join("x")}, was ${pageBefore.join("x")}, for a window ${box(now)} (was ${box(before)})`,
      );
  }
  await page.picture("9-resized");

  // Tiling to the left half: by dragging the top bar to the screen's left
  // edge, then with the desktop's key; each time dragged off the edge
  // again, the window gets its size back.
  const area = desktop.workArea();
  const tiledLeft = (window) =>
    near(window.x, area.x, 4) &&
    near(window.width, area.width / 2, 8) &&
    window.height >= area.height - 40;
  for (const [how, perform] of [
    [
      "by dragging it to the screen's left edge",
      async () =>
        drag(
          await barOnScreen(),
          { x: 0, y: area.y + area.height / 2 },
          { hold: 800 },
        ),
    ],
    [
      `with the desktop's key (${desktop.tileLeftKey})`,
      async () => {
        await realClick(await barOnScreen());
        await desktop.key(desktop.tileLeftKey);
        await sleep(800);
      },
    ],
  ]) {
    before = await settledWindow();
    await perform();
    let now = await settledWindow();
    await page.picture(`9-tiled-left-${how.startsWith("by") ? "drag" : "key"}`);
    if (!tiledLeft(now)) {
      problem(
        `tiling ${how}: the window is ${box(now)} (the work area ${area.width}x${area.height} at ${area.x},${area.y})`,
      );
      // Back where the next steps can reach it.
      desktop.place(now, 150, 120);
      await sleep(500);
      continue;
    }
    results.push(`tiled to the left half ${how} (${box(now)})`);
    await drag(await barOnScreen(), {
      x: SCREEN.width / 2,
      y: SCREEN.height / 3,
    });
    now = await settledWindow();
    if (near(now.width, before.width, 4) && near(now.height, before.height, 4))
      results.push(`dragged off the edge, back to ${now.width}x${now.height}`);
    else
      problem(
        `dragged off the edge after tiling ${how}, the window is ${box(now)}, was ${box(before)}`,
      );
  }

  // A double click on the top bar with the real pointer.
  before = await settledWindow();
  await realClick(await barOnScreen(), 2);
  const maximized = await settles(
    async () => (await appWindow()).maximized,
    5000,
  );
  await page.picture("9-maximized-by-double-click");
  if (maximized) {
    let now = await settledWindow();
    results.push(`a double click on the top bar maximized it (${box(now)})`);
    await realClick(await barOnScreen(), 2);
    const restored = await settles(
      async () => !(await appWindow()).maximized,
      5000,
    );
    now = await settledWindow();
    if (restored && near(now.width, before.width, 4))
      results.push(`a second one restored it (${box(now)})`);
    else problem(`a second double click left the window ${box(now)}`);
  } else
    problem(
      "a double click on the top bar's empty part with the desktop's pointer did not maximize the window",
    );
  step(
    `the frame with the desktop's pointer (${desktop.name}): ${results.join("; ")}`,
  );
  // Back to where and how big it started, for the checks after this.
  desktop.place(await appWindow(), start.x, start.y, start.width, start.height);
  await settledWindow();
}

/** 9 (end): Minimize with the desktop's pointer; the window manager says
 * it is minimized, and the desktop brings it back. */
async function desktopMinimize(page) {
  const origin = await pageOrigin(page);
  const button = await page.js(`
    const box = document.querySelector("[data-window-control=minimize]").getBoundingClientRect();
    return { x: Math.round(box.left + box.width / 2), y: Math.round(box.top + box.height / 2) };`);
  await realClick({ x: origin.x + button.x, y: origin.y + button.y });
  const minimized = await settles(
    async () => (await appWindow()).minimized,
    5000,
  );
  const told = (await page.window("is_minimized")).value;
  desktop.picture(join(pictures, "9-minimized-desktop.png"));
  if (!minimized) {
    problem(
      `Minimize clicked with the desktop's pointer: the window manager does not have it minimized (the app says is_minimized ${told})`,
    );
    return;
  }
  desktop.activate(await appWindow());
  const back = await settles(async () => !(await appWindow()).minimized, 5000);
  const answers = await page.js("return document.title").catch(() => null);
  await page.picture("9-restored");
  if (back && answers)
    step(
      `Minimize (the desktop's pointer) minimized the window (the window manager says so; the app's is_minimized ${told}); the desktop brought it back and it draws`,
    );
  else
    problem(
      `after Minimize the window did not come back (${back}, ${answers})`,
    );
}

/** 8. With a desktop: the file manager and the browser really started,
 * and what they were given. */
async function desktopOpener(release) {
  const fileManager = await until(
    "the file manager's window",
    () => desktop.windows(FILE_MANAGER.classes)[0],
    30000,
  ).catch(() => null);
  const browser = await until(
    "the browser's window",
    () => desktop.windows(BROWSER.classes)[0],
    60000,
  ).catch(() => null);
  const visited = await until(
    "the browser to load the pages",
    () =>
      release.visits.some((visit) => visit.path === "/status") &&
      release.visits.some(
        (visit) => visit.path === new URL(RELEASE_PAGE).pathname,
      ),
    60000,
  ).catch(() => false);
  await sleep(1500);
  desktop.picture(join(pictures, "8-opened-desktop.png"));
  const logFolder = basename(
    readFileSync(openedFile, "utf8")
      .split("\n")
      .find((line) => line.startsWith("xdg-open ") && line.includes("/logs/"))
      ?.slice("xdg-open ".length) ?? "",
  );
  if (fileManager && fileManager.title.includes(logFolder))
    step(
      `"Open log folder" started the file manager on it: window "${fileManager.title}"`,
    );
  else
    problem(
      `"Open log folder": the file manager ${fileManager ? `shows "${fileManager.title}", not ${logFolder}` : "did not start"}`,
    );
  if (browser && visited)
    step(
      `the web link and the release page opened in the browser ("${browser.title}"), which loaded them: ${JSON.stringify(release.visits.map((visit) => visit.path))} as "${release.visits[0].agent}"`,
    );
  else
    problem(
      `the browser ${browser ? `("${browser.title}") ` : "did not start, and "}loaded ${JSON.stringify(release.visits)}`,
    );

  // What the started programs, and the opener, got from the app: nothing
  // that points into an AppImage's mount (its libraries, its Python, its
  // GTK modules) may reach them.
  const bundled = (name, value) =>
    /^(APPDIR|APPIMAGE|ARGV0|OWD|APPIMAGE_GTK_THEME)$/.test(name) ||
    value.includes("/tmp/.mount_");
  const leaks = (pairs) => [
    ...new Set(
      pairs
        .filter(([name, value]) => bundled(name, value))
        .map(([name]) => name),
    ),
  ];
  const opener = existsSync(openerEnvironmentFile)
    ? readFileSync(openerEnvironmentFile, "utf8")
        .split("\n")
        .map((line) => line.split(/=(.*)/s).slice(0, 2))
        .filter((pair) => pair.length === 2)
    : [];
  const report = [`xdg-open: ${leaks(opener).join(", ") || "clean"}`];
  let leaked = leaks(opener).length > 0;
  for (const [name, window] of [
    ["file manager", fileManager],
    ["browser", browser],
  ]) {
    if (!window) continue;
    // A program in a container (test:linux-fedora) has a process id of
    // the container's, not readable here.
    if (!existsSync(`/proc/${window.pid}/environ`)) {
      report.push(`${name}: in another process namespace, not read`);
      continue;
    }
    const environment = readFileSync(`/proc/${window.pid}/environ`, "utf8")
      .split("\0")
      .map((line) => line.split(/=(.*)/s).slice(0, 2))
      .filter((pair) => pair.length === 2);
    const found = leaks(environment);
    // Nor may it stand in the bundle's mount.
    const folder = readlinkSync(`/proc/${window.pid}/cwd`);
    if (folder.includes("/tmp/.mount_")) found.push(`its folder ${folder}`);
    leaked ||= found.length > 0;
    report.push(`${name} (pid ${window.pid}): ${found.join(", ") || "clean"}`);
  }
  if (leaked)
    problem(
      `the AppImage's environment reached the opened programs: ${report.join("; ")}`,
    );
  else
    step(
      `what the opened programs got from the app points nowhere into the app's bundle: ${report.join("; ")}`,
    );
  for (const window of [fileManager, browser])
    if (window) desktop.minimize(window);
}

/** 2. A workspace opened by its folder; the tree, the details sheet, the
 * palette and Preferences. */
async function workspaceAndViews(page) {
  await page.type("[data-welcome-form=open] [name=welcome-folder]", workspace);
  await page.click("[data-welcome-form=open] button[type=submit]");
  await until(
    "the welcome view gone",
    async () => !(await page.exists("[data-welcome]")),
  );
  await until("the Lab folder", () =>
    page.exists('nav [data-item=folder][data-path="Lab"]'),
  );
  if (!(await page.exists('nav [data-host="Lab/r1-password.yaml"]')))
    await page.click('nav [data-item=folder][data-path="Lab"]');
  await until("the hosts in the tree", () =>
    page.exists('nav [data-host="Lab/r1-password.yaml"]'),
  );
  await page.click('nav [data-host="Lab/r1-password.yaml"]');
  await page.js(
    `document.querySelector('nav [data-host="Lab/r1-password.yaml"]').parentElement.querySelector(":scope > [data-row-details]")?.setAttribute("data-check-details", "")`,
  );
  await page.click("[data-check-details]");
  await until("the details sheet", async () =>
    (await page.text("[data-details-sheet]")).includes("r1-password"),
  );
  await page.picture("2-details");
  step(
    "the workspace opened from the welcome view; tree and details sheet shown",
  );

  await page.click(`[data-tab-bar] [data-new-tab]`).catch(() => {});
  await page.js("document.activeElement?.blur()");
  await page.keys(CTRL, "k");
  await until("the palette, its query focused", () =>
    page.js("return document.activeElement?.name === 'palette-query'"),
  );
  await page.typeKeys("r1-key");
  await until("the host found", () =>
    page.exists('[data-hit="Lab/r1-key.yaml"]'),
  );
  await page.picture("2-palette");
  await page.keys(ESCAPE);
  await until(
    "the palette closed",
    async () => !(await page.exists("[name=palette-query]")),
  );
  step(
    "Ctrl+K opened the command palette, typing found a host, Escape closed it",
  );

  await openPreferences(page);
  const theme = await page.js(`return {
    setting: document.querySelector("[name=preference-theme]").value,
    dark: matchMedia("(prefers-color-scheme: dark)").matches,
    applied: document.documentElement.dataset.theme ?? null,
  }`);
  step(
    `Preferences opened; theme "${theme.setting}", the desktop is ${theme.dark ? "dark" : "light"}, the app ${theme.applied}`,
  );
  return theme;
}

/** 3. The vault is the only store: Preferences says so, saving a profile
 * offers to turn it on, turning it off is refused. */
async function vaultOnly(page) {
  await until("Security without a keychain", () =>
    page.exists("[data-vault-status=off][data-vault-keychain=false]"),
  );
  if (
    !/only be kept in the vault/.test(
      await page.text("[data-vault-no-keychain]"),
    )
  )
    fail("Preferences do not say the vault is the only store");
  await page.picture("3-security");
  await closeTab(page, "[data-tab-view=preferences]");
  step("Preferences, Security: credentials can only be kept in the vault");

  const view = "[data-credentials]";
  await showSideView(page, "credentials");
  await until("the vault note", () =>
    page.exists(`${view} [data-profiles-needs-vault]`),
  );
  await page.click(`${view} [data-profile-new]`);
  await page.type(`${view} [name=profile-name]`, "lab-password");
  await page.type(`${view} [name=profile-username]`, USERNAME);
  await page.type(`${view} [name=profile-password]`, PASSWORD);
  await page.click(`${view} form button[type=submit]`);
  const why = await until("saving explained", () =>
    page.text(`${view} [data-needs-vault=explain]`),
  );
  if (!/no keychain/.test(why)) fail(`the explanation: ${why}`);
  await page.click(`${view} [data-needs-vault-action=turn-on]`);
  await page.type(
    `${view} [data-vault-form] [name=vault-password]`,
    MASTER_PASSWORD,
  );
  await page.type(
    `${view} [data-vault-form] [name=vault-repeat]`,
    MASTER_PASSWORD,
  );
  await page.click(`${view} [data-vault-form] button[type=submit]`);
  await until("the profile saved into the vault", () =>
    page.exists(
      `${view} [data-profile="lab-password"]:not([data-profile-missing])`,
    ),
  );
  await page.picture("3-vault-on");
  step(
    "saving a profile explained there is no keychain, turned the vault on and saved it",
  );

  const profile = async (name, kind, fields) => {
    await page.click(`${view} [data-profile-new]`);
    await page.type(`${view} [name=profile-name]`, name);
    await page.type(`${view} [name=profile-username]`, USERNAME);
    await page.js(
      `const select = document.querySelector(arguments[0]);
       select.value = arguments[1];
       select.dispatchEvent(new Event("change", { bubbles: true }));`,
      `${view} [name=profile-auth]`,
      kind,
    );
    for (const [field, value] of Object.entries(fields))
      await page.type(`${view} [name=${field}]`, value);
    await page.click(`${view} form button[type=submit]`);
    await until(`the profile ${name}`, () =>
      page.exists(
        `${view} [data-profile="${name}"]:not([data-profile-missing])`,
      ),
    );
  };
  await profile("lab-key", "private_key", { "profile-key-file": keyFile });
  await profile("lab-agent", "agent", {});
  step("a key profile and an agent profile were saved into the vault");

  await openPreferences(page);
  await page.click("[data-vault=turn-off]");
  const off = await until("turning off explained", () =>
    page.text("[data-vault-off-no-keychain]"),
  );
  if (!/no other store/.test(off)) fail(`turning off: ${off}`);
  await page.click("[data-vault-off-no-keychain] [data-vault=cancel]");
  await closeTab(page, "[data-tab-view=preferences]");
  await showSideView(page, "hosts");
  step("turning the vault off said there is no other store");
}

/** The ids of the tabs, in every pane. */
function tabIds(page) {
  return page.js(
    `return [...document.querySelectorAll("[role=tab][data-tab]")].map((tab) => tab.dataset.tab)`,
  );
}

/** Double-clicks the host and waits for its new tab's session to show
 * `prompt` (trusting the host key if asked); resolves to the tab's slot
 * and whether the key was asked about. */
async function connectHost(page, host, prompt = "test-ssh$") {
  const before = await tabIds(page);
  await page.doubleClick(`nav [data-host="${host}"]`);
  const tab = await until(`${host}'s tab`, async () =>
    (await tabIds(page)).find((id) => !before.includes(id)),
  );
  const slot = `.slot[data-tab="${tab}"]`;
  const shown = await until(
    `${host}: connected or a host key question`,
    async () => {
      if (
        await page.exists(
          `${slot} [data-host-key=first-use] [data-decision=trustAndRemember]`,
        )
      )
        return "question";
      return (await screen(page, slot)).includes(prompt) && "connected";
    },
  );
  if (shown === "question") {
    await page.click(
      `${slot} [data-host-key=first-use] [data-decision=trustAndRemember]`,
    );
    await until(`${host}'s prompt`, async () =>
      (await screen(page, slot)).includes(prompt),
    );
  }
  return { slot, tab, asked: shown === "question" };
}

/** 4. SSH with a password, a key and an agent profile; telnet. */
async function sessions(page) {
  const password = await connectHost(page, "Lab/r1-password.yaml");
  if (password.asked)
    step("the first connection asked about the host key; trusted");
  if (await page.exists("main input[name=password]"))
    fail("a password was asked for");
  await page.picture("4-ssh-password");
  step("SSH with the password profile from the vault: logged in, no prompt");
  await connectHost(page, "Lab/r1-key.yaml");
  step("SSH with the key profile (the test key's file): logged in");
  await connectHost(page, "Lab/r1-agent.yaml");
  step(
    `SSH with the agent profile (an ssh-agent of the check's own, SSH_AUTH_SOCK): logged in`,
  );

  const telnet = await connectHost(page, "Lab/sw-telnet.yaml", "Username:");
  await focusTerminal(page, telnet.slot);
  await page.typeKeys(`${USERNAME}\r`);
  await until("the telnet password prompt", async () =>
    (await screen(page, telnet.slot)).includes("Password:"),
  );
  await page.typeKeys(`${PASSWORD}\r`);
  await until("the telnet device's prompt", async () =>
    (await screen(page, telnet.slot)).includes("test-telnet>"),
  );
  if (
    (await page.text(`${telnet.slot} [data-unencrypted]`)).trim() !==
    "Unencrypted"
  )
    fail("the telnet session is not marked unencrypted");
  await page.picture("4-telnet");
  step("telnet: logged in in the terminal, marked unencrypted");
  return password;
}

/** Clicks into the session's terminal, as a user does before typing. */
async function focusTerminal(page, slot) {
  await page.click(`${slot} .xterm-screen`);
  await until("the terminal focused", () =>
    page.js(
      `return !!document.activeElement?.closest(arguments[0] + " .terminal")`,
      slot,
    ),
  );
}

/** 5. Typing, copy and paste, WebGL, zoom, split panes, tabs, multi-send. */
async function terminal(page, first) {
  await page.click(`[role=tab][data-tab="${first.tab}"]`);
  await until("its session shown", () =>
    page.exists(`${first.slot}[data-active]`),
  );
  await focusTerminal(page, first.slot);
  await page.typeKeys("typed-on-linux\r");
  await until("the typed line echoed", async () =>
    /\$ typed-on-linux\s*\n/.test(await screen(page, first.slot)),
  );
  step("typing reached the device and came back");

  // xterm's WebGL renderer draws on a canvas and has no DOM rows.
  const renderer = await page.js(
    `const slot = document.querySelector(arguments[0]);
     return {
       canvas: !!slot.querySelector(".xterm-screen canvas:not(.xterm-link-layer)"),
       rows: !!slot.querySelector(".xterm-rows"),
     };`,
    first.slot,
  );
  if (renderer.canvas && !renderer.rows)
    step("the terminal draws with WebGL (its canvas, no DOM rows)");
  else
    problem(
      `the terminal does not draw with WebGL: ${JSON.stringify(renderer)}`,
    );

  // Copy: the typed word selected, Ctrl+Shift+C; paste: Ctrl+Shift+V.
  await page.js(
    `const terminal = document.querySelector(arguments[0] + " .terminal").xterm;
     const buffer = terminal.buffer.active;
     for (let y = buffer.length - 1; y >= 0; y--) {
       const line = buffer.getLine(y).translateToString(true);
       const at = line.indexOf("typed-on-linux");
       if (at >= 0) { terminal.select(at, y, "typed-on-linux".length); break; }
     }`,
    first.slot,
  );
  await page.keys(CTRL, SHIFT, "c");
  await page.keys(CTRL, SHIFT, "v");
  await until("the copied word pasted", async () =>
    /\$ typed-on-linux\s*\n(.|\n)*\$ typed-on-linux/.test(
      await screen(page, first.slot),
    ),
  );
  await page.typeKeys("\r");
  step(
    "Ctrl+Shift+C copied the selection and Ctrl+Shift+V pasted it (the system clipboard)",
  );

  const size = () =>
    page.js(
      `return document.querySelector(arguments[0] + " .terminal").xterm.options.fontSize`,
      first.slot,
    );
  const before = await size();
  await page.keys(CTRL, "=");
  await until("the terminal zoomed in", async () => (await size()) > before);
  await page.keys(CTRL, "0");
  await until("the zoom reset", async () => (await size()) === before);
  step(`Ctrl+= zoomed the terminal in from ${before}px, Ctrl+0 reset it`);

  // Multi-send: this session joins (Ctrl+Shift+M) before the split.
  const inGroup = (slot) => page.exists(`${slot} [data-multi-send-bar]`);
  await page.keys(CTRL, SHIFT, "m");
  await until("the session in the multi-send group", () => inGroup(first.slot));

  const panes = () =>
    page.js(`return document.querySelectorAll("[data-tab-bar]").length`);
  const panesBefore = await panes();
  await paneTool(page, "[data-split=right]", "Split right");
  await until("a second pane", async () => (await panes()) > panesBefore);
  await page.picture("5-split");
  step(`split right: ${panesBefore} pane became ${await panes()}`);

  // A second session of the host, in the active pane, joins too.
  const second = await connectHost(page, "Lab/r1-password.yaml");
  await focusTerminal(page, second.slot);
  await page.keys(CTRL, SHIFT, "m");
  await until("the second session in the group", () => inGroup(second.slot));
  await page.typeKeys("to-both\r");
  const received = (slot) => async () =>
    /\$ to-both\s*\n/.test(await screen(page, slot));
  await until("to-both in the second session", received(second.slot));
  await page.picture("5-multi-send");
  await page.keys(CTRL, SHIFT, "m");
  await until(
    "the second session out of the group",
    async () => !(await inGroup(second.slot)),
  );
  // The first is a hidden tab of the same pane now, whose output is held
  // until shown: Ctrl+PageUp shows it.
  await page.keys(CTRL, PAGE_UP);
  await until("the first session shown", () =>
    page.exists(`${first.slot}[data-active]`),
  );
  await until("to-both in the first session", received(first.slot));
  step(
    `${(await tabIds(page)).length} tabs in ${await panes()} panes; multi-send (Ctrl+Shift+M in two sessions) typed into both`,
  );
}

/** A console on a pseudo terminal: `socat` makes one at `consolePort`
 * and forwards it to a TCP port, where the script plays the device (a
 * console server's port reached this way looks the same to the app).
 * Resolves to the device: `send`, `received()`. */
async function startConsole() {
  const forwarder = await startProcess(
    "socat",
    [
      "-d",
      "-d",
      `PTY,raw,echo=0,link=${consolePort}`,
      `TCP-LISTEN:${CONSOLE_PORT},bind=127.0.0.1,reuseaddr`,
    ],
    process.env,
    (output) => output.includes("listening on"),
  ).catch((error) => {
    if (/ENOENT|socat to start/.test(String(error.message)))
      fail("socat is needed for the serial console (apt-get install socat)");
    throw error;
  });
  const socket = await new Promise((resolve, reject) => {
    const made = connect(CONSOLE_PORT, "127.0.0.1", () => resolve(made));
    made.on("error", reject);
  });
  let received = "";
  socket.on("data", (chunk) => (received += chunk.toString("latin1")));
  await until("the console's pseudo terminal", () => existsSync(consolePort));
  return {
    forwarder,
    send: (text) => socket.write(text),
    received: () => received,
    close: () => socket.destroy(),
  };
}

/** 4 (serial). The console opened from quick connect by its path, at
 * 115200: bytes both ways, Send break from the session's menu, the
 * session going on after it. A pseudo terminal has no wire: the break
 * reaches no device, and the speed is only the line's setting. */
async function serialConsole(page) {
  const device = await startConsole();
  const before = await tabIds(page);
  await paneTool(page, "[data-new-tab]", "New connection in this pane");
  const form = ".slot.connect [data-quick-serial]";
  await page.click(".slot.connect [data-quick-mode=serial]");
  await page.type(`${form} [name=serial-port]`, consolePort);
  await page.type(`${form} [name=baud-rate]`, "115200");
  await page.click(`${form} button[type=submit]`);
  const tab = await until("the serial session's tab", async () =>
    (await tabIds(page)).find((id) => !before.includes(id)),
  );
  const slot = `.slot[data-tab="${tab}"]`;
  const status = () => page.text(`${slot} [role=status]`);
  await until("the serial session connected", async () => {
    const said = await status();
    if (/closed|cannot/i.test(said)) fail(`the serial session: ${said}`);
    return /connected/i.test(said);
  });
  device.send("\r\nSwitch>");
  await until("the console's prompt", async () =>
    (await screen(page, slot)).includes("Switch>"),
  );
  await focusTerminal(page, slot);
  await page.typeKeys("show clock\r");
  await until("the typed line at the console", () =>
    device.received().endsWith("show clock\r"),
  );
  await page.picture("4-serial");
  step(
    `serial: ${consolePort} (a pseudo terminal) opened from quick connect at 115200; the console's output shown, typing reached it`,
  );

  await page.click(`${slot} [data-session-menu]`);
  await page.click("[data-send-break]");
  await page.typeKeys("after break\r");
  await until("the line typed after the break", () =>
    device.received().endsWith("after break\r"),
  );
  if (!/connected/i.test(await status()))
    fail(`after Send break the session is ${await status()}`);
  step(
    "Send break (the session's menu) was taken, and the session went on: the line typed after it reached the console",
  );
  await closeTab(page, `[role=tab][data-tab="${tab}"]`);
  // A live session's tab asks first.
  await page.click("[data-confirm-close] [data-confirm=close]");
  await until(
    "the serial tab closed",
    async () => !(await tabIds(page)).includes(tab),
  );
  device.close();
}

/** 6. Python: the system's Python, the environment from the package's
 * wheel, a script job; the headless binary from a shell. */
async function python(page) {
  await openPreferences(page);
  await page.click("[data-python=set-up]");
  const setup = ".slot.view:not([hidden]) [data-view=python-setup]";
  await until(
    "the setup offer",
    async () =>
      (await page.js(
        `return document.querySelector(arguments[0])?.dataset.step`,
        setup,
      )) === "offer",
  );
  const offer = await page.js(
    `return {
    base: document.querySelector("[data-python-base]")?.textContent,
    boxes: [...document.querySelectorAll(arguments[0] + " input[type=checkbox]")].map((box) => [box.name, box.checked]),
  }`,
    setup,
  );
  for (const [name, checked] of offer.boxes)
    if (checked) await page.click(`${setup} [name=${name}]`);
  await page.click("[data-python-setup=continue]");
  await until(
    "the confirmation",
    async () =>
      (await page.js(
        `return document.querySelector(arguments[0])?.dataset.step`,
        setup,
      )) === "confirm",
  );
  const installs = await page.js(
    `return [...document.querySelectorAll("[data-python-install]")].map((item) => item.dataset.pythonInstall + " from " + item.dataset.source)`,
  );
  await page.click("[data-python-setup=install]");
  await until(
    "the environment set up",
    async () => {
      const at = await page.js(
        `return document.querySelector(arguments[0])?.dataset.step`,
        setup,
      );
      if (at === "failed")
        fail(
          `the Python setup failed: ${await page.text("[data-python-setup-result]")}`,
        );
      return at === "done";
    },
    180000,
  );
  await page.picture("6-python-done");
  step(`Python set up with ${offer.base} from ${JSON.stringify(installs)}`);
  await closeTab(page, "[data-tab-view=python-setup]");
  await closeTab(page, "[data-tab-view=preferences]");

  await showSideView(page, "hosts");
  if (!(await page.exists('nav [data-host="Jobs/jobs-r1.yaml"]')))
    await page.click('nav [data-item=folder][data-path="Jobs"]');
  await page.click('nav [data-item=folder][data-path="Jobs"]');
  if (!(await page.exists(".side [data-folder-run-script]"))) {
    await page.js(
      `document.querySelector('nav [data-item=folder][data-path="Jobs"]').parentElement.querySelector(":scope > [data-row-details]")?.setAttribute("data-check-details", "")`,
    );
    await page.click("[data-check-details]");
  }
  await page.click(".side [data-folder-run-script]");
  const job = ".slot.view:not([hidden]) [data-view=job]";
  await until("the script listed", () =>
    page.exists(`${job} [name=job-script] option[value="Jobs/linux_check.py"]`),
  );
  await page.js(
    `const select = document.querySelector(arguments[0]);
     select.value = "Jobs/linux_check.py";
     select.dispatchEvent(new Event("change", { bubbles: true }));`,
    `${job} [name=job-script]`,
  );
  await page.click(`${job} [data-job=plan]`);
  await until("the pre-flight", () =>
    page.exists(`${job}[data-step=preflight]`),
  );
  await page.click(`${job} [data-job=start]`);
  await until(
    "the script finished",
    () => page.exists(`${job} [data-job-progress][data-finished]`),
    60000,
  );
  const done = await page.js(
    `
    const row = document.querySelector(arguments[0] + ' [data-job-result="script"]');
    return { state: row.dataset.hostState, exit: row.dataset.exitStatus, output: row.querySelector("[data-job-output]").textContent };`,
    job,
  );
  if (
    done.state !== "done" ||
    done.exit !== "0" ||
    !done.output.includes("system Linux") ||
    !done.output.includes(`login ${USERNAME}`)
  )
    fail(`the script job: ${JSON.stringify(done)}`);
  await page.picture("6-script-job");
  step(
    `a script job ran with the app's environment: ${done.output.trim().split("\n").join(" / ")}`,
  );

  const inventory = spawnSync(options.cli, ["inventory", workspace], {
    encoding: "utf8",
    env: { ...process.env, NETTERM_DATA_DIR: dataFolder },
  });
  if (inventory.status !== 0 || !inventory.stdout.includes("r1-password"))
    fail(`netterm-cli inventory: ${inventory.status} ${inventory.stderr}`);
  step("netterm-cli inventory <workspace> worked from a shell");
}

/** 7. The update notice only offers the release page. */
async function updates(page, release) {
  const notice = await until("the update notice", () =>
    page.js(`const n = document.querySelector("[data-update]");
      return n && { action: n.dataset.update, version: n.dataset.updateVersion,
        install: !!n.querySelector("[data-update-install]"), page: !!n.querySelector("[data-update-page]") };`),
  );
  if (
    notice.action !== "notify" ||
    notice.install ||
    !notice.page ||
    notice.version !== NEWER_VERSION
  )
    fail(`the update notice: ${JSON.stringify(notice)}`);
  // A package's copy points at the package manager, any other copy at
  // the download.
  const said = (await page.text("[data-update] .detail"))
    .replace(/\s+/g, " ")
    .trim();
  const expected = portable ? /portable copy/ : /package manager/;
  if (!expected.test(said)) problem(`the update notice says: ${said}`);
  await page.picture("7-update");
  await page.click("[data-update-page]");
  await until(
    "the release page handed to the opener",
    () =>
      existsSync(openedFile) &&
      readFileSync(openedFile, "utf8").includes(RELEASE_PAGE),
  );
  await page.click("[data-update-later]");
  await until(
    "the notice closed",
    async () => !(await page.exists("[data-update]")),
  );
  step(
    `the update check (${release.requests} request) announced ${NEWER_VERSION} with its page only, no install ("${said}"); the page went to the desktop's opener`,
  );
}

/** 8. Open log folder and web links go to the desktop's opener. */
async function opener(page) {
  await showSideView(page, "hosts");
  await page.click('nav [data-host="Lab/r1-password.yaml"]');
  if (!(await page.exists("[data-details-sheet]"))) {
    await page.js(
      `document.querySelector('nav [data-host="Lab/r1-password.yaml"]').parentElement.querySelector(":scope > [data-row-details]")?.setAttribute("data-check-details", "")`,
    );
    await page.click("[data-check-details]");
  }
  await page.click("[data-details-sheet] [data-web-link=status]");
  await until("the web link handed to the opener", () =>
    readFileSync(openedFile, "utf8").includes(WEB_LINK),
  );
  await page.click("[data-details-sheet] [data-open-logs]");
  // Under WSL as a Windows path (\\wsl$\<distribution>\tmp\...).
  await until("the log folder handed to the opener", () =>
    readFileSync(openedFile, "utf8")
      .split("\n")
      .some((line) =>
        line.replaceAll("\\", "/").includes(`${dataFolder}/logs/`),
      ),
  );
  const lines = readFileSync(openedFile, "utf8").trim().split("\n");
  step(
    `a web link and "Open log folder" went to the desktop's opener: ${JSON.stringify(lines)}`,
  );
}

/** 9. The app's own frame: no title bar, its buttons, the top bar. Run
 * with one pane, before any session. */
async function frame(page) {
  const answer = async (command) => {
    const got = await page.window(command);
    if (got.error) fail(`${command}: ${got.error}`);
    return got.value;
  };
  if ((await answer("is_decorated")) !== false)
    fail("the window has the system's frame");
  const buttons = await page.js(
    `return [...document.querySelectorAll("[data-window-control]")].filter((b) => b.checkVisibility()).map((b) => b.dataset.windowControl)`,
  );
  if (buttons.join() !== "minimize,maximize,close")
    fail(`the window's buttons: ${buttons}`);
  step(
    "no system frame (is_decorated false); Minimize, Maximize, Close in the top bar",
  );

  // The page's size follows the window's a moment after its state.
  const width = () => page.js("return innerWidth");
  const normal = await width();
  await page.click("[data-window-control=maximize]");
  await until("maximized by its button", () => answer("is_maximized"));
  await until(
    "the page as wide as the screen",
    async () => (await width()) > normal,
  );
  await page.picture("9-maximized");
  await page.click("[data-window-control=maximize]");
  await until(
    "restored by its button",
    async () => !(await answer("is_maximized")),
  );
  await until(
    "the page back to its width",
    async () => (await width()) === normal,
  );
  step("Maximize maximized the window, Restore restored it");

  // In a desktop session the double click is made with the desktop's own
  // pointer (desktopFrame). WebDriver's synthetic one confuses xfwm4 on
  // the way back (its state stays maximized at the normal size), which a
  // real double click does not.
  if (desktop) return;
  // A point of the top bar where nothing but the bar is (a drag region).
  const empty = await page.js(`
    const bar = document.querySelector("[data-tab-bar]");
    const box = bar.getBoundingClientRect();
    const y = Math.round(box.top + box.height / 2);
    for (let x = Math.round(box.right) - 8; x > box.left; x -= 4)
      if (document.elementFromPoint(x, y) === bar) return { x, y };
    return null;`);
  if (!empty) fail("no empty part in the top bar");
  const doubleClickBar = () =>
    page.pointer("body", () => [
      { type: "pointerMove", origin: "viewport", ...empty },
      { type: "pointerDown", button: 0 },
      { type: "pointerUp", button: 0 },
      { type: "pointerDown", button: 0 },
      { type: "pointerUp", button: 0 },
    ]);
  await doubleClickBar();
  if (await settles(() => answer("is_maximized"))) {
    await until(
      "the page as wide as the screen",
      async () => (await width()) > normal,
    );
    await doubleClickBar();
    if (!(await settles(async () => !(await answer("is_maximized")), 10000)))
      fail(
        `a second double click did not restore the window: the app says is_maximized ${await answer("is_maximized")}, ` +
          `the page is ${await width()} wide (${normal} before)` +
          (desktop
            ? `, the window manager has ${JSON.stringify(appWindows())}`
            : ""),
      );
    await until(
      "the page back to its width",
      async () => (await width()) === normal,
    );
    step(
      "a double click on the top bar's empty part maximized and restored the window",
    );
  } else
    problem(
      "a double click on the top bar's empty part did not maximize the window",
    );
}

/** 9 (end): Minimize. Last, as a minimized window may stop drawing.
 * WSLg reports back neither the window's place (Wayland has none to read;
 * under WSLg's XWayland it stays put, as the compositor, not X, moves
 * windows) nor whether it is minimized. */
async function minimize(page) {
  await page.click("[data-window-control=minimize]");
  const got = await settles(
    async () => (await page.window("is_minimized")).value,
  );
  note(`Minimize clicked: is_minimized ${got} (under WSLg it stays false)`);
}

/** 3 (end) and 2 (theme): after a restart the vault asks for its
 * password; with a dark desktop theme the app is dark. */
async function restart(bin) {
  // In a desktop session the desktop itself is made dark (XFCE's style,
  // GNOME's colour scheme) before the app starts: what it starts with,
  // the AppImage's GTK hooks included (#143). Otherwise GTK_THEME plays a
  // dark desktop (an AppImage's runtime sets GTK_THEME itself, from the
  // colour scheme, unless APPIMAGE_GTK_THEME says otherwise).
  let darkDesktop = {
    GTK_THEME: "Adwaita:dark",
    APPIMAGE_GTK_THEME: "Adwaita:dark",
  };
  let how = "GTK_THEME=Adwaita:dark";
  if (desktop) {
    how = desktop.setDark(true);
    darkDesktop = {};
  }
  const page = await startApp(appEnvironment(bin, darkDesktop));
  await until("the unlock screen", () =>
    page.exists("[data-unlock-screen] [name=vault-unlock-password]"),
  );
  const theme = await page.js(`return {
    dark: matchMedia("(prefers-color-scheme: dark)").matches,
    applied: document.documentElement.dataset.theme ?? null,
  }`);
  await page.picture("3-unlock-dark");
  step("after a restart the unlock screen asked for the master password");
  if (theme.dark)
    step(
      `started with the desktop dark (${how}) the app is dark: ${JSON.stringify(theme)}`,
    );
  else
    problem(
      `started with the desktop dark (${how}) the app is not dark: ${JSON.stringify(theme)}`,
    );
  await page.type("[name=vault-unlock-password]", MASTER_PASSWORD);
  await page.click("[data-unlock=unlock]");
  await until(
    "unlocked",
    async () => !(await page.exists("[data-unlock-screen]")),
  );
  step("the vault unlocked with the master password");
  const running = page.app();
  if (running.length === 0) fail("the app's process was not found");
  await page.click("[data-window-control=close]");
  if (await settles(() => running.every((pid) => !alive(pid))))
    step("Close closed the window, and the app exited");
  else problem("the app did not exit after Close");
  await page.quit();
}

async function main() {
  console.log(`test:linux-desktop: temporary folder ${temp}`);
  if (options.desktop) {
    desktop = await startDesktop(options.desktop, join(temp, "desktop"), note);
    children.push({ kill: () => desktop.stop() });
    // Everything started from here on is in the desktop session.
    for (const name of Object.keys(process.env))
      if (!(name in desktop.env)) delete process.env[name];
    Object.assign(process.env, desktop.env);
    step(`a desktop session out of sight: ${desktop.name}`);
  }
  createWorkspace();
  const bin = fakeOpener();
  const release = await startServers();
  children.push({ kill: release.close });
  if (desktop && options.app === "/usr/bin/netterm") await desktopMenu();
  const page = await startApp(appEnvironment(bin));
  try {
    await started(page);
    if (desktop) await desktopTheme(page);
    await updates(page, release);
    // The release page opened a real browser: minimized, out of the way of
    // the frame checks (desktopOpener sees it again for the web link).
    // (Closed, GNOME Web 49 aborts, and XFCE's opener, which waits for
    // it, then says it failed.)
    if (desktop)
      for (const window of await until(
        "the browser with the release page",
        () =>
          desktop.windows(BROWSER.classes).length &&
          desktop.windows(BROWSER.classes),
        60000,
      ).catch(() => []))
        desktop.minimize(window);
    await frame(page);
    if (desktop) await desktopFrame(page);
    await workspaceAndViews(page);
    await vaultOnly(page);
    await terminal(page, await sessions(page));
    await serialConsole(page);
    await python(page);
    await opener(page);
    if (desktop) {
      await desktopOpener(release);
      await desktopMinimize(page);
    } else await minimize(page);
  } catch (error) {
    await page.picture("failure").catch(() => {});
    throw error;
  } finally {
    await page.quit();
  }
  await restart(bin);
  console.log(`test:linux-desktop: pictures in ${pictures}`);
}

// Stopped from outside: what was started goes too (a desktop session's
// programs run in process groups of their own).
for (const signal of ["SIGINT", "SIGTERM"])
  process.on(signal, () => {
    for (const child of children) child.kill();
    process.exit(1);
  });

try {
  await main();
  if (!process.exitCode) console.log("test:linux-desktop: every check passed");
} catch (error) {
  if (!error.reported) console.error(error);
  process.exitCode = 1;
} finally {
  for (const child of children) child.kill();
  // Kept after a failure, to look into; after a pass only the pictures.
  if (!process.exitCode)
    for (const name of [
      "data",
      "workspace",
      "keys",
      "xdg-data",
      "xdg-cache",
      "xdg-config",
      "desktop",
    ])
      rmSync(join(temp, name), { recursive: true, force: true });
  process.exit();
}
