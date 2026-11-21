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
// Not checked here (see docs/linux.md): dragging the window, resizing it
// from its edges, Minimize's effect, edge tiling, and what the desktop's
// opener then does.
//
// Needs, besides those two: Rust (to build the example server),
// openssh-client (ssh-agent, ssh-add), python3 with venv, and the app's
// package installed (/usr/bin/netterm-cli, used from the shell).

import { spawn, spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { quoted, ROOT, syncIntoWsl, wsl, WSL_FOLDER } from "./wsl.mjs";

const SSH_PORT = 24722;
const TELNET_PORT = 24723;
const RELEASE_PORT = 24724;
const DRIVER_PORT = 24727;
const NATIVE_PORT = 24728;
const USERNAME = "engineer";
const PASSWORD = "correct horse";
const MASTER_PASSWORD = "linux desktop master";
const NEWER_VERSION = "99.0.0";
const RELEASE_PAGE = "https://example.invalid/releases/99.0.0";
const TIMEOUT_MS = 20000;

function fail(message) {
  console.error(`test:linux-desktop: FAILED: ${message}`);
  process.exitCode = 1;
  throw Object.assign(new Error(message), { reported: true });
}

const options = { app: "/usr/bin/netterm", backend: null, pictures: null };
for (let at = 2; at < process.argv.length; at++) {
  const name = process.argv[at].replace(/^--/, "");
  if (!(name in options)) {
    console.error(`test:linux-desktop: unknown option ${process.argv[at]}`);
    process.exit(2);
  }
  options[name] = process.argv[++at];
}

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
const pictures = options.pictures ?? join(temp, "pictures");
const children = [];
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
        (name === "r1-password"
          ? "web_links:\n  status: https://example.invalid/status\n"
          : ""),
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
 * the Windows browser: that one is replaced too. */
function fakeOpener() {
  const bin = join(temp, "bin");
  mkdirSync(bin);
  const record = (name, what) =>
    `#!/bin/sh\nprintf '%s %s\\n' ${name} "${what}" >> ${quoted(openedFile)}\n`;
  for (const name of ["xdg-open", "gio", "gnome-open", "kde-open"])
    writeFileSync(join(bin, name), record(name, "$*"));
  writeFileSync(
    join(bin, "powershell.exe"),
    record("windows", "$OPEN_RS_TARGET"),
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
  const release = { requests: 0 };
  const manifest = JSON.stringify({
    version: NEWER_VERSION,
    notes: "A newer release.",
    pub_date: "2026-11-01T10:00:00Z",
    page: RELEASE_PAGE,
    platforms: {},
  });
  const server = createServer((request, response) => {
    release.requests += 1;
    response.writeHead(200, { "Content-Type": "application/json" });
    response.end(manifest);
  });
  await new Promise((resolve) =>
    server.listen(RELEASE_PORT, "127.0.0.1", resolve),
  );
  release.close = () => server.close();
  return release;
}

/** The app's environment: temporary data and XDG folders, the agent, the
 * release server, the recording opener first on the PATH. */
function appEnvironment(bin, extra = {}) {
  const env = {
    ...process.env,
    NETTERM_DATA_DIR: dataFolder,
    XDG_DATA_HOME: join(temp, "xdg-data"),
    XDG_CACHE_HOME: join(temp, "xdg-cache"),
    XDG_CONFIG_HOME: join(temp, "xdg-config"),
    SSH_AUTH_SOCK: agentSocket,
    NETTERM_UPDATE_ENDPOINT: `http://127.0.0.1:${RELEASE_PORT}/latest.json`,
    PATH: `${bin}:${process.env.PATH}`,
    ...extra,
  };
  if (options.backend) env.GDK_BACKEND = options.backend;
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
      // Under X11 also the window as the X server has it (ImageMagick's
      // import; WSLg's XWayland cannot capture its root window).
      if ((options.backend ?? process.env.GDK_BACKEND) === "x11") {
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
  step(
    `the app started (${options.app}, GDK_BACKEND ${options.backend ?? process.env.GDK_BACKEND ?? "unset"}): ` +
      `window "${info.title}" ${info.size.join("x")}, WebGL 2 ${info.webgl2 ? "available" : "NOT available"}`,
  );
  await page.picture("1-welcome");
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
  await page.click(`[data-split=right]`);
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

  const inventory = spawnSync(
    "/usr/bin/netterm-cli",
    ["inventory", workspace],
    {
      encoding: "utf8",
      env: { ...process.env, NETTERM_DATA_DIR: dataFolder },
    },
  );
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
  const expected = options.app.startsWith("/usr/")
    ? /package manager/
    : /portable copy/;
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
    readFileSync(openedFile, "utf8").includes("https://example.invalid/status"),
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
    await until(
      "restored by a double click",
      async () => !(await answer("is_maximized")),
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
  // An AppImage's runtime sets GTK_THEME itself, from the desktop's
  // colour scheme, unless APPIMAGE_GTK_THEME says otherwise.
  const darkDesktop = {
    GTK_THEME: "Adwaita:dark",
    APPIMAGE_GTK_THEME: "Adwaita:dark",
  };
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
      `with the desktop dark (GTK_THEME=Adwaita:dark) the app is dark: ${JSON.stringify(theme)}`,
    );
  else
    problem(
      `with GTK_THEME=Adwaita:dark the app is not dark: ${JSON.stringify(theme)}`,
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
  createWorkspace();
  const bin = fakeOpener();
  const release = await startServers();
  children.push({ kill: release.close });
  const page = await startApp(appEnvironment(bin));
  try {
    await started(page);
    await updates(page, release);
    await frame(page);
    await workspaceAndViews(page);
    await vaultOnly(page);
    await terminal(page, await sessions(page));
    await python(page);
    await opener(page);
    await minimize(page);
  } catch (error) {
    await page.picture("failure").catch(() => {});
    throw error;
  } finally {
    await page.quit();
  }
  await restart(bin);
  console.log(`test:linux-desktop: pictures in ${pictures}`);
}

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
    ])
      rmSync(join(temp, name), { recursive: true, force: true });
  process.exit();
}
