// End-to-end smoke test: launches the built app and the test SSH server,
// drives the real interface through the WebView2 DevTools protocol, and
// checks what a user would see.
//
//   npm run app:build && npm run smoke
//
// Windows only for now: it relies on WebView2's remote-debugging switch.

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

const SSH_PORT = 22722;
// A second test server, restricted to an old device's algorithms.
const OLD_DEVICE_PORT = 22723;
// The test telnet device (same username and password, asked in the terminal).
const TELNET_PORT = 22724;
const DEVTOOLS_PORT = 9333;
const USERNAME = "engineer";
const PASSWORD = "correct horse";
// The test server's user who also needs a one-time code, and the code.
const OTP_USERNAME = "engineer-otp";
const ONE_TIME_CODE = "424242";
// The test server's SSH agent, which the app reaches through SSH_AUTH_SOCK:
// the user's own agent is never used.
const AGENT_PIPE = String.raw`\\.\pipe\netterm-smoke-agent-${process.pid}`;
const TIMEOUT_MS = 15000;
/** The session the keyboard goes to: the one the active pane shows. */
const SHOWN_SESSION = ".slot[data-active]";
// A public key the test server does not have: stored as the server's key,
// it makes the server's real key look changed.
const OTHER_KEY_FILE = "crates/core/tests/fixtures/keys/rsa_client.pub";

const children = [];
const userData = mkdtempSync(join(tmpdir(), "netterm-smoke-"));
// The app's own data folder, holding a last workspace to reopen on launch.
const appData = join(userData, "app-data");
const workspace = join(userData, "smoke-workspace");
// Where the app writes the web links it would open in the browser (the
// smoke test never starts a browser).
const openedLinks = join(userData, "opened-links.txt");
// Beside the built program, the first makes the app an installed copy (the
// installer writes its uninstaller there; the smoke's is an empty file,
// never run), the second a portable one (the release script puts it in the
// portable zip). The first launch is "installed", the restart "portable".
const INSTALLED_MARKER = "target/release/uninstall.exe";
const PORTABLE_MARKER = "target/release/portable.txt";
// The release the smoke's own release server announces.
const NEWER_VERSION = "99.0.0";
const RELEASE_PAGE = `https://example.invalid/netterm/releases/tag/v${NEWER_VERSION}`;
/** The local release server (the app is pointed at it through
 * NETTERM_UPDATE_ENDPOINT): no real server is ever asked. */
const releaseServer = { url: "", requests: 0, close: () => {} };

function startReleaseServer() {
  const manifest = JSON.stringify({
    version: NEWER_VERSION,
    notes: "Smoke release.",
    pub_date: "2026-11-01T10:00:00Z",
    page: RELEASE_PAGE,
    platforms: {
      "windows-x86_64": {
        signature: "bm90IGEgc2lnbmF0dXJl",
        url: "https://example.invalid/netterm-setup.exe",
      },
    },
  });
  const server = createServer((request, response) => {
    releaseServer.requests += 1;
    response.writeHead(200, { "Content-Type": "application/json" });
    response.end(manifest);
  });
  return new Promise((resolve) =>
    server.listen(0, "127.0.0.1", () => {
      releaseServer.url = `http://127.0.0.1:${server.address().port}/latest.json`;
      releaseServer.close = () => server.close();
      resolve();
    }),
  );
}

/** A workspace with one host (the test server, whose port its folder
 * sets, with a role of the workspace's own), a host whose role is not
 * defined, and one malformed file. */
function createWorkspace() {
  mkdirSync(join(workspace, "Lab"), { recursive: true });
  writeFileSync(
    join(workspace, "_roles.yaml"),
    'lab_router:\n  name: Lab router\n  icon: router\n  color: "#ff8800"\n',
  );
  writeFileSync(
    join(workspace, "odd.yaml"),
    "address: 192.0.2.9\nrole: mainframe\n",
  );
  writeFileSync(
    join(workspace, "Lab", "_folder.yaml"),
    `description: Lab\nport: ${SSH_PORT}\n`,
  );
  writeFileSync(
    join(workspace, "Lab", "smoke-r1.yaml"),
    "name: smoke-r1\naddress: 127.0.0.1\nrole: lab_router\n",
  );
  writeFileSync(join(workspace, "broken.yaml"), "name: [\n");
  mkdirSync(appData);
  // JSON is valid YAML, and quotes the Windows path safely.
  writeFileSync(
    join(appData, "state.yaml"),
    JSON.stringify({ last_workspace: workspace }),
  );
}

/** OpenSSH's SHA-256 fingerprint of a public key line. */
function fingerprint(keyLine) {
  const blob = Buffer.from(keyLine.split(/\s+/)[1], "base64");
  const digest = createHash("sha256").update(blob).digest("base64");
  return `SHA256:${digest.replace(/=+$/, "")}`;
}

/**
 * The rows on screen of the session `slot` selects, read from xterm.js's
 * buffer (the WebGL renderer draws no text into the page).
 */
function screenOf(evaluate, slot) {
  return evaluate(`(() => {
    const terminal = document.querySelector(${JSON.stringify(slot + " .terminal")})?.xterm;
    if (!terminal) return "";
    const buffer = terminal.buffer.active;
    let text = "";
    for (let y = buffer.viewportY; y < buffer.viewportY + terminal.rows; y++) {
      const line = buffer.getLine(y);
      if (!line) break;
      // A long line wrapped onto the next row reads as one.
      const separator = y === buffer.viewportY || line.isWrapped ? "" : String.fromCharCode(10);
      text += separator + line.translateToString(!buffer.getLine(y + 1)?.isWrapped);
    }
    return text;
  })()`);
}

function fail(message) {
  throw new Error(message);
}

async function until(description, probe) {
  const deadline = Date.now() + TIMEOUT_MS;
  let last;
  while (Date.now() < deadline) {
    try {
      last = await probe();
      if (last) return last;
    } catch (error) {
      last = error;
    }
    await sleep(100);
  }
  fail(`timed out waiting for ${description} (last: ${String(last)})`);
}

function startServer() {
  const build = spawnSync(
    "cargo",
    ["build", "-p", "netterm-core", "--example", "test_ssh_server"],
    { stdio: "inherit" },
  );
  if (build.status !== 0) fail("could not build the test SSH server");
  const server = spawn(
    "target/debug/examples/test_ssh_server.exe",
    [
      String(SSH_PORT),
      "--agent",
      AGENT_PIPE,
      "--old-device",
      String(OLD_DEVICE_PORT),
      "--telnet",
      String(TELNET_PORT),
    ],
    { stdio: ["ignore", "pipe", "inherit"] },
  );
  children.push(server);
  return new Promise((resolve, reject) => {
    server.stdout.once("data", resolve);
    server.once("exit", () => reject(new Error("the test SSH server exited")));
  });
}

function startApp() {
  const app = spawn("target/release/netterm.exe", [], {
    stdio: "inherit",
    env: {
      ...process.env,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${DEVTOOLS_PORT}`,
      WEBVIEW2_USER_DATA_FOLDER: userData,
      NETTERM_DATA_DIR: appData,
      // Credential profiles stay in memory: the smoke test never touches
      // the keychain.
      NETTERM_CREDENTIAL_STORE: "memory",
      // Web links are recorded, not opened in the browser.
      NETTERM_WEB_LINKS_TO: openedLinks,
      // Agent profiles use the test server's agent.
      SSH_AUTH_SOCK: AGENT_PIPE,
      // The update check asks the smoke's own release server.
      NETTERM_UPDATE_ENDPOINT: releaseServer.url,
    },
  });
  children.push(app);
  return app;
}

/** A minimal DevTools protocol client for the app's page. */
async function connectDevTools() {
  const page = await until("the app's webview", async () => {
    const response = await fetch(`http://127.0.0.1:${DEVTOOLS_PORT}/json/list`);
    return (await response.json()).find((target) => target.type === "page");
  });
  const socket = new WebSocket(page.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.addEventListener("open", resolve);
    socket.addEventListener("error", reject);
  });
  let nextId = 0;
  const pending = new Map();
  socket.addEventListener("message", (event) => {
    const message = JSON.parse(event.data);
    const waiter = pending.get(message.id);
    if (!waiter) return;
    pending.delete(message.id);
    if (message.error) waiter.reject(new Error(message.error.message));
    else waiter.resolve(message.result);
  });
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = ++nextId;
      pending.set(id, { resolve, reject });
      socket.send(JSON.stringify({ id, method, params }));
    });
  return { send, close: () => socket.close() };
}

async function main() {
  createWorkspace();
  rmSync(PORTABLE_MARKER, { force: true });
  writeFileSync(INSTALLED_MARKER, "");
  await startReleaseServer();
  await startServer();
  const app = startApp();
  const devtools = await connectDevTools();

  const evaluate = async (expression) => {
    const { result, exceptionDetails } = await devtools.send(
      "Runtime.evaluate",
      { expression, returnByValue: true },
    );
    if (exceptionDetails) {
      const detail = exceptionDetails.exception?.description ?? "";
      fail(`page error: ${exceptionDetails.text} ${detail}`);
    }
    return result.value;
  };
  const text = (selector) =>
    evaluate(
      `document.querySelector(${JSON.stringify(selector)})?.textContent ?? ""`,
    );
  // The session the keyboard goes to (the active pane's); others run in
  // other tabs and panes.
  const shownSession = SHOWN_SESSION;
  const screen = () => screenOf(evaluate, shownSession);
  const status = () => text(`${shownSession} [role=status]`);
  // Clicks the button on screen with this label (other tabs have theirs).
  const click = (label) =>
    evaluate(
      `[...document.querySelectorAll("button")].find((b) => b.textContent.trim() === ${JSON.stringify(label)} && b.checkVisibility()).click()`,
    );
  const type = async (keys) => {
    for (const char of keys) {
      const key =
        char === "\r"
          ? { key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 }
          : { key: char };
      await devtools.send("Input.dispatchKeyEvent", {
        type: "keyDown",
        text: char,
        ...key,
      });
      await devtools.send("Input.dispatchKeyEvent", { type: "keyUp", ...key });
    }
  };
  // Fills fields of the first form matching `form`, then submits it.
  const fillForm = (fields, form = "form") =>
    evaluate(`(() => {
      const fields = ${JSON.stringify(fields)};
      const form = document.querySelector(${JSON.stringify(form)});
      for (const [name, value] of Object.entries(fields)) {
        const input = form.querySelector("[name=" + name + "]");
        input.value = value;
        input.dispatchEvent(new Event("input", { bubbles: true }));
      }
      form.requestSubmit();
    })()`);
  // Types into a side-panel field and commits it, as leaving the field does.
  const changeField = (selector, value) =>
    evaluate(`(() => {
      const input = document.querySelector(".side " + ${JSON.stringify(selector)});
      input.value = ${JSON.stringify(value)};
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
  const quickConnect = async (password) => {
    await until("the quick-connect form", () =>
      evaluate(`!!document.querySelector("input[name=host]")`),
    );
    await fillForm({
      host: "127.0.0.1",
      port: String(SSH_PORT),
      username: USERNAME,
      password,
    });
  };
  // Scrolled into view first: a long tree can hide the element.
  const centreOf = (selector) =>
    evaluate(`(() => {
      const element = document.querySelector(${JSON.stringify(selector)});
      element.scrollIntoView({ block: "nearest" });
      const box = element.getBoundingClientRect();
      return { x: box.x + box.width / 2, y: box.y + box.height / 2 };
    })()`);
  const mouse = (type, { x, y }, extra = {}) =>
    devtools.send("Input.dispatchMouseEvent", { type, x, y, ...extra });
  // A real double-click, through the mouse, in the middle of the element.
  const doubleClick = async (selector) => {
    const { x, y } = await centreOf(selector);
    for (const clickCount of [1, 2]) {
      for (const type of ["mousePressed", "mouseReleased"]) {
        await devtools.send("Input.dispatchMouseEvent", {
          type,
          x,
          y,
          button: "left",
          clickCount,
        });
      }
    }
  };
  // Waits for the host-key question of the given kind ("first-use" or
  // "changed") and returns its text and fingerprints.
  const hostKeyCheck = (kind) =>
    until(`the ${kind} host-key question`, () =>
      evaluate(`(() => {
        const card = document.querySelector("[data-host-key=${kind}]");
        const shown = (which) =>
          card.querySelector("[data-fingerprint=" + which + "]")?.textContent;
        return card && !card.querySelector("button:disabled") && {
          text: card.textContent,
          presented: shown("presented"),
          stored: shown("stored"),
        };
      })()`),
    );
  const decideHostKey = (decision) =>
    evaluate(`document.querySelector("[data-decision=${decision}]").click()`);
  const step = (message) => console.log(`ok - ${message}`);
  // Set SMOKE_SCREENSHOTS to a folder to keep pictures of the window.
  const screenshot = async (name) => {
    const folder = process.env.SMOKE_SCREENSHOTS;
    if (!folder) return;
    const { data } = await devtools.send("Page.captureScreenshot");
    writeFileSync(join(folder, `${name}.png`), Buffer.from(data, "base64"));
  };

  // The app launches and shows its name.
  const name = await until("the window title", () =>
    evaluate("document.title"),
  );
  const heading = await until("the app name on screen", () => text("h1"));
  if (heading !== name) fail(`heading ${heading} differs from title ${name}`);
  await screenshot("quick-connect");
  step(`app launched, named "${name}"`);

  // The last opened workspace reopens: its tree is shown, and the malformed
  // file is reported by name without stopping the rest from loading.
  await until("the reopened workspace's tree", () =>
    evaluate(
      `[...document.querySelectorAll("nav button")].some((b) => b.textContent.includes("Lab"))`,
    ),
  );
  await until("the malformed file reported", async () =>
    (await text("details")).includes("broken.yaml"),
  );
  step("the last workspace reopened, with the malformed file reported");

  await updateCheck({ evaluate, text, screenshot, step });

  // The first connection asks about the host key, showing its type and
  // fingerprint; trusting it once connects.
  await quickConnect(PASSWORD);
  const firstUse = await hostKeyCheck("first-use");
  if (!/ssh-ed25519/.test(firstUse.text) || !firstUse.presented) {
    fail(
      `the host-key question lacks the type or fingerprint: ${firstUse.text}`,
    );
  }
  await screenshot("host-key-first-use");
  await decideHostKey("trustOnce");
  // Quick-connect opens an interactive shell.
  await until(
    "the Connected status",
    async () => (await status()) === "Connected",
  );
  await until("the shell prompt", async () =>
    (await screen()).includes("test-ssh$"),
  );
  step("quick-connect opened a shell and the prompt appeared");

  // Typing reaches the shell and its output is shown.
  await type("size\r");
  const sizeBefore = await until(
    "the output of `size`",
    async () => (await screen()).match(/size\s*(\d+x\d+)/)?.[1],
  );
  step(`typing works; the remote terminal is ${sizeBefore}`);

  // Colours are rendered: "down" arrives bold, "up" in 256-colour green (46).
  await type("colors\r");
  await until("coloured output", () =>
    evaluate(`(() => {
      // From the buffer: the WebGL renderer draws no text into the page.
      const buffer = document.querySelector("${SHOWN_SESSION} .terminal").xterm.buffer.active;
      const cells = [];
      for (let y = 0; y < buffer.length; y++) {
        const line = buffer.getLine(y);
        let word = "";
        let first = null;
        for (let x = 0; x <= line.length; x++) {
          const cell = x < line.length ? line.getCell(x) : null;
          const char = cell?.getChars() ?? "";
          if (char && char !== " ") {
            word += char;
            first ??= cell;
          } else if (word) {
            cells.push({ word, cell: first });
            word = "";
            first = null;
          }
        }
      }
      const has = (text, test) => cells.some(({ word, cell }) => word === text && test(cell));
      return has("down", (cell) => cell.isBold()) &&
        has("up", (cell) => cell.isFgPalette() && cell.getFgColor() === 46);
    })()`),
  );
  await screenshot("session");
  step("colours are rendered");

  // A full-screen program's output leaves the main screen intact.
  await type("fullscreen\r");
  await until("the prompt after a full-screen program", async () =>
    /fullscreen\s*test-ssh\$/.test(await screen()),
  );
  step("full-screen program output is handled");

  // Resizing the window resizes the remote terminal.
  const { windowId, bounds } = await devtools.send(
    "Browser.getWindowForTarget",
  );
  await devtools.send("Browser.setWindowBounds", {
    windowId,
    bounds: { width: bounds.width - 300, height: bounds.height - 200 },
  });
  await type("size\r");
  const sizeAfter = await until("a new size from `size`", async () => {
    const sizes = [...(await screen()).matchAll(/size\s*(\d+x\d+)/g)];
    const latest = sizes.at(-1)?.[1];
    return latest && latest !== sizeBefore ? latest : null;
  });
  step(`resizing the window resized the remote terminal to ${sizeAfter}`);

  // Output larger than one message arrives complete and in order.
  await type("lines 3000\r");
  await until("the end of a long listing", async () =>
    /line 2999\s*line 3000\s*test-ssh\$/.test(await screen()),
  );
  step("a long listing arrives complete");

  // Output far beyond the core's flow-control window keeps flowing, so the
  // interface acknowledges what the terminal consumes.
  await type("lines 200000\r");
  await until("the end of a 2.5 MB listing", async () =>
    /line 199999\s*line 200000\s*test-ssh\$/.test(await screen()),
  );
  step("a 2.5 MB listing arrives complete");

  // A flood of output can be stopped with Ctrl+C.
  await type("flood\r");
  await until("the flood", async () => (await screen()).includes("flood"));
  await sleep(1000);
  const ctrlC = {
    key: "c",
    code: "KeyC",
    windowsVirtualKeyCode: 67,
    modifiers: 2,
  };
  await devtools.send("Input.dispatchKeyEvent", { type: "keyDown", ...ctrlC });
  await devtools.send("Input.dispatchKeyEvent", { type: "keyUp", ...ctrlC });
  await until("the prompt after Ctrl+C", async () =>
    /\^C\s*test-ssh\$\s*$/.test(await screen()),
  );
  step("Ctrl+C stops a flood of output and the prompt returns");

  // The remote end exiting is reflected in the interface.
  await type("exit\r");
  await until("the closed status", async () =>
    (await status()).startsWith("Closed: the remote end closed the session"),
  );
  step(`remote exit is shown: "${await status()}"`);

  // Closing the session from the interface is reflected too. A key trusted
  // once is asked about again; this time it is remembered, in the app's
  // data folder.
  await click("New connection");
  await quickConnect(PASSWORD);
  const again = await hostKeyCheck("first-use");
  if (again.presented !== firstUse.presented) {
    fail(
      `the fingerprint changed: ${again.presented} vs ${firstUse.presented}`,
    );
  }
  await decideHostKey("trustAndRemember");
  const knownHosts = join(appData, "known_hosts");
  await until("the remembered host key", () =>
    readFileSync(knownHosts, "utf8").startsWith(
      `[127.0.0.1]:${SSH_PORT} ssh-ed25519 `,
    ),
  );
  step("the first connection asked about the host key; it was remembered");
  await until(
    "the Connected status",
    async () => (await status()) === "Connected",
  );

  // A dropped connection keeps the terminal and its scrollback, says why,
  // and Enter reconnects it in place.
  await type("lines 3\r");
  await until("output before the drop", async () =>
    /line 3\s*test-ssh\$/.test(await screen()),
  );
  await type("drop\r");
  await until(
    "the lost connection",
    async () => (await status()) === "Closed: connection lost",
  );
  await until("the reconnect hint", async () =>
    (await text(`${shownSession} [data-session-ended]`)).includes(
      "Press Enter to reconnect",
    ),
  );
  if (!(await screen()).includes("line 3")) {
    fail("the scrollback is gone after the connection dropped");
  }
  await screenshot("connection-lost");
  await type("\r");
  await until(
    "the reconnected session",
    async () => (await status()) === "Connected",
  );
  await until("a new prompt after the reconnect marker", async () =>
    /reconnecting ---\s*test-ssh\$/.test(await screen()),
  );
  if (!(await screen()).includes("line 3")) {
    fail("the scrollback is gone after reconnecting");
  }
  step("a dropped session said why and reconnected in place with Enter");

  // Duplicate opens a second session to the same host in a tab beside the
  // first.
  const switchTo = (index) =>
    evaluate(`document.querySelectorAll("[role=tab]")[${index}].click()`);
  await click("Duplicate");
  await until("two sessions", () =>
    evaluate(`document.querySelectorAll(".slot[data-tab]").length === 2`),
  );
  await until(
    "the duplicate connected",
    async () => (await status()) === "Connected",
  );
  await until("the duplicate's prompt", async () =>
    (await screen()).includes("test-ssh$"),
  );
  if ((await screen()).includes("line 3")) {
    fail("the duplicate shows the first session's output");
  }
  await type("size\r");
  await until("the duplicate's own output", async () =>
    /size\s*\d+x\d+/.test(await screen()),
  );
  await screenshot("duplicate");
  await switchTo(0);
  await until("the first session again", async () =>
    (await screen()).includes("line 3"),
  );
  if ((await status()) !== "Connected") {
    fail(`the first session is not connected: ${await status()}`);
  }
  // Closing the duplicate leaves the first session running.
  await switchTo(1);
  await until(
    "the duplicate again",
    async () => !(await screen()).includes("line 3"),
  );
  // The first session's buttons are there too, out of sight.
  const clickShown = (label) =>
    evaluate(
      `[...document.querySelectorAll("${shownSession} button")].find((b) => b.textContent.trim() === ${JSON.stringify(label)}).click()`,
    );
  await clickShown("Disconnect");
  await until("the duplicate closed", async () =>
    (await status()).startsWith("Closed"),
  );
  await clickShown("New connection");
  await until("one session left", () =>
    evaluate(`document.querySelectorAll(".slot[data-tab]").length === 1`),
  );
  await switchTo(0);
  await until(
    "the first session still connected",
    async () => (await status()) === "Connected",
  );
  step("Duplicate opened a second session to the same host, each on its own");

  await click("Disconnect");
  await until(
    "the closed status",
    async () => (await status()) === "Closed: session closed",
  );
  step("disconnecting is shown");

  // A wrong password is reported.
  await click("New connection");
  await quickConnect("wrong");
  await until(
    "the authentication failure",
    async () => (await status()) === "Closed: authentication failed",
  );
  step("a wrong password is reported (the remembered key was not asked about)");

  // A port nobody listens on is reported as refused.
  await click("New connection");
  await until("the quick-connect form", () =>
    evaluate(`!!document.querySelector("input[name=host]")`),
  );
  await fillForm({
    host: "127.0.0.1",
    port: "1",
    username: USERNAME,
    password: PASSWORD,
  });
  await until(
    "the refused connection",
    async () =>
      (await status()) === "Closed: 127.0.0.1 refused the connection on port 1",
  );
  step(`a refused connection is reported: "${await status()}"`);

  // A changed key blocks the connection, shows both fingerprints, and goes
  // on only when the user replaces the stored key.
  const realKeyLine = readFileSync(knownHosts, "utf8");
  const otherKey = readFileSync(OTHER_KEY_FILE, "utf8").trim();
  writeFileSync(knownHosts, `[127.0.0.1]:${SSH_PORT} ${otherKey}\n`);
  await click("New connection");
  await quickConnect(PASSWORD);
  const changed = await hostKeyCheck("changed");
  if (
    changed.stored !== fingerprint(otherKey) ||
    changed.presented !== firstUse.presented
  ) {
    fail(`the changed-key warning shows ${JSON.stringify(changed)}`);
  }
  await screenshot("host-key-changed");
  await decideHostKey("cancel");
  await until("the refused connection", async () =>
    (await status()).includes("host key has changed"),
  );
  if (!readFileSync(knownHosts, "utf8").includes(otherKey.split(" ")[1])) {
    fail("cancelling changed the stored key");
  }
  await click("New connection");
  await quickConnect(PASSWORD);
  await hostKeyCheck("changed");
  await decideHostKey("trustAndRemember");
  await until(
    "the Connected status",
    async () => (await status()) === "Connected",
  );
  if (readFileSync(knownHosts, "utf8") !== realKeyLine) {
    fail("replacing the key did not store the server's key alone");
  }
  await click("Disconnect");
  await until("the closed status", async () =>
    (await status()).startsWith("Closed"),
  );
  step("a changed host key was blocked, then replaced on request");

  // Double-clicking a host in the tree prompts for credentials and opens an
  // SSH session to its management address, on the port its folder sets.
  await evaluate(
    `[...document.querySelectorAll("nav button[aria-expanded]")].find((b) => b.textContent.includes("Lab")).click()`,
  );
  const hostButton = '[data-host="Lab/smoke-r1.yaml"]';
  await until("the host in the tree", () =>
    evaluate(`!!document.querySelector(${JSON.stringify(hostButton)})`),
  );
  await doubleClick(hostButton);
  await until("the credentials prompt", async () =>
    (await text("main h2")).includes("smoke-r1"),
  );
  await until("the inherited port in the prompt", async () =>
    (await text("main p")).includes(`127.0.0.1:${SSH_PORT}`),
  );
  await fillForm({ username: USERNAME, password: PASSWORD });
  await until(
    "the Connected status",
    async () => (await status()) === "Connected",
  );
  await until("the shell prompt", async () =>
    (await screen()).includes("test-ssh$"),
  );
  await screenshot("host-session");
  step("double-clicking a host opened a session to it on its folder's port");

  // Sessions are logged as plain text under the data folder's logs (the
  // app's per-user log folder when NETTERM_DATA_DIR is not set): by
  // workspace, tree path and host, and quick connections by address.
  await type("colors\r");
  await until("coloured output in the host session", async () =>
    (await screen()).includes("down up"),
  );
  const logFolder = join(appData, "logs");
  const hostLogs = join(
    logFolder,
    "workspaces",
    "smoke-workspace",
    "Lab",
    "smoke-r1",
  );
  const hostLog = await until("the host session's log file", () => {
    if (!existsSync(hostLogs)) return null;
    const files = readdirSync(hostLogs);
    return files.length === 1 &&
      /^\d{4}-\d\d-\d\d_\d\d-\d\d-\d\d\.log$/.test(files[0])
      ? join(hostLogs, files[0])
      : null;
  });
  await until("the command and its output in the log", () =>
    readFileSync(hostLog, "utf8").includes("test-ssh$ colors\ndown up\n"),
  );
  if (readFileSync(hostLog, "utf8").includes("\x1b")) {
    fail("the session log holds terminal control sequences");
  }
  const quickLogs = readdirSync(join(logFolder, "quick connect", "127.0.0.1"));
  if (quickLogs.length < 3) {
    fail(`expected a log per quick connection, found ${quickLogs.join(", ")}`);
  }
  if (!(await text("[data-setting-note=logging]")).includes("secrets")) {
    fail("the logging setting does not warn that logs can contain secrets");
  }
  if (!(await evaluate(`!!document.querySelector("[data-open-logs]")`))) {
    fail("the side panel has no action to open the host's log folder");
  }
  step("sessions were logged as plain text by workspace, path, host and time");

  // Each host shows its role's icon and colour in the tree, and so does its
  // session's header; a role the workspace does not define shows neutrally.
  const roleIcon = (selector) =>
    evaluate(`(() => {
      const icon = document.querySelector(${JSON.stringify(selector + " [data-role-icon]")});
      return icon && [icon.dataset.roleIcon, getComputedStyle(icon).color];
    })()`);
  const orange = "rgb(255, 136, 0)";
  const expectIcon = async (where, selector, icon, colour) => {
    const shown = await roleIcon(selector);
    if (shown?.[0] !== icon || (colour && shown[1] !== colour)) {
      fail(`${where}: expected the ${icon} icon, got ${JSON.stringify(shown)}`);
    }
  };
  await expectIcon("tree", hostButton, "router", orange);
  await expectIcon(
    "session header",
    `${SHOWN_SESSION} .session header`,
    "router",
    orange,
  );
  if (
    !(await text(`${SHOWN_SESSION} .session header`)).includes("Lab router")
  ) {
    fail("the session header does not name the host's role");
  }
  await expectIcon("undefined role", '[data-host="odd.yaml"]', "unknown");
  step("roles showed their icon and colour in the tree and the session header");

  await searchAndFilter({
    evaluate,
    text,
    click,
    fillForm,
    type,
    devtools,
    screenshot,
    step,
  });

  await liveReload({ evaluate, text, screenshot, step });

  await credentialProfiles({
    evaluate,
    text,
    click,
    fillForm,
    changeField,
    doubleClick,
    status,
    screen,
    screenshot,
    step,
  });

  await agentAndKeyboardInteractive({
    evaluate,
    text,
    click,
    fillForm,
    changeField,
    doubleClick,
    status,
    screen,
    screenshot,
    step,
  });

  await legacyCrypto({
    evaluate,
    text,
    click,
    fillForm,
    changeField,
    doubleClick,
    hostKeyCheck,
    decideHostKey,
    status,
    screen,
    screenshot,
    step,
  });

  await telnet({
    evaluate,
    text,
    click,
    type,
    doubleClick,
    status,
    screen,
    screenshot,
    step,
  });

  await serial({
    evaluate,
    text,
    click,
    fillForm,
    doubleClick,
    status,
    screenshot,
    step,
  });

  await editInventory({
    evaluate,
    text,
    click,
    fillForm,
    changeField,
    centreOf,
    mouse,
    devtools,
    screenshot,
    step,
  });

  await tabsAndPanes({
    evaluate,
    fillForm,
    centreOf,
    mouse,
    devtools,
    screenshot,
    step,
  });

  await terminalComforts({
    evaluate,
    centreOf,
    mouse,
    devtools,
    type,
    screen,
    screenshot,
    step,
  });

  await highlighting({ evaluate, fillForm, type, screenshot, step });

  await snippets({
    evaluate,
    fillForm,
    doubleClick,
    text,
    status,
    screen,
    screenshot,
    step,
  });

  await pasteSafety({
    evaluate,
    fillForm,
    doubleClick,
    text,
    status,
    screen,
    devtools,
    screenshot,
    step,
  });

  await webLinks({ evaluate, text, screenshot, step });

  await csvImport({ evaluate, fillForm, text, screenshot, step });

  await nornirImport({ evaluate, fillForm, text, screenshot, step });

  await workspaces({
    evaluate,
    fillForm,
    doubleClick,
    text,
    status,
    screen,
    devtools,
    screenshot,
    step,
  });

  devtools.close();

  // The restarted app is a portable copy, with the update check off.
  const requestsBeforeRestart = releaseServer.requests;
  rmSync(INSTALLED_MARKER, { force: true });
  writeFileSync(PORTABLE_MARKER, "smoke test\n");

  // Preferences survive a restart: the app comes back light, with the
  // font size and scrollback chosen.
  app.kill();
  await new Promise((resolve) =>
    app.exitCode === null ? app.once("exit", resolve) : resolve(),
  );
  await sleep(1500);
  startApp();
  const restarted = await connectDevTools();
  const evaluateAgain = async (expression) => {
    const { result, exceptionDetails } = await restarted.send(
      "Runtime.evaluate",
      {
        expression,
        returnByValue: true,
      },
    );
    if (exceptionDetails) fail(`page error: ${exceptionDetails.text}`);
    return result.value;
  };
  await until("the light theme after a restart", () =>
    evaluateAgain(`document.documentElement.dataset.theme === "light"`),
  );
  await evaluateAgain(
    `document.querySelector("[data-open-preferences]").click()`,
  );
  const kept = await until("the preferences dialog after a restart", () =>
    evaluateAgain(`(() => {
      const dialog = document.querySelector("[data-dialog=preferences]");
      const field = (name) => dialog?.querySelector("[name=" + name + "]");
      return dialog && {
        theme: field("preference-theme").value,
        family: field("preference-font-family").value,
        size: field("preference-font-size").value,
        scrollback: field("preference-scrollback").value,
        copyOnSelect: field("preference-copy-on-select").checked,
        rightClickPaste: field("preference-right-click-paste").checked,
        confirmMultiLinePaste: field("preference-confirm-multi-line-paste").checked,
      };
    })()`),
  );
  const expected = {
    theme: "light",
    family: "Consolas",
    size: "15",
    scrollback: "2000",
    copyOnSelect: true,
    rightClickPaste: true,
    confirmMultiLinePaste: false,
  };
  if (JSON.stringify(kept) !== JSON.stringify(expected)) {
    fail(`preferences after a restart: ${JSON.stringify(kept)}`);
  }
  step("the preferences were kept across a restart");

  // The known workspaces survive it too, and the last one opened reopens.
  await until(
    "the smoke workspace reopened",
    async () =>
      (await evaluateAgain(
        `document.querySelector("aside header .name")?.textContent.trim()`,
      )) === "smoke-workspace",
  );
  await evaluateAgain(
    `document.querySelector("[data-open-workspaces]").click()`,
  );
  const listed = await until("the switcher after a restart", () =>
    evaluateAgain(`(() => {
      const rows = [...document.querySelectorAll("[data-workspace]")];
      return rows.length > 0 && rows.map((row) => row.querySelector(".name").textContent);
    })()`),
  );
  if (
    JSON.stringify(listed) !== JSON.stringify(["smoke-workspace", "customer-b"])
  ) {
    fail(`the switcher lists after a restart: ${JSON.stringify(listed)}`);
  }
  step("the known workspaces were kept across a restart");

  await portableUpdateCheck({
    evaluate: evaluateAgain,
    requestsBeforeRestart,
    step,
  });
  restarted.close();
}

/**
 * The update check at start: the installed build (an uninstaller beside it) finds
 * the newer release and offers it, installs nothing until asked (and this
 * build, not made by the release script, refuses to), and the check is then
 * turned off in the preferences.
 */
async function updateCheck({ evaluate, text, screenshot, step }) {
  const notice = await until("the update notice", () =>
    evaluate(`(() => {
      const notice = document.querySelector("[data-update]");
      return notice && {
        action: notice.dataset.update,
        version: notice.dataset.updateVersion,
        install: !!notice.querySelector("[data-update-install]"),
      };
    })()`),
  );
  if (
    notice.action !== "install" ||
    notice.version !== NEWER_VERSION ||
    !notice.install
  ) {
    fail(`the update notice: ${JSON.stringify(notice)}`);
  }
  if (releaseServer.requests !== 1) {
    fail(`the release server was asked ${releaseServer.requests} times`);
  }
  await screenshot("update-offered");
  step(`at start, the installed build offered version ${NEWER_VERSION}`);

  await evaluate(`document.querySelector("[data-update-install]").click()`);
  const refused = await until("the install refused", () =>
    text("[data-update-error]"),
  );
  if (!/cannot install updates/.test(refused)) {
    fail(`installing in a development build: ${refused}`);
  }
  step("installing waits for a click, and a non-release build refuses it");

  await evaluate(`document.querySelector("[data-update-later]").click()`);
  await until("the notice closed", () =>
    evaluate(`!document.querySelector("[data-update]")`),
  );

  await evaluate(`document.querySelector("[data-open-preferences]").click()`);
  const before = await until("the update preference", () =>
    evaluate(`(() => {
      const box = document.querySelector("[name=preference-check-for-updates]");
      const status = document.querySelector("[data-update-status]");
      return box && { checked: box.checked, status: status.dataset.updateStatus };
    })()`),
  );
  if (!before.checked || before.status !== "available") {
    fail(`the update preference at first: ${JSON.stringify(before)}`);
  }
  await evaluate(
    `document.querySelector("[name=preference-check-for-updates]").click()`,
  );
  await until("the update check saved as off", () =>
    readFileSync(join(appData, "preferences.yaml"), "utf8").includes(
      "check_for_updates: false",
    ),
  );
  await evaluate(
    `document.querySelector("[data-dialog=preferences] .actions button").click()`,
  );
  await until("the preferences closed", () =>
    evaluate(`!document.querySelector("[data-dialog=preferences]")`),
  );
  step("the update check was turned off in the preferences");
}

/**
 * After the restart, as a portable copy with the check off: no request at
 * start; "Check now" asks once and only announces the release, with its
 * download page (recorded, never opened in a browser) and no install.
 */
async function portableUpdateCheck({ evaluate, requestsBeforeRestart, step }) {
  await evaluate(`(() => {
    if (!document.querySelector("[data-dialog=preferences]"))
      document.querySelector("[data-open-preferences]").click();
  })()`);
  const status = await until("the update status after a restart", () =>
    evaluate(`(() => {
      const status = document.querySelector("[data-update-status]");
      return status?.dataset.updateStatus && {
        kind: status.dataset.updateStatus,
        text: status.textContent,
      };
    })()`),
  );
  if (status.kind !== "disabled" || !/portable/.test(status.text)) {
    fail(`the update status after a restart: ${JSON.stringify(status)}`);
  }
  // Give a stray request time to arrive.
  await sleep(1500);
  if (releaseServer.requests !== requestsBeforeRestart) {
    fail("the app asked the release server on its own with the check off");
  }
  step("with the check off, the app made no request at start");

  await evaluate(`document.querySelector("[data-update-check-now]").click()`);
  const notice = await until("the portable copy's notice", () =>
    evaluate(`(() => {
      const notice = document.querySelector("[data-update]");
      return notice && {
        action: notice.dataset.update,
        install: !!notice.querySelector("[data-update-install]"),
        page: !!notice.querySelector("[data-update-page]"),
      };
    })()`),
  );
  if (notice.action !== "notify" || notice.install || !notice.page) {
    fail(`the portable copy's notice: ${JSON.stringify(notice)}`);
  }
  if (releaseServer.requests !== requestsBeforeRestart + 1) {
    fail(
      `"Check now" asked ${releaseServer.requests - requestsBeforeRestart} times`,
    );
  }
  await evaluate(`document.querySelector("[data-update-page]").click()`);
  await until(
    "the download page opened",
    () =>
      existsSync(openedLinks) &&
      readFileSync(openedLinks, "utf8").includes(RELEASE_PAGE),
  );
  step("the portable copy only announced the release, with its download page");
}

/**
 * Keyword highlighting: the built-in rules colour what the shown session
 * prints (typed text echoed a character at a time included), the screen's
 * text and the log stay as they were, the preference turns it off, and a
 * workspace rule is added, turned off and broken from the app and on disk.
 */
async function highlighting({ evaluate, fillForm, type, screenshot, step }) {
  const json = (value) => JSON.stringify(value);
  const rulesFile = join(workspace, "_highlights.yaml");
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  // The last row holding `mark`, its text, the cells coloured on it, and
  // the terminal's palette colours the rules below name.
  const row = (mark) =>
    evaluate(`(() => {
      const slot = document.querySelector("${SHOWN_SESSION} .terminal");
      const terminal = slot.xterm;
      const buffer = terminal.buffer.active;
      const theme = terminal.options.theme;
      for (let y = buffer.length - 1; y >= 0; y--) {
        const text = buffer.getLine(y).translateToString(true);
        if (!text.includes(${json(mark)})) continue;
        const cells = slot.highlights.shown().filter((cell) => cell.line === y);
        return {
          text,
          cells,
          palette: {
            red: theme.red,
            cyan: theme.cyan,
            "bright-blue": theme.brightBlue,
            magenta: theme.magenta,
          },
        };
      }
      return null;
    })()`);
  // Whether the row holding `mark` has exactly `expected` ([word, colour]).
  const coloured = async (mark, expected) => {
    const found = await row(mark);
    if (!found) return false;
    const want = expected
      .map(([word, color]) => ({
        col: found.text.indexOf(word),
        width: word.length,
        color: found.palette[color],
      }))
      .sort((a, b) => a.col - b.col);
    const got = found.cells
      .map(({ col, width, color }) => ({ col, width, color }))
      .sort((a, b) => a.col - b.col);
    return json(got) === json(want) || Promise.reject(json(got));
  };
  const setPreference = (name, value) =>
    evaluate(`(() => {
      const field = document.querySelector("[data-dialog=preferences] [name=${name}]");
      field.checked = ${json(value)};
      field.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
  const closeDialog = async (dialog) => {
    await evaluate(
      `[...document.querySelectorAll("[data-dialog=${dialog}] button")].find((b) => b.textContent.trim() === "Close").click()`,
    );
    await until(
      `the ${dialog} dialog closed`,
      async () => !(await exists(`[data-dialog=${dialog}]`)),
    );
  };
  const openRules = async () => {
    // From the preferences, which name the rules beside the switch.
    await evaluate(`document.querySelector("[data-open-preferences]").click()`);
    await until("the preferences dialog", () =>
      exists("[data-dialog=preferences]"),
    );
    await evaluate(
      `document.querySelector("[data-dialog=preferences] [data-open-highlights]").click()`,
    );
    await until("the highlights dialog", () =>
      exists("[data-dialog=highlights]"),
    );
  };
  const focusTerminal = () =>
    evaluate(
      `document.querySelector("${SHOWN_SESSION} .xterm textarea").focus()`,
    );

  // Built-in rules: an interface, a state, IPv4 and IPv6 addresses. The
  // echo arrives a character at a time; the whole words are still found.
  const line = "hlcheck Gi0/1 is down via 10.0.0.1 and 2001:db8::1";
  const builtIn = [
    ["Gi0/1", "bright-blue"],
    ["down", "red"],
    ["10.0.0.1", "cyan"],
    ["2001:db8::1", "cyan"],
  ];
  await focusTerminal();
  await type(`${line}\r`);
  await until("the built-in highlights", () => coloured("hlcheck", builtIn));
  const shown = await row("hlcheck");
  if (!shown.text.endsWith(line)) {
    fail(`the screen's text changed: ${shown.text}`);
  }
  await screenshot("highlighting");
  step("the built-in rules highlighted an interface, a state and addresses");

  // The log holds the line as it came, highlighting or not.
  const logs = [];
  const walk = (dir) => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) walk(path);
      else if (entry.name.endsWith(".log")) logs.push(path);
    }
  };
  await until("the highlighted line in a session log", () => {
    logs.length = 0;
    walk(join(appData, "logs"));
    const seen = logs.flatMap((log) =>
      readFileSync(log, "utf8")
        .split("\n")
        .filter((row) => row.includes("hlcheck")),
    );
    // Plain text, as typed (the prompt may sit on the line before it).
    if (seen.some((row) => row.endsWith(line) && !row.includes("\x1b"))) {
      return true;
    }
    throw new Error(`${logs.length} logs; lines: ${JSON.stringify(seen)}`);
  });
  step("the session log holds the line unchanged");

  // Turned off in the preferences: nothing is coloured; on again: back.
  await evaluate(`document.querySelector("[data-open-preferences]").click()`);
  await until("the preferences dialog", () =>
    exists("[data-dialog=preferences]"),
  );
  await setPreference("preference-highlighting", false);
  await until("no highlights", () => coloured("hlcheck", []));
  await until("highlighting off saved", () =>
    readFileSync(join(appData, "preferences.yaml"), "utf8").includes(
      "highlighting: false",
    ),
  );
  await setPreference("preference-highlighting", true);
  await until("the highlights back", () => coloured("hlcheck", builtIn));
  await closeDialog("preferences");
  step("the preference turned highlighting off and on again");

  // A workspace rule, added in the app, is saved to the workspace.
  await openRules();
  await evaluate(`document.querySelector("[data-rule=new]").click()`);
  await until("the rule form", () => exists("[data-rule-form]"));
  await evaluate(`(() => {
    const select = document.querySelector("[data-rule-form] [name=rule-color]");
    select.value = "magenta";
    select.dispatchEvent(new Event("change", { bubbles: true }));
  })()`);
  await fillForm(
    { "rule-name": "smoke-word", "rule-pattern": "SMOKEWORD" },
    "[data-rule-form]",
  );
  await until(
    "the rule in the workspace",
    () =>
      existsSync(rulesFile) &&
      readFileSync(rulesFile, "utf8") ===
        "smoke-word:\n  pattern: SMOKEWORD\n  color: magenta\n",
  );
  await until("the rule listed", () =>
    exists("[data-highlight-rule=smoke-word][data-highlight-source=workspace]"),
  );
  await closeDialog("highlights");
  await focusTerminal();
  await type("hlrule SMOKEWORD\r");
  await until("the workspace rule's highlight", () =>
    coloured("hlrule", [["SMOKEWORD", "magenta"]]),
  );
  step("a rule added in the app was saved to the workspace and applied");

  // One rule turned off: only its highlights go.
  await openRules();
  await evaluate(`(() => {
    const box = document.querySelector("[data-highlight-rule=smoke-word] [name=rule-enabled]");
    box.checked = false;
    box.dispatchEvent(new Event("change", { bubbles: true }));
  })()`);
  await until("the rule saved as off", () =>
    readFileSync(rulesFile, "utf8").includes("enabled: false"),
  );
  await until("the rule's highlight gone", () => coloured("hlrule", []));
  await until("the other highlights kept", () => coloured("hlcheck", builtIn));
  step("turning one rule off removed only its highlights");

  // A broken pattern written on disk is reported and skipped; the other
  // rules keep working.
  writeFileSync(
    rulesFile,
    "smoke-word:\n  pattern: SMOKEWORD\n  color: magenta\nbroken:\n  pattern: 'Gi('\n  color: red\n",
  );
  await until("the broken rule reported", () =>
    exists("[data-highlight-rule=broken] [data-rule-error]"),
  );
  await until("the rule's highlight back", () =>
    coloured("hlrule", [["SMOKEWORD", "magenta"]]),
  );
  await until("the built-in highlights kept", () =>
    coloured("hlcheck", builtIn),
  );
  await screenshot("highlight-rules");
  await closeDialog("highlights");
  rmSync(rulesFile);
  step("a broken pattern was reported and skipped; the other rules work");
}

/**
 * Snippets: one is created in the panel and saved to the workspace, sent
 * to a host session from the panel and from the palette (its host's
 * fields filled in, the other variable asked for, line by line), edited,
 * deleted, and one written on disk shows in the panel.
 */
async function snippets({
  evaluate,
  fillForm,
  doubleClick,
  text,
  status,
  screen,
  screenshot,
  step,
}) {
  const json = (value) => JSON.stringify(value);
  const file = join(workspace, "_snippets.yaml");
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) =>
    evaluate(`document.querySelector(${json(selector)}).click()`);
  const fileText = () => (existsSync(file) ? readFileSync(file, "utf8") : "");

  // A session to an inventory host (added on disk: earlier parts deleted
  // the others), whose fields fill the variables.
  const hostFile = join(workspace, "snip-r1.yaml");
  writeFileSync(
    hostFile,
    `address: 127.0.0.1\nport: ${SSH_PORT}\nsite: SMK1\n`,
  );
  const hostButton = '[data-host="snip-r1.yaml"]';
  await until("the host in the tree", () => exists(hostButton));
  await doubleClick(hostButton);
  await until("the credentials prompt", async () =>
    (await text("main h2")).includes("snip-r1"),
  );
  await fillForm({ username: USERNAME, password: PASSWORD });
  await until(
    "the Connected status",
    async () => (await status()) === "Connected",
  );
  await until("the shell prompt", async () =>
    (await screen()).includes("test-ssh$"),
  );

  // Created in the panel, saved to the workspace's snippets file.
  await clickOn("[data-snippet-new]");
  await until("the snippet form", () => exists("[data-snippet-form]"));
  await fillForm(
    {
      "snippet-name": "Smoke check",
      "snippet-group": "Smoke",
      "snippet-text": "snipA {{address}} {{name}} {{site}}\nsnipB {{vlan}}\n",
    },
    "[data-snippet-form]",
  );
  await until(
    "the snippet in the workspace",
    () =>
      fileText() ===
      "Smoke check:\n  group: Smoke\n  text: |\n    snipA {{address}} {{name}} {{site}}\n    snipB {{vlan}}\n",
  );
  await until("the snippet in the panel", () =>
    exists('[data-snippet-group=Smoke] [data-snippet="Smoke check"]'),
  );
  step("a snippet created in the panel was saved to the workspace");

  // Sent from the panel: the host's address, name and site are filled in,
  // the unknown variable is asked for, and each line arrives as its own.
  const sent = async (vlan) => {
    const shown = await screen();
    const lines = `test-ssh$ snipA 127.0.0.1 snip-r1 SMK1\ntest-ssh$ snipB ${vlan}\ntest-ssh$`;
    if (shown.includes(lines)) return true;
    const error = await text(".edit-error");
    throw new Error(`${json(shown.trimEnd().slice(-300))} ${error}`);
  };
  const answer = async (vlan) => {
    const asked = await until("the variables asked for", () =>
      evaluate(`(() => {
        const dialog = document.querySelector("[data-dialog=snippet-variables]");
        return dialog && [...dialog.querySelectorAll("[data-variable]")].map((i) => i.dataset.variable);
      })()`),
    );
    if (json(asked) !== json(["vlan"])) fail(`asked for ${json(asked)}`);
    await fillForm(
      { "variable-0": vlan },
      "[data-dialog=snippet-variables] form",
    );
  };
  // Two lines: the paste confirmation shows them first (on by default).
  const confirmLines = async (vlan) => {
    const shown = await until("the paste confirmation", () =>
      evaluate(`(() => {
        const card = document.querySelector("${SHOWN_SESSION} [data-paste-confirm]");
        return card && {
          environment: card.dataset.pasteConfirm,
          lines: card.querySelector("[data-paste-lines]").dataset.pasteLines,
          text: card.querySelector("[data-paste-text]").textContent,
        };
      })()`),
    );
    const expected = {
      environment: "none",
      lines: "2",
      text: `snipA 127.0.0.1 snip-r1 SMK1\nsnipB ${vlan}\n`,
    };
    if (json(shown) !== json(expected)) {
      fail(`the paste confirmation shows ${json(shown)}`);
    }
    await clickOn(`${SHOWN_SESSION} [data-paste=send]`);
  };
  await clickOn('[data-snippet="Smoke check"]');
  await answer("42");
  await confirmLines("42");
  await until("the snippet's lines in the session", () => sent("42"));
  await screenshot("snippet-sent");
  step(
    "a snippet sent from the panel filled the host's fields, asked the rest",
  );

  // Sent from the palette: `>` searches the snippets, Enter sends.
  await evaluate(
    `[...document.querySelectorAll("button")].find((b) => b.title === "Search hosts (Ctrl+K)").click()`,
  );
  await until("the palette", () => exists("input[name=palette-query]"));
  await evaluate(`(() => {
    const input = document.querySelector("input[name=palette-query]");
    input.value = ">smoke";
    input.dispatchEvent(new Event("input", { bubbles: true }));
  })()`);
  await until("the snippet in the palette", () =>
    exists('[data-snippet-hit="Smoke check"]'),
  );
  await evaluate(
    `document.querySelector("input[name=palette-query]").dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }))`,
  );
  await answer("43");
  await confirmLines("43");
  await until("the snippet's lines sent from the palette", () => sent("43"));
  step("a snippet sent from the command palette");

  // Edited (renamed) and deleted from the panel.
  await clickOn('[data-snippet-edit="Smoke check"]');
  await until("the snippet form", () => exists("[data-snippet-form]"));
  await fillForm({ "snippet-name": "Smoke renamed" }, "[data-snippet-form]");
  await until("the renamed snippet in the workspace", () =>
    fileText().startsWith("Smoke renamed:\n  group: Smoke\n"),
  );
  await clickOn('[data-snippet-delete="Smoke renamed"]');
  await until("the delete confirmation", () =>
    evaluate(
      `[...document.querySelectorAll("[role=dialog] button")].some((b) => b.textContent.trim() === "Delete")`,
    ),
  );
  await evaluate(
    `[...document.querySelectorAll("[role=dialog] button")].find((b) => b.textContent.trim() === "Delete").click()`,
  );
  await until("the snippets file gone", () => !existsSync(file));
  await until(
    "the snippet gone from the panel",
    async () => !(await exists("[data-snippet]")),
  );
  step("a snippet was renamed and deleted from the panel");

  // Written on disk (a teammate's, from git): shown in the panel.
  writeFileSync(file, "From disk: show clock\n");
  await until("the snippet from disk in the panel", () =>
    exists('[data-snippet="From disk"]'),
  );
  rmSync(file);
  await until(
    "the snippet from disk gone",
    async () => !(await exists("[data-snippet]")),
  );
  step("a snippet written on disk showed in the panel");
  rmSync(hostFile);
}

/**
 * Web links: links set on a folder and on the host are offered beside its
 * ways in, filled from its fields; clicking one hands the URL to the
 * browser (recorded in `openedLinks` here, through NETTERM_WEB_LINKS_TO),
 * and a link that is not http/https, or lacks a field, is refused with a
 * message and nothing opened.
 */
async function webLinks({ evaluate, text, screenshot, step }) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) =>
    evaluate(`document.querySelector(${json(selector)}).click()`);
  const opened = () =>
    existsSync(openedLinks)
      ? readFileSync(openedLinks, "utf8").split("\n").filter(Boolean)
      : [];
  const hostFile = join(workspace, "Firewalls", "fw-r1.yaml");
  const link = (name) => `.side [data-web-link="${name}"]`;
  const error = () => text(".side [data-web-link-error]");

  mkdirSync(join(workspace, "Firewalls"), { recursive: true });
  writeFileSync(
    join(workspace, "Firewalls", "_folder.yaml"),
    [
      "web_links:",
      "  GUI: https://{{address}}:8443/",
      "  Explorer: 'file:///C:/Windows/explorer.exe'",
      "  Controller: https://{{custom.controller}}/",
      "",
    ].join("\n"),
  );
  writeFileSync(hostFile, "address: 192.0.2.10\n");
  const folderRow = 'nav button[data-item=folder][data-path="Firewalls"]';
  await until("the Firewalls folder in the tree", () => exists(folderRow));
  await evaluate(`(() => {
    const row = document.querySelector(${json(folderRow)});
    if (row.getAttribute("aria-expanded") !== "true") row.click();
  })()`);
  const hostRow = 'nav [data-host="Firewalls/fw-r1.yaml"]';
  await until("fw-r1 in the tree", () => exists(hostRow));
  await clickOn(hostRow);
  await until("fw-r1 in the side panel", async () =>
    (await text(".side h2")).includes("fw-r1"),
  );

  // The CLI ways in and the web links, side by side.
  const waysIn = () =>
    evaluate(`[...document.querySelectorAll(".side [data-ways-in] button")].map(
      (b) => b.dataset.connect ?? "link:" + b.dataset.webLink + (b.dataset.refused === undefined ? "" : " (refused)"),
    ).join(", ")`);
  const expectedWays =
    "default, telnet, serial, link:GUI, link:Explorer (refused), link:Controller (refused)";
  await until(
    `the ways in to be ${expectedWays}`,
    async () => (await waysIn()) === expectedWays,
  );
  const title = await evaluate(
    `document.querySelector(${json(link("GUI"))}).title`,
  );
  if (title !== "Open https://192.0.2.10:8443/ in the browser") {
    fail(`the GUI link's title: ${title}`);
  }
  await screenshot("web-links");
  step("the side panel offers SSH, telnet, serial and the web links together");

  // A click opens the filled URL in the browser.
  await clickOn(link("GUI"));
  await until(
    "the GUI link opened",
    () => json(opened()) === json(["https://192.0.2.10:8443/"]),
  );
  step("clicking a web link opens its filled URL in the browser");

  // Not http/https, or a field without a value: refused with a message.
  await clickOn(link("Explorer"));
  await until("the refused file: link's message", async () =>
    (await error()).includes(
      "Explorer was not opened: only http and https links are opened, not file",
    ),
  );
  await clickOn(link("Controller"));
  await until("the refused Controller link's message", async () =>
    (await error()).includes(
      "Controller was not opened: the host has no value for custom.controller",
    ),
  );
  await sleep(300);
  if (opened().length !== 1) fail(`refused links were opened: ${opened()}`);
  step("links that are not http/https, or lack a field, are refused");

  // A link added on the host from the side panel, then the host's own GUI
  // link replacing the folder's (in its place) and a field filling the
  // controller's.
  await evaluate(`(() => {
    const form = document.querySelector(".side form:has([name=new-link-name])");
    for (const [name, value] of Object.entries({
      "new-link-name": "Grafana",
      "new-link-url": "https://grafana.example.net/d/{{name}}",
    })) {
      const input = form.querySelector("[name=" + name + "]");
      input.value = value;
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }
    form.requestSubmit();
  })()`);
  await until("the Grafana link on the host", () => exists(link("Grafana")));
  if (!readFileSync(hostFile, "utf8").includes("Grafana:")) {
    fail(`the host file lacks the link: ${readFileSync(hostFile, "utf8")}`);
  }
  await clickOn(link("Grafana"));
  await until(
    "the Grafana link opened",
    () => opened().at(-1) === "https://grafana.example.net/d/fw-r1",
  );
  writeFileSync(
    hostFile,
    [
      "address: 192.0.2.10",
      "web_links:",
      "  GUI: https://{{address}}/login",
      "  Grafana: https://grafana.example.net/d/{{name}}",
      "custom:",
      "  controller: ctl.example.net",
      "",
    ].join("\n"),
  );
  const merged =
    "default, telnet, serial, link:GUI, link:Explorer (refused), link:Controller, link:Grafana";
  await until(
    `the ways in to be ${merged}`,
    async () => (await waysIn()) === merged,
  );
  await clickOn(link("GUI"));
  await until(
    "the host's own GUI link opened",
    () => opened().at(-1) === "https://192.0.2.10/login",
  );
  await clickOn(link("Controller"));
  await until(
    "the Controller link opened",
    () => opened().at(-1) === "https://ctl.example.net/",
  );
  step("host links merge with the folder's and are filled from its fields");

  // The page is never shown inside the app.
  if (await exists("iframe, webview, embed, object")) {
    fail("a web page is embedded in the app");
  }
  step("no web page is embedded in the app");
}

/**
 * Multiple workspaces: the switcher lists the known ones; a new empty
 * workspace is created in a folder that does not exist yet (each workspace
 * gets its own identifier file); switching with a live session asks, and
 * "keep" leaves the session in its tab with its own role; switching back
 * by typing in the switcher; "close" closes the session; removing a
 * workspace from the list leaves its folder, and it can be added again as
 * an existing folder; creating over a non-empty folder is refused. Ends
 * with the smoke workspace open again.
 */
/**
 * CSV import: a spreadsheet saved by a European Excel (semicolons, a
 * byte-order mark, quoted values) is mapped, previewed, imported into the
 * top of the workspace with one existing host confirmed, then imported again
 * without confirming anything.
 */
async function csvImport({ evaluate, fillForm, text, screenshot, step }) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) =>
    evaluate(`document.querySelector(${json(selector)}).click()`);
  const dialog = "[data-dialog=csv-import]";
  const atStep = (name) => exists(`${dialog}[data-step=${name}]`);
  const csvFile = join(userData, "inventory.csv");
  writeFileSync(
    csvFile,
    "\uFEFF" +
      [
        "Hostname;Management IP;Vendor;Folder;Tags;Password;Owner",
        'imp-1;10.9.0.1;Arista;Europe/Madrid;"core, dc1";hunter2;noc',
        "imp-2;10.9.0.2;Cisco;Europe/Paris;;;",
        "odd;192.0.2.99;Juniper;;;;",
        ";;;;;;",
        "bad:name;10.9.0.5;;;;;",
        "",
      ].join("\r\n"),
  );
  const oddFile = join(workspace, "odd.yaml");
  const oddBefore = readFileSync(oddFile, "utf8");

  const readFile = async () => {
    await clickOn("[data-open-csv-import]");
    await until("the CSV import dialog", () => atStep("file"));
    await fillForm({ "csv-file": csvFile }, `${dialog} form`);
    await until("the column mapping", () => atStep("map"));
  };
  const choices = () =>
    evaluate(
      `[...document.querySelectorAll("${dialog} select[name^=csv-column-]")].map((s) => s.value).join(" ")`,
    );
  const preview = async () => {
    await evaluate(`(() => {
      const target = document.querySelector("${dialog} select[name=csv-target]");
      target.value = "";
      target.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
    await clickOn(`${dialog} [data-csv=preview]`);
    await until("the import preview", () => atStep("preview"));
  };
  const outcomes = () =>
    evaluate(`[...document.querySelectorAll("${dialog} [data-preview-row]")].map(
      (row) => row.dataset.previewRow + ":" + row.dataset.outcome,
    ).join(" ")`);
  const counts = () =>
    evaluate(`[...document.querySelectorAll("${dialog} [data-report]")].map(
      (section) => section.dataset.report + "=" + section.dataset.count,
    ).join(" ")`);
  const runImport = async () => {
    await clickOn(`${dialog} [data-csv=import]`);
    await until("the import report", () => atStep("report"));
  };

  // The file is read with its byte-order mark and semicolons, and the
  // columns are mapped from their headers; the password is never mapped.
  await readFile();
  const suggested =
    "field:name field:address field:vendor folderPath tags ignore custom";
  await until(
    `the suggested mapping ${suggested}`,
    async () => (await choices()) === suggested,
  );
  await screenshot("csv-mapping");
  step("a semicolon CSV with a byte-order mark is read and its columns mapped");

  // The preview shows every row's fate and writes nothing.
  await preview();
  const expected = "2:create 3:create 4:exists 5:skip 6:fail";
  await until(
    `the preview rows ${expected}`,
    async () => (await outcomes()) === expected,
  );
  if (!(await exists(`${dialog} [data-preview-skipped-column="Password"]`))) {
    fail("the preview does not say the password column is left out");
  }
  if (
    !(await text(`${dialog} [data-preview-folders]`)).includes("Europe/Madrid")
  ) {
    fail("the preview does not list the new folders");
  }
  if (existsSync(join(workspace, "Europe"))) fail("the preview wrote folders");
  await screenshot("csv-preview");
  step(
    "the preview lists new, existing, empty and failing rows, writing nothing",
  );

  // Importing with the existing host confirmed: folders made under the
  // target, the confirmed host updated, the report complete.
  await clickOn(`${dialog} [data-replace="odd.yaml"]`);
  await until("the Import button counting 3 hosts", async () =>
    /Import\s+3\s+hosts/.test(await text(`${dialog} [data-csv=import]`)),
  );
  await runImport();
  const first = "created=2 updated=1 skipped=2 failed=1";
  await until(`the report ${first}`, async () => (await counts()) === first);
  const failed = await text(`${dialog} [data-report=failed] li`);
  if (!failed.includes("Row 6")) fail(`the failed row: ${failed}`);
  await screenshot("csv-report");
  const madrid = readFileSync(
    join(workspace, "Europe", "Madrid", "imp-1.yaml"),
    "utf8",
  );
  const expectedMadrid =
    "address: 10.9.0.1\nvendor: Arista\ntags:\n- core\n- dc1\ncustom:\n  Owner: noc\n";
  if (madrid !== expectedMadrid) fail(`imp-1.yaml holds ${json(madrid)}`);
  if (!existsSync(join(workspace, "Europe", "Paris", "imp-2.yaml"))) {
    fail("imp-2 was not created in Europe/Paris");
  }
  const odd = readFileSync(oddFile, "utf8");
  if (
    !odd.includes("address: 192.0.2.99\n") ||
    !odd.includes("vendor: Juniper\n") ||
    !odd.includes("role: mainframe\n")
  ) {
    fail(`the confirmed odd.yaml holds ${json(odd)} (was ${json(oddBefore)})`);
  }
  await clickOn(`${dialog} [data-csv=close]`);
  await until("the dialog closed", async () => !(await exists(dialog)));
  await until("the imported folder in the tree", () =>
    exists('nav button[data-item=folder][data-path="Europe"]'),
  );
  step(
    "importing creates folders under the target, replaces only the confirmed host and reports every row",
  );

  // Again, confirming nothing: every host exists now and none is written.
  const madridBefore = readFileSync(
    join(workspace, "Europe", "Madrid", "imp-1.yaml"),
  );
  writeFileSync(oddFile, oddBefore);
  await readFile();
  await preview();
  await until(
    "every host existing",
    async () =>
      (await outcomes()) === "2:exists 3:exists 4:exists 5:skip 6:fail",
  );
  await runImport();
  const second = "created=0 updated=0 skipped=5 failed=1";
  await until(`the report ${second}`, async () => (await counts()) === second);
  if (readFileSync(oddFile, "utf8") !== oddBefore)
    fail("odd.yaml was overwritten");
  if (
    !readFileSync(join(workspace, "Europe", "Madrid", "imp-1.yaml")).equals(
      madridBefore,
    )
  ) {
    fail("imp-1.yaml was overwritten");
  }
  await clickOn(`${dialog} [data-csv=close]`);
  await until("the dialog closed", async () => !(await exists(dialog)));
  step("a re-import overwrites no existing host without confirmation");
}

/**
 * Nornir import: a simple inventory (hosts, groups and defaults files) into
 * the Europe folder the CSV import made: inherited values, groups as tags,
 * data as custom fields, passwords never written, and a re-import that
 * overwrites nothing.
 */
async function nornirImport({ evaluate, fillForm, text, screenshot, step }) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) =>
    evaluate(`document.querySelector(${json(selector)}).click()`);
  const dialog = "[data-dialog=nornir-import]";
  const atStep = (name) => exists(`${dialog}[data-step=${name}]`);
  const files = {
    "nornir-hosts": join(userData, "nornir-hosts.yaml"),
    "nornir-groups": join(userData, "nornir-groups.yaml"),
    "nornir-defaults": join(userData, "nornir-defaults.yaml"),
  };
  writeFileSync(
    files["nornir-hosts"],
    [
      "---",
      "nr-1:",
      "  hostname: 10.8.0.1",
      "  groups: [core]",
      "  password: NornirHostPass",
      "  data:",
      "    owner: neteng",
      "nr-2:",
      "  hostname: 10.8.0.2",
      "  port: 2201",
      "broken:",
      "  platform: eos",
      "",
    ].join("\n"),
  );
  writeFileSync(
    files["nornir-groups"],
    ["core:", "  port: 830", "  data:", "    layer: core", ""].join("\n"),
  );
  writeFileSync(
    files["nornir-defaults"],
    [
      "platform: ios",
      "username: automation",
      "password: NornirDefaultPass",
      "",
    ].join("\n"),
  );
  const nr1 = join(workspace, "Europe", "nr-1.yaml");
  const nr2 = join(workspace, "Europe", "nr-2.yaml");

  const outcomes = () =>
    evaluate(`[...document.querySelectorAll("${dialog} [data-preview-host]")].map(
      (row) => row.dataset.previewHost + ":" + row.dataset.outcome,
    ).join(" ")`);
  const counts = () =>
    evaluate(`[...document.querySelectorAll("${dialog} [data-report]")].map(
      (section) => section.dataset.report + "=" + section.dataset.count,
    ).join(" ")`);
  const preview = async () => {
    await clickOn("[data-open-nornir-import]");
    await until("the Nornir import dialog", () => atStep("files"));
    await evaluate(`(() => {
      const target = document.querySelector("${dialog} select[name=nornir-target]");
      target.value = "Europe";
      target.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
    await fillForm(files, `${dialog} form`);
    await until("the Nornir preview", () => atStep("preview"));
  };
  const runImport = async () => {
    await clickOn(`${dialog} [data-nornir=import]`);
    await until("the Nornir report", () => atStep("report"));
  };
  const close = async () => {
    await clickOn(`${dialog} [data-nornir=close]`);
    await until("the dialog closed", async () => !(await exists(dialog)));
  };

  // The preview: each host's fate, the passwords left out, nothing written.
  await preview();
  const expected = "nr-1:create nr-2:create broken:fail";
  await until(
    `the Nornir preview ${expected}`,
    async () => (await outcomes()) === expected,
  );
  for (const skipped of ["host nr-1: password", "defaults: password"]) {
    if (
      !(await exists(`${dialog} [data-preview-skipped-entry="${skipped}"]`))
    ) {
      fail(`the preview does not list ${skipped} as not imported`);
    }
  }
  if (existsSync(nr1)) fail("the Nornir preview wrote a host");
  await screenshot("nornir-preview");
  step(
    "a Nornir inventory's preview lists every host and the passwords left out",
  );

  // Importing: inherited values, tags and custom fields; no password.
  await runImport();
  const first = "created=2 updated=0 skipped=3 failed=1";
  await until(`the report ${first}`, async () => (await counts()) === first);
  await screenshot("nornir-report");
  const nr1Text = readFileSync(nr1, "utf8");
  const expectedNr1 =
    "address: 10.8.0.1\nport: 830\nplatform: ios\ntags:\n- core\ncustom:\n  owner: neteng\n  layer: core\n";
  if (nr1Text !== expectedNr1) fail(`nr-1.yaml holds ${json(nr1Text)}`);
  const nr2Text = readFileSync(nr2, "utf8");
  if (nr2Text !== "address: 10.8.0.2\nport: 2201\nplatform: ios\n") {
    fail(`nr-2.yaml holds ${json(nr2Text)}`);
  }
  for (const secret of ["NornirHostPass", "NornirDefaultPass", "automation"]) {
    if (nr1Text.includes(secret) || nr2Text.includes(secret)) {
      fail(`${secret} was written to the workspace`);
    }
  }
  await close();
  await until("an imported Nornir host in the tree", () =>
    exists('nav [data-host="Europe/nr-1.yaml"]'),
  );
  step(
    "importing writes inherited values, group tags and data fields, never passwords",
  );

  // Again: both hosts exist now, nothing is confirmed, nothing is written.
  writeFileSync(nr2, "address: 10.8.0.2\ndescription: edited by hand\n");
  const nr2Before = readFileSync(nr2, "utf8");
  await preview();
  await until(
    "both Nornir hosts existing",
    async () => (await outcomes()) === "nr-1:exists nr-2:exists broken:fail",
  );
  await runImport();
  const second = "created=0 updated=0 skipped=5 failed=1";
  await until(`the report ${second}`, async () => (await counts()) === second);
  if (readFileSync(nr1, "utf8") !== expectedNr1)
    fail("nr-1.yaml was overwritten");
  if (readFileSync(nr2, "utf8") !== nr2Before)
    fail("nr-2.yaml was overwritten");
  await close();
  step("a Nornir re-import overwrites no existing host without confirmation");
}

async function workspaces({
  evaluate,
  fillForm,
  doubleClick,
  text,
  status,
  screen,
  devtools,
  screenshot,
  step,
}) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) =>
    evaluate(`document.querySelector(${json(selector)}).click()`);
  const openName = () => text("aside header .name");
  const second = join(userData, "customers", "customer-b");
  const idOf = (folder) =>
    readFileSync(join(folder, "_workspace.yaml"), "utf8").match(
      /^id: (\S+)$/m,
    )?.[1];
  const known = () =>
    evaluate(`[...document.querySelectorAll("[data-workspace]")].map(
      (row) => row.dataset.workspace + (row.dataset.workspaceOpen === undefined ? "" : " (open)"),
    )`);
  const openSwitcher = async () => {
    await clickOn("[data-open-workspaces]");
    await until("the workspace switcher", () =>
      exists("[data-dialog=workspaces] [data-workspace]"),
    );
  };
  const escape = { key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 };

  // A live session to a host of the smoke workspace, with its role.
  writeFileSync(
    join(workspace, "ws-r1.yaml"),
    `address: 127.0.0.1\nport: ${SSH_PORT}\nrole: lab_router\n`,
  );
  const hostButton = '[data-host="ws-r1.yaml"]';
  await until("the host in the tree", () => exists(hostButton));
  await doubleClick(hostButton);
  await until("the credentials prompt", async () =>
    (await text("main h2")).includes("ws-r1"),
  );
  await fillForm({ username: USERNAME, password: PASSWORD });
  await until(
    "the Connected status",
    async () => (await status()) === "Connected",
  );
  await until("the shell prompt", async () =>
    (await screen()).includes("test-ssh$"),
  );
  const tab = await evaluate(
    `document.querySelector("${SHOWN_SESSION}").dataset.tab`,
  );
  const slot = `.slot[data-tab="${tab}"]`;
  const sessionRole = () =>
    evaluate(
      `document.querySelector(${json(slot + " .session header [data-role-icon]")})?.dataset.roleIcon ?? null`,
    );

  // The switcher lists the open workspace; an identifier file was given to
  // it when it opened.
  await openSwitcher();
  if (json(await known()) !== json([`${workspace} (open)`])) {
    fail(`the switcher lists ${json(await known())}`);
  }
  if (!idOf(workspace)) fail("the smoke workspace has no identifier file");
  await screenshot("workspace-switcher");

  // A new workspace in a folder that is not there yet: the live session
  // is asked about first, and kept.
  await fillForm(
    { "workspace-folder": second },
    "[data-workspace-form=create]",
  );
  await until("the keep-or-close question", () =>
    exists("[data-confirm-switch]"),
  );
  // Earlier parts left sessions to its hosts open too.
  if (
    !/\d+ sessions? to hosts of smoke-workspace/.test(
      await text("[data-confirm-switch]"),
    )
  ) {
    fail(`the question says: ${await text("[data-confirm-switch]")}`);
  }
  await screenshot("workspace-switch-question");
  await clickOn("[data-switch=keep]");
  await until(
    "the new workspace open",
    async () => (await openName()) === "customer-b",
  );
  if (await exists("[data-dialog=workspaces]")) {
    fail("the switcher stayed open after switching");
  }
  if (!idOf(second) || idOf(second) === idOf(workspace)) {
    fail(`identifiers: ${idOf(second)} and ${idOf(workspace)}`);
  }
  if (await exists("nav [data-host]")) fail("the new workspace is not empty");
  if (!(await exists(slot))) fail("the kept session's tab is gone");
  if ((await text(`${slot} [role=status]`)) !== "Connected") {
    fail(`the kept session is ${await text(`${slot} [role=status]`)}`);
  }
  // Its host's role comes from its own workspace, which defines it.
  if ((await sessionRole()) !== "router") {
    fail(`the kept session's role icon is ${await sessionRole()}`);
  }
  step(
    "a new empty workspace was created and opened; the live session was kept",
  );

  // Back by typing in the switcher: nothing of customer-b is open, so
  // nothing is asked.
  await openSwitcher();
  if (json(await known()) !== json([`${second} (open)`, workspace])) {
    fail(`the switcher lists ${json(await known())}`);
  }
  await evaluate(`(() => {
    const input = document.querySelector("[name=workspace-query]");
    input.value = "smoke-work";
    input.dispatchEvent(new Event("input", { bubbles: true }));
  })()`);
  await until(
    "only the smoke workspace listed",
    async () => json(await known()) === json([workspace]),
  );
  await devtools.send("Input.dispatchKeyEvent", {
    type: "keyDown",
    key: "Enter",
    code: "Enter",
    windowsVirtualKeyCode: 13,
  });
  await until(
    "the smoke workspace open again",
    async () => (await openName()) === "smoke-workspace",
  );
  if (await exists("[data-confirm-switch]")) {
    fail("switching asked with no session of the open workspace live");
  }
  if (!(await exists(slot))) fail("the kept session's tab is gone");
  step("the switcher switched back on Enter, without asking");

  // Leaving again, closing the session this time.
  await openSwitcher();
  await clickOn(`[data-workspace=${json(second)}]`);
  await until("the keep-or-close question", () =>
    exists("[data-confirm-switch]"),
  );
  await clickOn("[data-switch=close]");
  await until(
    "customer-b open",
    async () => (await openName()) === "customer-b",
  );
  await until("the session's tab closed", async () => !(await exists(slot)));
  step("switching with a live session closed it when asked to");

  // Removed from the list, the smoke workspace keeps its folder; added
  // again as an existing folder, it opens.
  await openSwitcher();
  await clickOn(`[data-workspace=${json(workspace)}] [data-workspace-remove]`);
  await until(
    "the smoke workspace off the list",
    async () => json(await known()) === json([`${second} (open)`]),
  );
  if (!existsSync(join(workspace, "ws-r1.yaml"))) {
    fail("removing the workspace from the list deleted its files");
  }
  if (
    readFileSync(join(appData, "state.yaml"), "utf8").includes(
      "smoke-workspace",
    )
  ) {
    fail("the removed workspace is still in the app state");
  }
  // Creating over a folder with files in it is refused.
  await fillForm(
    { "workspace-folder": workspace },
    "[data-workspace-form=create]",
  );
  await until("the refusal", async () =>
    (await text("[data-workspace-error]")).includes("not empty"),
  );
  await fillForm(
    { "workspace-folder": workspace },
    "[data-workspace-form=add]",
  );
  await until(
    "the smoke workspace added and open",
    async () => (await openName()) === "smoke-workspace",
  );
  await until("the smoke workspace's hosts", () => exists(hostButton));
  const state = readFileSync(join(appData, "state.yaml"), "utf8");
  if (!state.includes("smoke-workspace") || !state.includes("customer-b")) {
    fail(`the app state lists: ${state}`);
  }
  step("a workspace removed from the list kept its folder and was added back");

  // The shortcut opens the switcher too; Escape closes it.
  const ctrlShiftO = {
    key: "O",
    code: "KeyO",
    windowsVirtualKeyCode: 79,
    modifiers: 2 | 8,
  };
  await devtools.send("Input.dispatchKeyEvent", {
    type: "keyDown",
    ...ctrlShiftO,
  });
  await devtools.send("Input.dispatchKeyEvent", {
    type: "keyUp",
    ...ctrlShiftO,
  });
  await until("the switcher from Ctrl+Shift+O", () =>
    exists("[data-dialog=workspaces]"),
  );
  await devtools.send("Input.dispatchKeyEvent", { type: "keyDown", ...escape });
  await devtools.send("Input.dispatchKeyEvent", { type: "keyUp", ...escape });
  await until(
    "the switcher closed",
    async () => !(await exists("[data-dialog=workspaces]")),
  );
  step("Ctrl+Shift+O opens the workspace switcher");
}

/**
 * Environment marking and paste safety: a production host's session has a
 * red tab and border; with the multi-line confirmation turned off, a
 * multi-line paste or snippet into it still asks (showing the text and line
 * count; Cancel sends nothing), and its lines go out the folder's paste
 * line delay apart; a lab host's session is marked lab and, with the
 * confirmation off, takes a multi-line paste at once.
 */
async function pasteSafety({
  evaluate,
  fillForm,
  doubleClick,
  text,
  status,
  screen,
  devtools,
  screenshot,
  step,
}) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) =>
    evaluate(`document.querySelector(${json(selector)}).click()`);
  const CTRL = 2;
  const SHIFT = 8;
  const pasteKeys = async () => {
    const event = {
      key: "V",
      code: "KeyV",
      windowsVirtualKeyCode: 86,
      modifiers: CTRL | SHIFT,
    };
    await devtools.send("Input.dispatchKeyEvent", {
      type: "keyDown",
      ...event,
    });
    await devtools.send("Input.dispatchKeyEvent", { type: "keyUp", ...event });
  };
  const preferencesFile = join(appData, "preferences.yaml");
  // `later 0 <text>` prints the text, before or after the next prompt.
  const ran = async (marker) =>
    new RegExp(`(^|\\n|\\$ )${marker}\\s*(\\n|$)`).test(await screen());
  const confirmation = () =>
    evaluate(`(() => {
      const card = document.querySelector("${SHOWN_SESSION} [data-paste-confirm]");
      return card && {
        environment: card.dataset.pasteConfirm,
        lines: card.querySelector("[data-paste-lines]").dataset.pasteLines,
        text: card.querySelector("[data-paste-text]").textContent.replace(/\\r\\n/g, "\\n"),
      };
    })()`);
  const connectTo = async (id, name) => {
    const folder = id.includes("/") ? id.slice(0, id.lastIndexOf("/")) : "";
    if (folder) {
      const row = `nav button[data-item=folder][data-path="${folder}"]`;
      await until(`the ${folder} folder in the tree`, () => exists(row));
      await evaluate(`(() => {
        const row = document.querySelector(${json(row)});
        if (row.getAttribute("aria-expanded") !== "true") row.click();
      })()`);
    }
    const button = `[data-host="${id}"]`;
    await until(`${name} in the tree`, () => exists(button));
    await doubleClick(button);
    await until("the credentials prompt", async () =>
      (await text("main h2")).includes(name),
    );
    await fillForm({ username: USERNAME, password: PASSWORD });
    await until(
      "the Connected status",
      async () => (await status()) === "Connected",
    );
    await until("the shell prompt", async () =>
      (await screen()).includes("test-ssh$"),
    );
  };

  mkdirSync(join(workspace, "Prod"), { recursive: true });
  writeFileSync(
    join(workspace, "Prod", "_folder.yaml"),
    "environment: production\npaste_line_delay: 400\n",
  );
  writeFileSync(
    join(workspace, "Prod", "prod-r1.yaml"),
    `address: 127.0.0.1\nport: ${SSH_PORT}\n`,
  );
  writeFileSync(
    join(workspace, "lab-r9.yaml"),
    `address: 127.0.0.1\nport: ${SSH_PORT}\nenvironment: lab\n`,
  );
  const savedClipboard = readClipboard();
  try {
    // Turned off: from now on only production hosts ask.
    await evaluate(`document.querySelector("[data-open-preferences]").click()`);
    await until("the preferences dialog", () =>
      exists("[data-dialog=preferences]"),
    );
    await evaluate(`(() => {
      const field = document.querySelector("[data-dialog=preferences] [name=preference-confirm-multi-line-paste]");
      field.checked = false;
      field.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
    await until("the confirmation switch saved", () =>
      /confirm_multi_line_paste: false/.test(
        readFileSync(preferencesFile, "utf8"),
      ),
    );
    await evaluate(
      `[...document.querySelectorAll("[data-dialog=preferences] button")].find((b) => b.textContent.trim() === "Close").click()`,
    );
    step("the multi-line paste confirmation was turned off in the preferences");

    // A production session: a red tab with its PROD label, a red frame.
    await connectTo("Prod/prod-r1.yaml", "prod-r1");
    const tab = await evaluate(
      `document.querySelector("${SHOWN_SESSION}").dataset.tab`,
    );
    const marks = await evaluate(`(() => {
      const tab = document.querySelector('[data-tab-bar] [data-tab="${tab}"]');
      const session = document.querySelector("${SHOWN_SESSION} .session");
      const style = getComputedStyle(session);
      return {
        tab: tab.dataset.environment,
        label: !!tab.querySelector("[data-tab-production]"),
        tabBackground: getComputedStyle(tab).backgroundColor,
        badge: session.querySelector("[data-environment-badge]")?.dataset.environmentBadge,
        border: style.borderTopWidth,
        borderColor: style.borderTopColor,
      };
    })()`);
    if (
      marks.tab !== "production" ||
      !marks.label ||
      marks.badge !== "production" ||
      marks.border !== "3px" ||
      !/^rgb\((255, 77, 77|196, 22, 28)\)$/.test(marks.borderColor) ||
      marks.tabBackground === "rgba(0, 0, 0, 0)"
    ) {
      fail(`the production session is not marked: ${json(marks)}`);
    }
    await screenshot("production-session");
    step("a production session has a red tab (PROD) and a red border");

    // A multi-line paste still asks on production; Cancel sends nothing.
    writeClipboard("later 0 prod-one\nlater 0 prod-two\n");
    await pasteKeys();
    const asked = await until(
      "the production paste confirmation",
      confirmation,
    );
    const expected = {
      environment: "production",
      lines: "2",
      text: "later 0 prod-one\nlater 0 prod-two\n",
    };
    if (json(asked) !== json(expected)) {
      fail(`the production paste confirmation shows ${json(asked)}`);
    }
    await screenshot("production-paste");
    await clickOn(`${SHOWN_SESSION} [data-paste=cancel]`);
    await until("the confirmation gone", async () => !(await confirmation()));
    await sleep(800);
    if ((await ran("prod-one")) || (await ran("prod-two"))) {
      fail("a cancelled paste reached the device");
    }
    step(
      "a multi-line paste into production asked even with it turned off; Cancel sent nothing",
    );

    // Confirmed: the lines go out one by one, the folder's 400 ms apart.
    await pasteKeys();
    await until("the production paste confirmation", confirmation);
    const started = Date.now();
    await clickOn(`${SHOWN_SESSION} [data-paste=send]`);
    await until("the first pasted line run", () => ran("prod-one"));
    await until("the second pasted line run", () => ran("prod-two"));
    const took = Date.now() - started;
    if (took < 400) fail(`two lines 400 ms apart arrived in ${took} ms`);
    step(
      `the confirmed paste went out line by line, ${took} ms for two lines (delay 400 ms)`,
    );

    // A multi-line snippet into production goes through the same question.
    const snippetsFile = join(workspace, "_snippets.yaml");
    writeFileSync(
      snippetsFile,
      'Prod lines: "later 0 snip-prod-1\\nlater 0 snip-prod-2\\n"\n',
    );
    await until("the snippet in the panel", () =>
      exists('[data-snippet="Prod lines"]'),
    );
    await clickOn('[data-snippet="Prod lines"]');
    const snippetAsked = await until(
      "the snippet's paste confirmation",
      confirmation,
    );
    if (
      snippetAsked.environment !== "production" ||
      snippetAsked.lines !== "2" ||
      snippetAsked.text !== "later 0 snip-prod-1\nlater 0 snip-prod-2\n"
    ) {
      fail(`the snippet's confirmation shows ${json(snippetAsked)}`);
    }
    await clickOn(`${SHOWN_SESSION} [data-paste=send]`);
    await until(
      "the snippet's lines run",
      async () => (await ran("snip-prod-1")) && (await ran("snip-prod-2")),
    );
    rmSync(snippetsFile);
    step("a multi-line snippet sent to production asked first");

    // A lab session: marked lab, and the paste goes out at once.
    await connectTo("lab-r9.yaml", "lab-r9");
    const labMarks = await evaluate(`(() => {
      const tab = document.querySelector('[data-tab-bar] [data-tab="' + document.querySelector("${SHOWN_SESSION}").dataset.tab + '"]');
      const session = document.querySelector("${SHOWN_SESSION} .session");
      return {
        tab: tab.dataset.environment,
        label: !!tab.querySelector("[data-tab-production]"),
        border: getComputedStyle(session).borderTopWidth,
      };
    })()`);
    if (json(labMarks) !== json({ tab: "lab", label: false, border: "1px" })) {
      fail(`the lab session is marked ${json(labMarks)}`);
    }
    writeClipboard("later 0 lab-one\nlater 0 lab-two\n");
    await pasteKeys();
    await until(
      "the lab paste run",
      async () => (await ran("lab-one")) && (await ran("lab-two")),
    );
    if (await confirmation()) fail("the lab paste asked though turned off");
    await screenshot("lab-session");
    step("a lab session is marked lab and took a multi-line paste at once");
  } finally {
    writeClipboard(savedClipboard);
  }
}

/** Reads the system clipboard's text (independently of the app). */
function readClipboard() {
  const result = spawnSync(
    "powershell",
    ["-NoProfile", "-Command", "[Console]::Out.Write((Get-Clipboard -Raw))"],
    { encoding: "utf8" },
  );
  return result.stdout ?? "";
}

/** Puts `text` on the system clipboard (independently of the app). */
function writeClipboard(text) {
  spawnSync(
    "powershell",
    ["-NoProfile", "-Command", "Set-Clipboard -Value $env:SMOKE_CLIPBOARD"],
    { env: { ...process.env, SMOKE_CLIPBOARD: text } },
  );
}

/**
 * Themes (following the system, then chosen), the terminal font and its
 * size (dialog, shortcuts, Ctrl+scroll), the scrollback length and search,
 * and copy and paste (shortcuts, copy-on-select, right-click paste). Runs
 * in the shown session of the tabs part; the system clipboard is used and
 * restored after.
 */
async function terminalComforts({
  evaluate,
  centreOf,
  mouse,
  devtools,
  type,
  screen,
  screenshot,
  step,
}) {
  const json = (value) => JSON.stringify(value);
  const CTRL = 2;
  const SHIFT = 8;
  const press = async (key, code, windowsVirtualKeyCode, modifiers = 0) => {
    const event = { key, code, windowsVirtualKeyCode, modifiers };
    await devtools.send("Input.dispatchKeyEvent", {
      type: "keyDown",
      ...event,
    });
    await devtools.send("Input.dispatchKeyEvent", { type: "keyUp", ...event });
  };
  const terminal = `document.querySelector("${SHOWN_SESSION} .terminal").xterm`;
  const option = (name) => evaluate(`${terminal}.options.${name}`);
  const pageTheme = () => evaluate("document.documentElement.dataset.theme");
  const preferencesFile = () =>
    existsSync(join(appData, "preferences.yaml"))
      ? readFileSync(join(appData, "preferences.yaml"), "utf8")
      : "";
  const emulateSystem = (scheme) =>
    devtools.send("Emulation.setEmulatedMedia", {
      features: [{ name: "prefers-color-scheme", value: scheme }],
    });
  const openPreferences = async () => {
    await evaluate(`document.querySelector("[data-open-preferences]").click()`);
    await until("the preferences dialog", () =>
      evaluate(`!!document.querySelector("[data-dialog=preferences]")`),
    );
  };
  // Sets a preferences field as the user does, then leaves it (change).
  const setPreference = (name, value) =>
    evaluate(`(() => {
      const field = document.querySelector("[data-dialog=preferences] [name=${name}]");
      if (field.type === "checkbox") field.checked = ${json(value)};
      else field.value = ${json(value)};
      field.dispatchEvent(new Event("input", { bubbles: true }));
      field.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
  const closePreferences = async () => {
    await evaluate(
      `[...document.querySelectorAll("[data-dialog=preferences] button")].find((b) => b.textContent.trim() === "Close").click()`,
    );
    await until(
      "the preferences dialog closed",
      async () =>
        !(await evaluate(
          `!!document.querySelector("[data-dialog=preferences]")`,
        )),
    );
  };
  const savedClipboard = readClipboard();

  try {
    // The theme follows the system by default, live.
    await emulateSystem("light");
    await until(
      "the light theme from the system",
      async () =>
        (await pageTheme()) === "light" &&
        (await option("theme.background")) === "#fbfbfc",
    );
    await screenshot("theme-light");
    await emulateSystem("dark");
    await until(
      "the dark theme from the system",
      async () =>
        (await pageTheme()) === "dark" &&
        (await option("theme.background")) === "#0e1116",
    );
    step("the theme followed the system setting, live");

    // A chosen theme wins over the system's, and is saved.
    await openPreferences();
    await setPreference("preference-theme", "light");
    await until(
      "the light theme chosen",
      async () =>
        (await pageTheme()) === "light" &&
        /theme: light/.test(preferencesFile()),
    );
    const colours = await evaluate(
      `getComputedStyle(document.documentElement).backgroundColor`,
    );
    if (colours !== "rgb(246, 247, 249)") {
      fail(`the page is not light: ${colours}`);
    }
    step("choosing the light theme applied it to the whole app and saved it");

    // The terminal's font, size and scrollback, from the dialog.
    const columnsBefore = await evaluate(`${terminal}.cols`);
    await setPreference("preference-font-family", "Consolas");
    await setPreference("preference-font-size", "18");
    await setPreference("preference-scrollback", "2000");
    await until(
      "the terminal with the chosen font",
      async () =>
        (await option("fontSize")) === 18 &&
        (await option("fontFamily")).startsWith(`"Consolas"`) &&
        (await option("scrollback")) === 2000,
    );
    await until(
      "fewer columns with a larger font",
      async () => (await evaluate(`${terminal}.cols`)) < columnsBefore,
    );
    await until("the font saved", () =>
      /font_family: Consolas[\s\S]*font_size: 18[\s\S]*scrollback: 2000/.test(
        preferencesFile(),
      ),
    );
    await closePreferences();
    step("the terminal font, its size and the scrollback length are set");

    // Shortcuts and Ctrl+scroll size the font.
    await press("=", "Equal", 187, CTRL);
    await until(
      "a larger font (Ctrl+=)",
      async () => (await option("fontSize")) === 19,
    );
    await press("-", "Minus", 189, CTRL);
    await press("-", "Minus", 189, CTRL);
    await until(
      "a smaller font (Ctrl+-)",
      async () => (await option("fontSize")) === 17,
    );
    const middle = await centreOf(`${SHOWN_SESSION} .terminal`);
    await mouse("mouseWheel", middle, {
      deltaX: 0,
      deltaY: -100,
      modifiers: CTRL,
    });
    await until(
      "a larger font (Ctrl+scroll)",
      async () => (await option("fontSize")) === 18,
    );
    await mouse("mouseWheel", middle, {
      deltaX: 0,
      deltaY: 100,
      modifiers: CTRL,
    });
    await mouse("mouseWheel", middle, {
      deltaX: 0,
      deltaY: 100,
      modifiers: CTRL,
    });
    await until(
      "a smaller font (Ctrl+scroll)",
      async () => (await option("fontSize")) === 16,
    );
    await press("0", "Digit0", 48, CTRL);
    await until(
      "the default font size (Ctrl+0)",
      async () => (await option("fontSize")) === 14,
    );
    await press("=", "Equal", 187, CTRL);
    await until(
      "the size saved",
      async () =>
        (await option("fontSize")) === 15 &&
        /font_size: 15/.test(preferencesFile()),
    );
    step("Ctrl+= / Ctrl+- / Ctrl+0 and Ctrl+scroll sized the font");

    // The scrollback keeps the chosen number of lines.
    await type("lines 3000\r");
    await until("the end of the listing", async () =>
      /line 3000\s*test-ssh\$/.test(await screen()),
    );
    const kept = await evaluate(
      `${terminal}.buffer.active.length - ${terminal}.rows`,
    );
    if (kept !== 2000) fail(`the scrollback kept ${kept} lines, not 2000`);
    step("the scrollback kept the 2000 lines chosen");

    // Searching the scrollback: Ctrl+Shift+F, then next and previous.
    await press("F", "KeyF", 70, CTRL | SHIFT);
    await until("the search bar, taking the keys", () =>
      evaluate(
        `document.activeElement?.matches("${SHOWN_SESSION} [data-terminal-search] input[name=search]") ?? false`,
      ),
    );
    await type("line 2999");
    const results = () =>
      evaluate(
        `document.querySelector("${SHOWN_SESSION} [data-search-results]").textContent`,
      );
    await until("one match", async () => (await results()) === "1 of 1");
    await evaluate(`(() => {
      const input = document.querySelector("${SHOWN_SESSION} [data-terminal-search] input[name=search]");
      input.select();
    })()`);
    // Of the lines kept (the last 2000 and a screen), "line 29" is in
    // 2900-2999 only: 100 matches.
    await type("line 29");
    await until("a hundred matches", async () =>
      /^\d+ of 100$/.test(await results()),
    );
    const current = async () =>
      Number((await results()).match(/^(\d+) of/)?.[1]);
    const first = await current();
    await press("Enter", "Enter", 13);
    await until(
      "the next match",
      async () => (await current()) === (first % 100) + 1,
    );
    await press("Enter", "Enter", 13, SHIFT);
    await until("the previous match", async () => (await current()) === first);
    await evaluate(
      `document.querySelector("${SHOWN_SESSION} [data-search=next]").click()`,
    );
    await until(
      "the next match (button)",
      async () => (await current()) === (first % 100) + 1,
    );
    const selected = await evaluate(`${terminal}.getSelection()`);
    if (selected !== "line 29") fail(`the match is not selected: ${selected}`);
    await screenshot("scrollback-search");
    step("the scrollback was searched, with next and previous");

    // Copying the selected match by keyboard; Escape closes the search and
    // gives the terminal the keys again.
    await press("Escape", "Escape", 27);
    await until(
      "the search closed",
      async () =>
        !(await evaluate(
          `!!document.querySelector("${SHOWN_SESSION} [data-terminal-search]")`,
        )),
    );
    // The clipboard may hold CR LF line ends where the terminal has LF.
    const same = (a, b) =>
      a.replace(/\r\n/g, "\n").trim() === b.replace(/\r\n/g, "\n").trim();
    writeClipboard("nothing yet");
    await press("C", "KeyC", 67, CTRL | SHIFT);
    await until("the selection copied (Ctrl+Shift+C)", () =>
      same(readClipboard(), "line 29"),
    );
    // Ctrl+C with a selection copies it (and does not interrupt).
    await evaluate(`${terminal}.selectAll()`);
    const all = await evaluate(`${terminal}.getSelection()`);
    await press("c", "KeyC", 67, CTRL);
    await until("the selection copied (Ctrl+C)", () =>
      same(readClipboard(), all),
    );
    if (await evaluate(`${terminal}.hasSelection()`)) {
      fail("Ctrl+C did not clear the selection it copied");
    }
    await sleep(300);
    if (/\^C/.test(await screen())) {
      fail("Ctrl+C with a selection interrupted");
    }
    step("Ctrl+Shift+C and Ctrl+C (with a selection) copied to the clipboard");

    // `later 0 <text>` prints the text, before or after the next prompt.
    const ran = (marker) => async () =>
      new RegExp(`(^|\\n|\\$ )${marker}\\s*(\\n|$)`).test(await screen());
    // Pasting by keyboard, three ways.
    const pastes = [
      ["Ctrl+Shift+V", () => press("V", "KeyV", 86, CTRL | SHIFT)],
      ["Ctrl+V", () => press("v", "KeyV", 86, CTRL)],
      ["Shift+Insert", () => press("Insert", "Insert", 45, SHIFT)],
    ];
    for (const [name, paste] of pastes) {
      const marker = `pasted-${name.replace(/\W/g, "").toLowerCase()}`;
      writeClipboard(`later 0 ${marker}\n`);
      await paste();
      await until(`the ${name} paste run`, ran(marker));
    }
    step("Ctrl+Shift+V, Ctrl+V and Shift+Insert pasted into the session");

    // Copy-on-select is off by default. (Right-click paste too: a right
    // click then opens the browser's own menu, which is not tried here.)
    const box = await evaluate(`(() => {
      const box = document.querySelector("${SHOWN_SESSION} .xterm-screen").getBoundingClientRect();
      return { x: box.x, y: box.y, width: box.width, height: box.height };
    })()`);
    const rowHeight = box.height / (await evaluate(`${terminal}.rows`));
    const dragSelect = async () => {
      const y = box.y + rowHeight * 1.5;
      const from = { x: box.x + 2, y };
      const to = { x: box.x + box.width / 3, y };
      await mouse("mousePressed", from, { button: "left", clickCount: 1 });
      await mouse("mouseMoved", to, { button: "left", buttons: 1 });
      await mouse("mouseReleased", to, { button: "left", clickCount: 1 });
    };
    const rightClick = async () => {
      const at = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
      await mouse("mousePressed", at, { button: "right", clickCount: 1 });
      await mouse("mouseReleased", at, { button: "right", clickCount: 1 });
    };
    writeClipboard("not a selection");
    await dragSelect();
    if (!(await evaluate(`${terminal}.hasSelection()`))) {
      fail("dragging selected nothing");
    }
    await sleep(800);
    if (!same(readClipboard(), "not a selection")) {
      fail(`selecting copied while copy-on-select is off: ${readClipboard()}`);
    }

    await openPreferences();
    await setPreference("preference-copy-on-select", true);
    await setPreference("preference-right-click-paste", true);
    await until("the copy and paste options saved", () =>
      /copy_on_select: true[\s\S]*right_click_paste: true/.test(
        preferencesFile(),
      ),
    );
    await closePreferences();
    await dragSelect();
    const selection = await evaluate(`${terminal}.getSelection()`);
    if (!selection.trim()) fail("dragging selected nothing");
    await until("the selection copied on select", () =>
      same(readClipboard(), selection),
    );
    writeClipboard("later 0 pasted-right-click\n");
    await rightClick();
    await until("the right-click paste run", ran("pasted-right-click"));
    step("copy-on-select and right-click paste work once turned on");
  } finally {
    await devtools.send("Emulation.setEmulatedMedia", { features: [] });
    writeClipboard(savedClipboard);
  }
}

/** Ctrl+K finds a host from the terminal, connects to it or shows it in the
 * tree; a saved filter is written to the workspace, applied to the tree
 * and deleted. */
async function searchAndFilter({
  evaluate,
  text,
  click,
  fillForm,
  type,
  devtools,
  screenshot,
  step,
}) {
  const filters = join(workspace, "_filters.yaml");
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${JSON.stringify(selector)})`);
  const clickOn = (selector) =>
    evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
  const key = async (spec) => {
    await devtools.send("Input.dispatchKeyEvent", { type: "keyDown", ...spec });
    await devtools.send("Input.dispatchKeyEvent", { type: "keyUp", ...spec });
  };
  const ctrlK = {
    key: "k",
    code: "KeyK",
    windowsVirtualKeyCode: 75,
    modifiers: 2,
  };
  const enter = { key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 };
  const palette = "[role=dialog][aria-label='Search hosts']";
  const hostRow = '[data-host="Lab/smoke-r1.yaml"]';
  const hit = '[data-hit="Lab/smoke-r1.yaml"]';
  const search = async (query) => {
    await key(ctrlK);
    await until("the palette", () => exists(`${palette} input`));
    await until("the palette's input focused", () =>
      evaluate(
        `document.activeElement?.name === "palette-query" || document.activeElement?.outerHTML.slice(0, 80)`,
      ).then((focused) => focused === true || Promise.reject(focused)),
    );
    await type(query);
    await until(`the hit for ${query}`, () =>
      evaluate(`(() => {
        const first = document.querySelector("${palette} [role=option]");
        return first?.dataset.hit === "Lab/smoke-r1.yaml" && first.getAttribute("aria-selected") === "true";
      })()`),
    );
  };
  const setView = (value) =>
    evaluate(`(() => {
      const select = document.querySelector("select[name=filter]");
      select.value = ${JSON.stringify(value)};
      select.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);

  // Ctrl+K reaches the palette even while the terminal has the keyboard.
  // Shift+Enter shows the host in the tree, opening its folder.
  await evaluate(
    `document.querySelector("nav [data-item=folder][data-path=Lab]").click()`,
  );
  await until("the Lab folder collapsed", async () => !(await exists(hostRow)));
  await evaluate(
    `document.querySelector("${SHOWN_SESSION} .xterm textarea")?.focus()`,
  );
  await search("smoke");
  await screenshot("palette");
  await key({ ...enter, modifiers: 8 });
  await until("the host shown and selected in the tree", () =>
    evaluate(
      `document.querySelector(${JSON.stringify(hostRow)})?.getAttribute("aria-pressed") === "true"`,
    ),
  );
  if (await exists(palette)) fail("the palette stayed open");
  step("Ctrl+K found a host and Shift+Enter showed it in the tree");

  // A field other than the name is searched and shown; Enter connects.
  await search("lab router");
  if (!(await text(hit)).includes("Role: Lab router")) {
    fail(`the hit does not show the matched role: ${await text(hit)}`);
  }
  await key(enter);
  await until("the credentials prompt", async () =>
    (await text("main h2")).includes("smoke-r1"),
  );
  await click("Cancel");
  step("a search on the role found the host and Enter connected to it");

  // A filter is saved to the workspace and the tree is viewed through it.
  await click("Save filter…");
  await until("the save filter dialog", () => exists("[role=dialog] form"));
  await evaluate(`(() => {
    const select = document.querySelector("[role=dialog] select[name=field]");
    select.value = "role";
    select.dispatchEvent(new Event("change", { bubbles: true }));
  })()`);
  await fillForm(
    { "filter-name": "Lab routers", value: "lab_router" },
    "[role=dialog] form",
  );
  await until(
    "the filter in the workspace",
    () =>
      existsSync(filters) &&
      readFileSync(filters, "utf8") === "Lab routers:\n  role: lab_router\n",
  );
  const odd = '[data-host="odd.yaml"]';
  await until(
    "the tree viewed through the filter",
    async () => (await exists(hostRow)) && !(await exists(odd)),
  );
  await screenshot("filter");
  await setView("");
  await until("every host back", () => exists(odd));
  await setView("Lab routers");
  await until("the filter applied again", async () => !(await exists(odd)));
  step("a filter was saved to the workspace and applied to the tree");

  await click("Delete filter");
  await until("the delete confirmation", () => exists("[role=dialog] form"));
  await clickOn("[role=dialog] form button[type=submit]");
  await until("the filters file removed", () => !existsSync(filters));
  await until("every host back", () => exists(odd));
  step("the filter was deleted");
}

/** Files changed outside the app (as by a git pull or an editor) show in
 * the tree and the side panel without a restart, keeping the selection. */
async function liveReload({ evaluate, text, screenshot, step }) {
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${JSON.stringify(selector)})`);
  const odd = 'nav [data-host="odd.yaml"]';
  const pulled = 'nav [data-host="pulled-r1.yaml"]';
  const oddSelected = () =>
    evaluate(
      `document.querySelector(${JSON.stringify(odd)})?.getAttribute("aria-pressed") === "true"`,
    );

  await evaluate(`document.querySelector(${JSON.stringify(odd)}).click()`);
  await until("odd.yaml's side panel", async () =>
    (await text(".side h2")).includes("odd"),
  );

  writeFileSync(join(workspace, "pulled-r1.yaml"), "address: 192.0.2.20\n");
  writeFileSync(
    join(workspace, "odd.yaml"),
    "address: 192.0.2.9\nrole: mainframe\nvendor: Juniper\n",
  );
  await until("the new host in the tree", () => exists(pulled));
  await until("the edited field in the side panel", () =>
    evaluate(
      `document.querySelector(".side [name=vendor]")?.value === "Juniper"`,
    ),
  );
  if (!(await oddSelected())) fail("the reload lost the selection");
  await screenshot("live-reload");
  step("a host added and a host edited on disk showed without a restart");

  rmSync(join(workspace, "pulled-r1.yaml"));
  await until(
    "the deleted host gone from the tree",
    async () => !(await exists(pulled)),
  );
  if (!(await oddSelected())) fail("the reload lost the selection");
  step("a host deleted on disk left the tree; the selection stayed");
}

/** A credential profile is created in the profiles dialog and assigned to
 * a host, which then connects without a prompt; a profile the workspace
 * names but nobody set up is asked for, saved and used; no secret reaches
 * a file. The app runs with its in-memory credential store, so the
 * keychain is never touched. */
async function credentialProfiles({
  evaluate,
  text,
  click,
  fillForm,
  changeField,
  doubleClick,
  status,
  screen,
  screenshot,
  step,
}) {
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${JSON.stringify(selector)})`);
  const clickOn = (selector) =>
    evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
  const hostRow = 'nav [data-host="Lab/smoke-r1.yaml"]';
  const hostFile = join(workspace, "Lab", "smoke-r1.yaml");
  const profilesDialog = "[data-dialog=profiles]";
  const profileRow = (name) => `${profilesDialog} [data-profile="${name}"]`;
  const connected = async () => {
    await until(
      "the Connected status",
      async () => (await status()) === "Connected",
    );
    await until("the shell prompt", async () =>
      (await screen()).includes("test-ssh$"),
    );
  };
  // Every file of the workspace and the app's data folder, for secrets.
  const everyFile = () =>
    [workspace, appData]
      .flatMap((root) =>
        readdirSync(root, { recursive: true, withFileTypes: true })
          .filter((entry) => entry.isFile())
          .map((entry) => join(entry.parentPath, entry.name)),
      )
      .map((path) => readFileSync(path, "utf8"))
      .join("\n");

  // A profile is created in the profiles dialog.
  await click("Profiles");
  await until("the profiles dialog", () => exists(profilesDialog));
  await click("New profile…");
  await until("the profile form", () =>
    exists(`${profilesDialog} [name=profile-name]`),
  );
  await fillForm(
    {
      "profile-name": "lab-login",
      "profile-username": USERNAME,
      "profile-password": PASSWORD,
      "profile-enable": "en4ble-s3cret",
    },
    `${profilesDialog} form`,
  );
  await until("the new profile listed", async () =>
    (await text(profileRow("lab-login"))).includes(
      `${USERNAME}, password, enable secret`,
    ),
  );
  await screenshot("profiles");
  await click("Close");
  await until(
    "the profiles dialog closed",
    async () => !(await exists(profilesDialog)),
  );
  step("a credential profile was created in the profiles dialog");

  // Assigned to a host, the profile logs in without a prompt.
  await until("the host in the tree", () => exists(hostRow));
  await clickOn(hostRow);
  await until("the host's side panel", async () =>
    (await text(".side h2")).includes("smoke-r1"),
  );
  await changeField("[name=setting-credential_profile]", "lab-login");
  await until("the profile's name in the host file", () =>
    readFileSync(hostFile, "utf8").includes("credential_profile: lab-login\n"),
  );
  await doubleClick(hostRow);
  await connected();
  if (
    !(await text(`${SHOWN_SESSION} .session header`)).includes(
      `${USERNAME}@smoke-r1`,
    )
  ) {
    fail(`the session is not titled with the profile's username`);
  }
  if (await exists("input[name=password]")) fail("a password was asked for");
  step("a host with a profile connected without a prompt");

  // A profile the workspace names but this computer lacks is asked for by
  // name, saved, and used.
  await click("Disconnect");
  await changeField("[name=setting-credential_profile]", "team-login");
  await until("the new profile name in the host file", () =>
    readFileSync(hostFile, "utf8").includes("credential_profile: team-login\n"),
  );
  await doubleClick(hostRow);
  await until("the missing profile named", async () =>
    (await text("main .notice")).includes("team-login"),
  );
  await screenshot("profile-not-set-up");
  await fillForm(
    { "profile-username": USERNAME, "profile-password": PASSWORD },
    "main form.profile",
  );
  await connected();
  step("a profile that was not set up was asked for by name, saved and used");

  // Profiles are edited and deleted in the dialog; a deleted profile the
  // workspace names shows as missing.
  await click("Disconnect");
  await click("Profiles");
  await until(
    "both profiles listed",
    async () =>
      (await exists(profileRow("lab-login"))) &&
      (await exists(profileRow("team-login"))),
  );
  await clickOn(`${profileRow("lab-login")} button`);
  await until("the edit form", () =>
    evaluate(
      `document.querySelector("${profilesDialog} [name=profile-username]")?.value === ${JSON.stringify(USERNAME)}`,
    ),
  );
  await fillForm({ "profile-username": "operator" }, `${profilesDialog} form`);
  await until("the edited username listed", async () =>
    (await text(profileRow("lab-login"))).includes("operator, password"),
  );
  await evaluate(
    `[...document.querySelectorAll(${JSON.stringify(`${profileRow("team-login")} button`)})].find((b) => b.textContent === "Delete").click()`,
  );
  await until("the delete confirmation", async () =>
    (await text(`${profilesDialog} h2`)).includes("Delete team-login"),
  );
  await evaluate(
    `document.querySelector("${profilesDialog} button.danger").click()`,
  );
  await until("the deleted profile shown as missing", async () =>
    (await text(profileRow("team-login"))).includes("not set up"),
  );
  await click("Close");
  step("a profile was edited and another deleted in the profiles dialog");

  for (const secret of [PASSWORD, "en4ble-s3cret"]) {
    if (everyFile().includes(secret)) fail(`a secret was written to a file`);
  }
  step("no secret was written to the workspace or the app's data folder");

  // Back as it was, for the inventory edits that follow.
  await changeField("[name=setting-credential_profile]", "");
  await until(
    "the profile cleared from the host file",
    () => !readFileSync(hostFile, "utf8").includes("credential_profile"),
  );
}

/** A one-time code the server asks for is shown and answered; a profile
 * that uses the SSH agent (the test server's, through SSH_AUTH_SOCK) logs
 * in with it; when the agent has no key the server accepts, the user is
 * told and logs in with a password instead. */
async function agentAndKeyboardInteractive({
  evaluate,
  text,
  click,
  fillForm,
  changeField,
  doubleClick,
  status,
  screen,
  screenshot,
  step,
}) {
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${JSON.stringify(selector)})`);
  const hostRow = 'nav [data-host="Lab/smoke-r1.yaml"]';
  const hostFile = join(workspace, "Lab", "smoke-r1.yaml");
  const profilesDialog = "[data-dialog=profiles]";
  const loginPrompt = "[data-login-prompt]";
  const connected = async () => {
    await until(
      "the Connected status",
      async () => (await status()) === "Connected",
    );
    await until("the shell prompt", async () =>
      (await screen()).includes("test-ssh$"),
    );
  };
  const useProfile = async (name) => {
    await changeField("[name=setting-credential_profile]", name);
    await until(`${name} in the host file`, () =>
      readFileSync(hostFile, "utf8").includes(`credential_profile: ${name}
`),
    );
  };

  // Keyboard-interactive: after the password, the server asks for a code,
  // which the user is shown and answers.
  await click("Disconnect").catch(() => {});
  await click("New connection").catch(() => {});
  await until("the quick-connect form", () => exists("input[name=host]"));
  await fillForm(
    {
      host: "127.0.0.1",
      port: String(SSH_PORT),
      username: OTP_USERNAME,
      password: PASSWORD,
    },
    "main form",
  );
  await until("the server's question", async () =>
    (await text(loginPrompt)).includes("Verification code:"),
  );
  if (!(await text(loginPrompt)).includes("authenticator app")) {
    fail("the question's instructions are not shown");
  }
  await screenshot("login-question");
  await fillForm({ "answer-0": ONE_TIME_CODE }, loginPrompt);
  await connected();
  if (await exists(loginPrompt)) fail("the question stayed on screen");
  step("a one-time code the server asked for was shown and answered");
  await click("Disconnect");

  // Profiles that log in with the SSH agent.
  const createAgentProfile = async (name, username) => {
    await click("Profiles");
    await until("the profiles dialog", () => exists(profilesDialog));
    await click("New profile…");
    await until("the profile form", () =>
      exists(`${profilesDialog} [name=profile-auth]`),
    );
    await evaluate(`(() => {
      const select = document.querySelector("${profilesDialog} [name=profile-auth]");
      select.value = "agent";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
    await until("the agent note", () =>
      exists(`${profilesDialog} [data-agent-note]`),
    );
    if (await exists(`${profilesDialog} [name=profile-password]`)) {
      fail("an agent profile asks for a password");
    }
    await fillForm(
      { "profile-name": name, "profile-username": username },
      `${profilesDialog} form`,
    );
    await until("the agent profile listed", async () =>
      (await text(`${profilesDialog} [data-profile="${name}"]`)).includes(
        `${username}, SSH agent`,
      ),
    );
    await click("Close");
    await until(
      "the profiles dialog closed",
      async () => !(await exists(profilesDialog)),
    );
  };
  await createAgentProfile("agent-login", USERNAME);
  await createAgentProfile("agent-stranger", "stranger");
  step("profiles that use the SSH agent were created");

  await useProfile("agent-login");
  await doubleClick(hostRow);
  await connected();
  if (
    !(await text(`${SHOWN_SESSION} .session header`)).includes(
      `${USERNAME}@smoke-r1`,
    )
  ) {
    fail("the agent session is not titled with the profile's username");
  }
  step("a profile using the SSH agent logged in with the agent's key");
  await click("Disconnect");

  // The server takes none of the agent's keys for this user: the user is
  // told why and logs in with a password instead.
  await useProfile("agent-stranger");
  await doubleClick(hostRow);
  await until("the agent failure", async () =>
    (await status()).includes("accepted none of the SSH agent's keys"),
  );
  await until("the failure explained over the terminal", async () =>
    (await text("[data-password-fallback]")).includes(
      "accepted none of the SSH agent's keys",
    ),
  );
  await screenshot("agent-refused");
  await click("Log in with a password");
  await until("the password prompt", () => exists("main input[name=password]"));
  if (!(await text("main [role=alert]")).includes("SSH agent")) {
    fail("the password prompt does not say why it is asked");
  }
  const prefilled = await evaluate(
    `document.querySelector("main input[name=username]").value`,
  );
  if (prefilled !== "stranger") {
    fail(`the profile's username is not filled in (${prefilled})`);
  }
  await fillForm({ username: USERNAME, password: PASSWORD }, "main form");
  await connected();
  step("an agent without an accepted key fell back to a password prompt");
  await click("Disconnect");

  await changeField("[name=setting-credential_profile]", "");
  await until(
    "the profile cleared from the host file",
    () => !readFileSync(hostFile, "utf8").includes("credential_profile"),
  );
}

/** A host on an old device (the test server restricted to legacy
 * algorithms) fails to negotiate with the switch off, and the failure
 * lists what the device offered and suggests the switch; with the switch
 * turned on in the side panel it connects, marked as using legacy crypto. */
async function legacyCrypto({
  evaluate,
  text,
  click,
  fillForm,
  changeField,
  doubleClick,
  hostKeyCheck,
  decideHostKey,
  status,
  screen,
  screenshot,
  step,
}) {
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${JSON.stringify(selector)})`);
  const hostRow = 'nav [data-host="Lab/old-switch.yaml"]';
  const hostFile = join(workspace, "Lab", "old-switch.yaml");
  const failureCard = "[data-negotiation-failure]";
  const marker = "[data-legacy-crypto]";
  const hasButton = (label) =>
    evaluate(
      `[...document.querySelectorAll("button")].some((b) => b.textContent.trim() === ${JSON.stringify(label)} && b.checkVisibility())`,
    );
  // Ends the session and goes back to the quick-connect view.
  const newConnection = async () => {
    await until("the New connection button", () => hasButton("New connection"));
    await click("New connection");
  };
  const logIn = async () => {
    await doubleClick(hostRow);
    await until(
      "the login prompt for the old device",
      async () =>
        (await text("main")).includes(`127.0.0.1:${OLD_DEVICE_PORT}`) &&
        (await exists("main input[name=password]")),
    );
    await fillForm({ username: USERNAME, password: PASSWORD }, "main form");
  };

  await click("Disconnect").catch(() => {});
  await click("New connection").catch(() => {});
  writeFileSync(
    hostFile,
    `name: old-switch\naddress: 127.0.0.1\nport: ${OLD_DEVICE_PORT}\n`,
  );
  await until("the old device in the tree", () => exists(hostRow));

  // Switch off: only modern algorithms are offered, and the device has none.
  await logIn();
  const failure = await until("the failed negotiation", async () =>
    (await exists(`${failureCard}[data-negotiation-failure=legacy-would-help]`))
      ? text(failureCard)
      : null,
  );
  for (const expected of [
    "No key-exchange algorithm in common with the device",
    "Turning on Legacy crypto would help",
    "diffie-hellman-group14-sha1",
    "ssh-rsa",
    "aes128-cbc",
    "hmac-sha1",
  ]) {
    if (!failure.includes(expected)) {
      fail(`the negotiation failure does not show "${expected}": ${failure}`);
    }
  }
  if (await exists(marker)) fail("a failed session is marked as legacy");
  await screenshot("negotiation-failure");
  step("a failed negotiation listed the device's algorithms and the switch");

  // Switch on, in the side panel: the session connects, marked.
  await newConnection();
  await evaluate(`document.querySelector(${JSON.stringify(hostRow)}).click()`);
  await until("the old device's side panel", async () =>
    (await text(".side h2")).includes("old-switch"),
  );
  await changeField("[name=setting-legacy_crypto]", "on");
  await until("the switch in the host file", () =>
    readFileSync(hostFile, "utf8").includes("legacy_crypto: true\n"),
  );
  await logIn();
  await hostKeyCheck("first-use");
  await decideHostKey("trustOnce");
  await until(
    "the Connected status",
    async () => (await status()) === "Connected",
  );
  await until("the shell prompt", async () =>
    (await screen()).includes("test-ssh$"),
  );
  const legacy = await until("the legacy-crypto marker", () =>
    evaluate(
      `document.querySelector(${JSON.stringify(marker)})?.getAttribute("data-legacy-crypto")`,
    ),
  );
  if (!legacy.includes("diffie-hellman-group14-sha1")) {
    fail(`the marker does not name the legacy algorithms: ${legacy}`);
  }
  if ((await text(marker)).trim() !== "Legacy crypto") {
    fail(`the marker reads "${await text(marker)}"`);
  }
  await screenshot("legacy-crypto");
  step("with the switch on, the old device connected, marked legacy crypto");
  await click("Disconnect");

  // A modern device is not marked.
  await newConnection();
  await until("the quick-connect form", () => exists("input[name=host]"));
  await fillForm(
    {
      host: "127.0.0.1",
      port: String(SSH_PORT),
      username: USERNAME,
      password: PASSWORD,
    },
    "main form",
  );
  await until(
    "the Connected status",
    async () => (await status()) === "Connected",
  );
  if (await exists(marker)) fail("a modern session is marked as legacy");
  if (await exists("[data-unencrypted]")) {
    fail("an SSH session is marked unencrypted");
  }
  step("a modern session is not marked");
  await click("Disconnect");
  await newConnection();

  rmSync(hostFile);
  await until(
    "the old device gone from the tree",
    async () => !(await exists(hostRow)),
  );
}

/** A host whose protocol is telnet opens a telnet session on double-click,
 * logging in in the terminal, marked unencrypted; the side panel offers
 * telnet for an SSH host, and SSH for a telnet one. */
async function telnet({
  evaluate,
  text,
  click,
  type,
  doubleClick,
  status,
  screen,
  screenshot,
  step,
}) {
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${JSON.stringify(selector)})`);
  const hostRow = 'nav [data-host="Lab/console-sw.yaml"]';
  const hostFile = join(workspace, "Lab", "console-sw.yaml");
  const marker = `${SHOWN_SESSION} [data-unencrypted]`;
  const sidePanelButton = (way) => `.side [data-connect=${way}]`;
  const hasButton = (label) =>
    evaluate(
      `[...document.querySelectorAll("button")].some((b) => b.textContent.trim() === ${JSON.stringify(label)} && b.checkVisibility())`,
    );
  const newConnection = async () => {
    await until("the New connection button", () => hasButton("New connection"));
    await click("New connection");
  };
  const loginPrompt = async () => {
    await until(
      "the Connected status",
      async () => (await status()) === "Connected",
    );
    await until("the device's login prompt", async () =>
      (await screen()).includes("Username:"),
    );
  };

  await click("Disconnect").catch(() => {});
  await click("New connection").catch(() => {});
  writeFileSync(
    hostFile,
    `name: console-sw\naddress: 127.0.0.1\nport: ${TELNET_PORT}\nprotocol: telnet\n`,
  );
  await until("the telnet host in the tree", () => exists(hostRow));

  // Double-click: no credentials prompt; the device asks in the terminal.
  await doubleClick(hostRow);
  await loginPrompt();
  if (await exists("main input[name=password]")) {
    fail("a telnet host asked for credentials before connecting");
  }
  if ((await text(marker)).trim() !== "Unencrypted") {
    fail(
      `the telnet session is not marked unencrypted: "${await text(marker)}"`,
    );
  }
  await type(`${USERNAME}\r`);
  await until("the password prompt", async () =>
    (await screen()).includes("Password:"),
  );
  await type(`${PASSWORD}\r`);
  await until("the telnet device's prompt", async () =>
    (await screen()).includes("test-telnet>"),
  );
  if ((await screen()).includes(PASSWORD)) fail("the password was shown");
  await type("size\r");
  const size = await until(
    "the output of `size`",
    async () => (await screen()).match(/size\s*(\d+x\d+)/)?.[1],
  );
  if (size.startsWith("0x") || size.endsWith("x0")) {
    fail(`the telnet device was not told the window size: ${size}`);
  }
  await screenshot("telnet-session");
  step(
    `double-clicking a telnet host logged in in the terminal (${size}), marked unencrypted`,
  );

  // The side panel offers SSH for the telnet host, and telnet for an SSH one.
  await click("Disconnect");
  await newConnection();
  await evaluate(`document.querySelector(${JSON.stringify(hostRow)}).click()`);
  await until("the SSH button for the telnet host", () =>
    exists(sidePanelButton("ssh")),
  );
  if (await exists(sidePanelButton("telnet"))) {
    fail("the side panel offers telnet for a host whose protocol is telnet");
  }
  writeFileSync(
    hostFile,
    `name: console-sw\naddress: 127.0.0.1\nport: ${TELNET_PORT}\n`,
  );
  await until("the Telnet button for an SSH host", () =>
    exists(sidePanelButton("telnet")),
  );
  await evaluate(
    `document.querySelector(${JSON.stringify(sidePanelButton("telnet"))}).click()`,
  );
  await loginPrompt();
  if (!(await exists(marker))) fail("the telnet session is not marked");
  step(
    "the side panel opened a telnet session to a host whose protocol is SSH",
  );
  await click("Disconnect");
  await newConnection();

  rmSync(hostFile);
  await until(
    "the telnet host gone from the tree",
    async () => !(await exists(hostRow)),
  );
}

/** Serial consoles: the quick-connect form lists the detected ports with
 * their adapters and can refresh them, defaults to 9600 8N1, and opens a
 * port without an inventory host; a serial host opens its own port on
 * double-click. The smoke machine has no port to talk to, so the sessions
 * open a port that does not exist and the failure names it (break and
 * unplug need a real adapter). */
async function serial({
  evaluate,
  text,
  click,
  fillForm,
  doubleClick,
  status,
  screenshot,
  step,
}) {
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${JSON.stringify(selector)})`);
  const clickOn = (selector) =>
    evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
  const value = (selector) =>
    evaluate(`document.querySelector(${JSON.stringify(selector)}).value`);
  const missingPort = "COM250";
  const failure = `Closed: cannot open the serial port ${missingPort}`;

  await until("the quick-connect form", () => exists("[data-quick-mode]"));
  await clickOn("[data-quick-mode=serial]");
  await until("the serial port list", async () => {
    const listed = await exists("[data-serial-port]");
    return listed || (await exists("[data-serial-none]"));
  });
  const ports = await evaluate(
    `[...document.querySelectorAll("[data-serial-port]")].map((b) => b.textContent.trim().replace(/\s+/g, " "))`,
  );
  await clickOn("[data-quick-serial] [data-serial-refresh]");
  await until("the refreshed list", async () => {
    const refreshing = await evaluate(
      `document.querySelector("[data-quick-serial] [data-serial-refresh]").disabled`,
    );
    return !refreshing && (await exists("[data-serial-ports]"));
  });
  if (ports.length > 0) {
    await clickOn(`[data-serial-port="${ports[0].split(" ")[0]}"]`);
    if ((await value("[name=serial-port]")) !== ports[0].split(" ")[0]) {
      fail("picking a detected port did not choose it");
    }
  }
  const defaults = [
    await value("[name=baud-rate]"),
    await value("[name=data-bits]"),
    await value("[name=parity]"),
    await value("[name=stop-bits]"),
    await value("[name=flow-control]"),
  ].join(" ");
  if (defaults !== "9600 8 none 1 none") {
    fail(
      `the serial defaults are ${defaults}, not 9600 8N1 without flow control`,
    );
  }
  await screenshot("serial-quick-connect");
  step(
    `the serial form lists ${ports.length ? ports.join(", ") : "no ports"} (refreshable), defaulting to 9600 8N1`,
  );

  await fillForm({ "serial-port": missingPort }, "[data-quick-serial]");
  await until(`"${failure}"`, async () => (await status()).startsWith(failure));
  if (await exists(`${SHOWN_SESSION} [data-unencrypted]`)) {
    fail("a serial session is marked unencrypted");
  }
  step(
    `a serial port opened without a host; a missing one is named: "${await status()}"`,
  );
  await click("New connection");

  // A host whose protocol is serial opens its own port and line settings,
  // with no credentials prompt.
  const hostRow = 'nav [data-host="Lab/serial-sw.yaml"]';
  const hostFile = join(workspace, "Lab", "serial-sw.yaml");
  writeFileSync(
    hostFile,
    `name: serial-sw\naddress: 192.0.2.50\nprotocol: serial\nserial:\n  port: ${missingPort}\n  baud_rate: 115200\n`,
  );
  await until("the serial host in the tree", () => exists(hostRow));
  await clickOn(hostRow);
  await until("its serial port in the side panel", async () =>
    (await text('.side [data-origin="serial.port"]')).includes(
      `${missingPort} · `,
    ),
  );
  await doubleClick(hostRow);
  await until(`"${failure}" for the host`, async () =>
    (await status()).startsWith(failure),
  );
  if (await exists("main input[name=password]")) {
    fail("a serial host asked for credentials");
  }
  step("double-clicking a serial host opened the port its settings name");
  await click("New connection");
  rmSync(hostFile);
  await until(
    "the serial host gone from the tree",
    async () => !(await exists(hostRow)),
  );
}

/** The inventory is edited from the tree and the side panel, and each
 * change is checked in the workspace files on disk. */
async function editInventory({
  evaluate,
  text,
  click,
  fillForm,
  changeField,
  centreOf,
  mouse,
  devtools,
  screenshot,
  step,
}) {
  const file = (path) => join(workspace, ...path.split("/"));
  const contents = (path) =>
    existsSync(file(path)) ? readFileSync(file(path), "utf8") : null;
  const clickOn = (selector) =>
    evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
  const dialog = "[role=dialog] form";
  const dialogOpen = () =>
    evaluate(`!!document.querySelector(${JSON.stringify(dialog)})`);
  const folderRow = (path) => `nav [data-item=folder][data-path="${path}"]`;
  const hostRow = (id) => `nav [data-host="${id}"]`;
  const hostFile =
    "address: 10.9.0.1\nvendor: Arista\nrack_unit: 12\nrole: firewall\n";
  // What the side panel says applies, and where it comes from.
  const shows = (key, expected) =>
    until(
      `${key} shown as "${expected}"`,
      async () =>
        (await text(`.side [data-origin="${key}"]`)).trim() === expected,
    );

  // Every toolbar button is fully inside the panel (it is narrower than
  // the buttons on one row, so they must wrap rather than be cut off).
  const clipped = await evaluate(`(() => {
    const panel = document.querySelector("aside").getBoundingClientRect();
    return [...document.querySelectorAll("aside .toolbar button")]
      .filter((b) => {
        const r = b.getBoundingClientRect();
        return r.width === 0 || r.left < panel.left || r.right > panel.right + 0.5
          || b.scrollWidth > b.clientWidth + 1 || r.height > 2 * parseFloat(getComputedStyle(b).lineHeight) + 8;
      })
      .map((b) => b.textContent.trim());
  })()`);
  if (clipped.length > 0) {
    fail(`toolbar buttons cut off or wrapped: ${clipped.join(", ")}`);
  }
  step("every workspace toolbar button is visible");

  // A folder and a host, created from the toolbar at the top level.
  await clickOn("aside header .name");
  await click("+ Folder");
  await until("the new folder dialog", dialogOpen);
  await fillForm({ name: "Edge" }, dialog);
  await until("the Edge folder on disk", () => existsSync(file("Edge")));
  await click("+ Host");
  await until("the new host dialog", dialogOpen);
  await fillForm({ name: "edge-r1", address: "10.9.0.1" }, dialog);
  await until(
    "the new host's file",
    () => contents("Edge/edge-r1.yaml") === "address: 10.9.0.1\n",
  );
  await until("the new host selected in the tree", () =>
    evaluate(
      `document.querySelector(${JSON.stringify(hostRow("Edge/edge-r1.yaml"))})?.getAttribute("aria-pressed") === "true"`,
    ),
  );
  step("a folder and a host were created and written to disk");

  // The side panel edits the selected host's file.
  await until("the host's side panel", async () =>
    (await text(".side h2")).includes("edge-r1"),
  );
  await changeField("[name=vendor]", "Arista");
  await until(
    "the vendor in the file",
    () =>
      contents("Edge/edge-r1.yaml") === "address: 10.9.0.1\nvendor: Arista\n",
  );
  await changeField("[name=rack_unit]", "12");
  await until(
    "the rack unit in the file",
    () =>
      contents("Edge/edge-r1.yaml") ===
      "address: 10.9.0.1\nvendor: Arista\nrack_unit: 12\n",
  );
  // The role is chosen from the list, built-in ones included.
  await changeField("[name=role]", "firewall");
  await until(
    "the role in the file",
    () => contents("Edge/edge-r1.yaml") === hostFile,
  );
  await until("the role's icon in the tree", () =>
    evaluate(
      `document.querySelector(${JSON.stringify(hostRow("Edge/edge-r1.yaml") + " [data-role-icon]")})?.dataset.roleIcon === "firewall"`,
    ),
  );
  await fillForm({ "new-tag": "edge" }, ".side form:has([name=new-tag])");
  await until("the tag in the file", () =>
    contents("Edge/edge-r1.yaml")?.endsWith("tags:\n- edge\n"),
  );
  await fillForm(
    { "new-key": "circuit", "new-value": "CID-77" },
    ".side form:has([name=new-key])",
  );
  await until("the custom field in the file", () =>
    contents("Edge/edge-r1.yaml")?.endsWith("custom:\n  circuit: CID-77\n"),
  );
  await changeField('[data-custom="circuit"]', "CID-78");
  await until("the edited custom field in the file", () =>
    contents("Edge/edge-r1.yaml")?.endsWith("custom:\n  circuit: CID-78\n"),
  );
  await screenshot("host-details");
  await clickOn('.side [aria-label="Remove tag edge"]');
  await until(
    "the tag removed from the file",
    async () => !contents("Edge/edge-r1.yaml")?.includes("tags:"),
  );
  await clickOn('.side [aria-label="Remove field circuit"]');
  await until(
    "the custom field removed from the file",
    () => contents("Edge/edge-r1.yaml") === hostFile,
  );
  step(
    "the side panel saved fields, a role, tags and custom fields to the file",
  );

  // One click copies a field's value.
  await devtools.send("Browser.grantPermissions", {
    permissions: ["clipboardReadWrite", "clipboardSanitizedWrite"],
  });
  await clickOn('.side [aria-label="Copy management address"]');
  await until("the copied address on the clipboard", async () => {
    const { result } = await devtools.send("Runtime.evaluate", {
      expression: "navigator.clipboard.readText()",
      awaitPromise: true,
      returnByValue: true,
    });
    return result.value === "10.9.0.1";
  });
  step("a field value was copied with one click");

  // Renaming from the side panel renames the file.
  await changeField("[name=name]", "edge-r2");
  await until(
    "the renamed host file",
    () =>
      contents("Edge/edge-r2.yaml") === hostFile &&
      !existsSync(file("Edge/edge-r1.yaml")),
  );
  step("renaming a host renamed its file");

  // Dragging the host onto another folder moves its file.
  const from = await centreOf(hostRow("Edge/edge-r2.yaml"));
  const to = await centreOf(folderRow("Lab"));
  await mouse("mousePressed", from, { button: "left", clickCount: 1 });
  for (let i = 1; i <= 5; i++) {
    const at = {
      x: from.x + ((to.x - from.x) * i) / 5,
      y: from.y + ((to.y - from.y) * i) / 5,
    };
    await mouse("mouseMoved", at, { button: "left", buttons: 1 });
  }
  await mouse("mouseReleased", to, { button: "left", clickCount: 1 });
  await until(
    "the dragged host's file in Lab",
    () =>
      contents("Lab/edge-r2.yaml") === hostFile &&
      !existsSync(file("Edge/edge-r2.yaml")),
  );
  step("dragging a host onto a folder moved its file");

  // The side panel shows each effective setting and where it comes from,
  // and sets and clears it on the host.
  await until("the moved host in the tree", () =>
    evaluate(
      `!!document.querySelector(${JSON.stringify(hostRow("Lab/edge-r2.yaml"))})`,
    ),
  );
  await clickOn(hostRow("Lab/edge-r2.yaml"));
  await shows("port", `${SSH_PORT} · from Lab`);
  await shows("protocol", "ssh · default");
  await shows("serial.baud_rate", "9600 · default");
  await shows("connect_timeout", "15 · default");
  await changeField("[name=setting-connect_timeout]", "40");
  await until(
    "the connect timeout in the host's file",
    () =>
      contents("Lab/edge-r2.yaml") ===
      "address: 10.9.0.1\nconnect_timeout: 40\nvendor: Arista\nrack_unit: 12\nrole: firewall\n",
  );
  await shows("connect_timeout", "40 · set on this host");
  await clickOn('.side [aria-label="Clear Connect timeout (seconds)"]');
  await until(
    "the connect timeout cleared from the host's file",
    () => contents("Lab/edge-r2.yaml") === hostFile,
  );
  await changeField("[name=setting-legacy_crypto]", "on");
  await until(
    "the host's own setting in its file",
    () =>
      contents("Lab/edge-r2.yaml") ===
      "address: 10.9.0.1\nlegacy_crypto: true\nvendor: Arista\nrack_unit: 12\nrole: firewall\n",
  );
  await shows("legacy_crypto", "on · set on this host");
  await screenshot("host-settings");
  await clickOn('.side [aria-label="Clear Legacy crypto"]');
  await until(
    "the host's setting cleared from its file",
    () => contents("Lab/edge-r2.yaml") === hostFile,
  );
  await shows("legacy_crypto", "off · default");
  step(
    "the side panel showed effective settings and origins, set and cleared one",
  );

  // A folder's description is saved to its settings file.
  await clickOn(folderRow("Edge"));
  await until(
    "the folder's side panel",
    async () => (await text(".side h2")) === "Edge",
  );
  await changeField("[name=folder-description]", "Edge routers");
  await until(
    "the folder settings file",
    () => contents("Edge/_folder.yaml") === "description: Edge routers\n",
  );
  step("a folder description was written to its settings file");

  // A setting on a folder is written to its settings file.
  await changeField("[name=setting-environment]", "lab");
  await until(
    "the folder's setting in its file",
    () =>
      contents("Edge/_folder.yaml") ===
      "description: Edge routers\nenvironment: lab\n",
  );
  await shows("environment", "lab · set on this folder");
  step("a setting on a folder was written to its settings file");

  // Moving a folder from its context menu.
  const openMenu = async (selector) => {
    const at = await centreOf(selector);
    await mouse("mousePressed", at, { button: "right", clickCount: 1 });
    await mouse("mouseReleased", at, { button: "right", clickCount: 1 });
    await until("the context menu", () =>
      evaluate(`!!document.querySelector("[role=menu]")`),
    );
  };
  await openMenu(folderRow("Edge"));
  await click("Move to…");
  await until("the move dialog", dialogOpen);
  await fillForm({ folder: "Lab" }, dialog);
  await until("the moved folder", () =>
    existsSync(file("Lab/Edge/_folder.yaml")),
  );
  step("a folder was moved from its context menu");

  // A folder without hosts is deleted at once; one with hosts asks first.
  await until("the moved folder in the tree", () =>
    evaluate(
      `!!document.querySelector(${JSON.stringify(folderRow("Lab/Edge"))})`,
    ),
  );
  await openMenu(folderRow("Lab/Edge"));
  await click("Delete");
  await until("the empty folder deleted", () => !existsSync(file("Lab/Edge")));
  await openMenu(folderRow("Lab"));
  await click("Delete");
  await until("the delete confirmation", async () =>
    (await text("[role=dialog]")).includes("contains 2 hosts"),
  );
  await screenshot("delete-confirmation");
  if (!existsSync(file("Lab/smoke-r1.yaml"))) {
    fail("the folder was deleted before the confirmation");
  }
  await click("Delete 2 hosts");
  await until("the folder with hosts deleted", () => !existsSync(file("Lab")));
  step("deleting a folder with hosts asked for confirmation first");
}

/** Sessions in tabs, split panes, WebGL on visible terminals only, hidden
 * terminals brought up to date, and twenty sessions at once. */
async function tabsAndPanes({
  evaluate,
  fillForm,
  centreOf,
  mouse,
  devtools,
  screenshot,
  step,
}) {
  const json = (value) => JSON.stringify(value);
  const clickOn = (selector) =>
    evaluate(`document.querySelector(${json(selector)}).click()`);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const press = async (key, code, windowsVirtualKeyCode, modifiers) => {
    const event = { key, code, windowsVirtualKeyCode, modifiers };
    await devtools.send("Input.dispatchKeyEvent", {
      type: "keyDown",
      ...event,
    });
    await devtools.send("Input.dispatchKeyEvent", { type: "keyUp", ...event });
  };
  const CTRL = 2;
  const SHIFT = 8;
  const nextTab = () => press("Tab", "Tab", 9, CTRL);
  const previousTab = () => press("Tab", "Tab", 9, CTRL | SHIFT);
  const type = async (keys) => {
    for (const char of keys) {
      const key =
        char === "\r"
          ? { key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 }
          : { key: char };
      await devtools.send("Input.dispatchKeyEvent", {
        type: "keyDown",
        text: char,
        ...key,
      });
      await devtools.send("Input.dispatchKeyEvent", { type: "keyUp", ...key });
    }
  };
  // Every tab, pane by pane: [{ pane, tabs: [{ id, name, state, selected }] }].
  const layout = () =>
    evaluate(`[...document.querySelectorAll(".pane")].map((pane) => ({
      pane: Number(pane.dataset.pane),
      active: pane.classList.contains("active"),
      tabs: [...pane.querySelectorAll("[role=tab]")].map((tab) => ({
        id: Number(tab.dataset.tab),
        name: tab.querySelector("[data-tab-name]").textContent,
        state: tab.dataset.tabState,
        selected: tab.getAttribute("aria-selected") === "true",
      })),
    }))`);
  const allTabs = async () => (await layout()).flatMap((pane) => pane.tabs);
  const activeTab = async () =>
    (await layout()).find((pane) => pane.active)?.tabs.find((t) => t.selected)
      ?.id ?? null;
  const slot = (tab) => `.slot[data-tab="${tab}"]`;
  const screenOfTab = (tab) => screenOf(evaluate, slot(tab));
  // Whether each terminal draws with WebGL: [{ tab, hidden, webgl, lost }].
  const renderers = () =>
    evaluate(`[...document.querySelectorAll(".slot[data-tab]")].map((slot) => {
      const canvas = slot.querySelector(".xterm-screen canvas:not(.xterm-link-layer)");
      const gl = canvas?.getContext("webgl2");
      return {
        tab: Number(slot.dataset.tab),
        hidden: slot.hidden,
        webgl: !!gl,
        lost: gl ? gl.isContextLost() : null,
      };
    })`);
  const checkRenderers = async (where) => {
    const all = await renderers();
    const wrong = all.filter(
      (r) => r.hidden === r.webgl || (r.webgl && r.lost),
    );
    if (wrong.length > 0) {
      fail(`${where}: WebGL not only on visible terminals: ${json(all)}`);
    }
  };
  const confirmClose = async (choice) => {
    await until("the close confirmation", () => exists("[data-confirm-close]"));
    await clickOn(`[data-confirm=${choice}]`);
    await until(
      "the confirmation gone",
      async () => !(await exists("[data-confirm-close]")),
    );
  };
  // Opens a quick-connect session in a new tab of the active pane and
  // waits for its prompt; returns its tab.
  const openSession = async () => {
    const before = new Set((await allTabs()).map((tab) => tab.id));
    await clickOn(".pane.active [data-new-tab]");
    await until("the quick-connect form", () => exists("input[name=host]"));
    await fillForm({
      host: "127.0.0.1",
      port: String(SSH_PORT),
      username: USERNAME,
      password: PASSWORD,
    });
    const tab = await until(
      "the new tab",
      async () => (await allTabs()).find((t) => !before.has(t.id))?.id,
    );
    if ((await activeTab()) !== tab) fail("the new tab is not shown");
    await until(`tab ${tab} connected`, async () =>
      (await allTabs()).some((t) => t.id === tab && t.state === "connected"),
    );
    await until(`tab ${tab}'s prompt`, async () =>
      (await screenOfTab(tab)).includes("test-ssh$"),
    );
    return tab;
  };
  const drag = async (fromSelector, toSelector, offsetX = 0) => {
    const from = await centreOf(fromSelector);
    const to = await centreOf(toSelector);
    to.x += offsetX;
    await mouse("mousePressed", from, { button: "left", clickCount: 1 });
    await mouse(
      "mouseMoved",
      { x: from.x + 10, y: from.y + 2 },
      {
        button: "left",
        buttons: 1,
      },
    );
    await mouse("mouseMoved", to, { button: "left", buttons: 1 });
    await mouse("mouseReleased", to, { button: "left", clickCount: 1 });
  };

  // Start from no tabs: closed sessions close at once, live ones ask.
  for (let left = await allTabs(); left.length > 0; left = await allTabs()) {
    const [tab] = left;
    await clickOn(`[data-tab="${tab.id}"] [data-tab-close]`);
    if (tab.state !== "disconnected") await confirmClose("close");
    await until("the tab closed", async () =>
      (await allTabs()).every((t) => t.id !== tab.id),
    );
  }
  if (await exists("[data-session-switch]")) {
    fail("the session switcher is still there");
  }

  // Each session opens in a new tab named after its host, showing its state.
  const a = await openSession();
  const b = await openSession();
  let tabs = await allTabs();
  if (
    tabs.length !== 2 ||
    tabs.some((t) => t.name !== "127.0.0.1" || t.state !== "connected")
  ) {
    fail(`unexpected tabs: ${json(tabs)}`);
  }
  await screenshot("tabs");
  step("each session opened in its own tab, named after the host, connected");

  // Next, previous and moving tabs from the keyboard.
  await nextTab();
  await until(
    "the next tab (round to the first)",
    async () => (await activeTab()) === a,
  );
  await previousTab();
  await until("the previous tab", async () => (await activeTab()) === b);
  await press("PageUp", "PageUp", 33, CTRL);
  await until(
    "the previous tab (Ctrl+PageUp)",
    async () => (await activeTab()) === a,
  );
  await press("PageDown", "PageDown", 34, CTRL | SHIFT);
  await until(
    "the tab moved right",
    async () => json((await allTabs()).map((t) => t.id)) === json([b, a]),
  );
  // Dragging the tab back before the other one.
  await drag(`[data-tab="${a}"]`, `[data-tab="${b}"]`, -20);
  await until(
    "the tab dragged back",
    async () => json((await allTabs()).map((t) => t.id)) === json([a, b]),
  );
  step(
    "Ctrl+Tab / Ctrl+Shift+Tab / Ctrl+PageUp switch tabs; tabs reorder by keyboard and drag",
  );

  // A hidden terminal keeps its output aside, and shows it when shown.
  await clickOn(`[data-tab="${a}"]`);
  await until("tab A shown", async () => (await activeTab()) === a);
  await type("later 2 hidden-update\r");
  await until("the command echoed", async () =>
    (await screenOfTab(a)).includes("later 2 hidden-update"),
  );
  await nextTab();
  await until("tab B shown", async () => (await activeTab()) === b);
  await sleep(3500);
  // It arrives after the new prompt: `test-ssh$ hidden-update`.
  const arrived = async () => /\$ hidden-update/.test(await screenOfTab(a));
  if (await arrived()) {
    fail("the hidden terminal was written to while out of sight");
  }
  await checkRenderers("one tab hidden");
  await previousTab();
  await until("the held output written once shown", arrived);
  step("a hidden terminal held its output and caught up when shown again");

  // Splitting right moves the shown tab into a new pane beside the other.
  await clickOn(".pane.active [data-split=right]");
  await until("two panes", async () => (await layout()).length === 2);
  let panes = await layout();
  if (
    json(panes.map((p) => p.tabs.map((t) => t.id))) !== json([[b], [a]]) ||
    !panes[1].active
  ) {
    fail(`unexpected panes after splitting: ${json(panes)}`);
  }
  const boxes = await evaluate(`[${a}, ${b}].map((tab) => {
    const box = document.querySelector('.slot[data-tab="' + tab + '"]').getBoundingClientRect();
    return { x: box.x, y: box.y, width: box.width };
  })`);
  if (!(boxes[0].x > boxes[1].x + boxes[1].width - 1)) {
    fail(`the panes are not side by side: ${json(boxes)}`);
  }
  await until("both terminals drawn with WebGL", async () =>
    (await renderers()).every((r) => r.webgl && !r.lost),
  );
  await checkRenderers("two panes");
  // Typing goes to the active pane's session.
  await type("size\r");
  await until("output in the active pane", async () =>
    /size\s*\d+x\d+/.test(await screenOfTab(a)),
  );
  // Splitting a pane with one tab gives an empty pane for a new session.
  await clickOn(".pane.active [data-split=down]");
  await until("three panes", async () => (await layout()).length === 3);
  const c = await openSession();
  panes = await layout();
  if (!panes[2].tabs.some((t) => t.id === c)) {
    fail(`the new session is not in the new pane: ${json(panes)}`);
  }
  await screenshot("split-panes");
  step("the window split right and down, each pane showing its own session");

  // A tab dragged into another pane moves there; its old pane, now empty,
  // goes. The terminal (and its session) move with it.
  await drag(
    `[data-tab="${c}"]`,
    `.pane[data-pane="${panes[0].pane}"] [data-new-tab]`,
  );
  await until("the tab moved into the first pane", async () => {
    const now = await layout();
    return (
      now.length === 2 &&
      json(now[0].tabs.map((t) => t.id)) === json([b, c]) &&
      now[0].active
    );
  });
  if (!(await screenOfTab(c)).includes("test-ssh$")) {
    fail("the moved tab lost its terminal");
  }
  await checkRenderers("after moving a tab");
  step("a tab moved into another pane, keeping its session");

  // A lost WebGL context is recovered from.
  await evaluate(`document.querySelector('${slot(c)} .xterm-screen canvas:not(.xterm-link-layer)')
    .getContext("webgl2").getExtension("WEBGL_lose_context").loseContext()`);
  await until("the WebGL renderer back after a lost context", async () =>
    (await renderers()).some((r) => r.tab === c && r.webgl && !r.lost),
  );
  await type("size\r");
  await until("output after the lost context", async () =>
    /size\s*\d+x\d+/.test(await screenOfTab(c)),
  );
  step("the terminal recovered from a lost WebGL context");

  // Closing a tab with a live session asks first; Cancel keeps it.
  await clickOn(`[data-tab="${c}"] [data-tab-close]`);
  await screenshot("close-confirmation");
  await confirmClose("cancel");
  if (!(await allTabs()).some((t) => t.id === c)) fail("Cancel closed the tab");
  await press("W", "KeyW", 87, CTRL | SHIFT);
  await confirmClose("close");
  await until("the tab closed", async () =>
    (await allTabs()).every((t) => t.id !== c),
  );
  step(
    "closing a live session's tab asked for confirmation (Ctrl+Shift+W too)",
  );

  // Closing a pane gives its tabs to the pane beside it.
  await clickOn(
    `.pane[data-pane="${(await layout())[1].pane}"] [data-close-pane]`,
  );
  await until("one pane with both tabs", async () => {
    const now = await layout();
    return now.length === 1 && now[0].tabs.length === 2;
  });
  step("closing a pane moved its tabs into the other");

  // Twenty sessions at once, each drawn when shown; only the visible one
  // has a WebGL context.
  while ((await allTabs()).length < 20) await openSession();
  const twenty = (await allTabs()).map((t) => t.id);
  for (const tab of twenty) {
    await clickOn(`[data-tab="${tab}"]`);
    await until(`tab ${tab} shown`, async () => (await activeTab()) === tab);
    await type("size\r");
    await until(`tab ${tab}'s output`, async () =>
      /size\s*\d+x\d+/.test(await screenOfTab(tab)),
    );
    await checkRenderers(`twenty sessions, tab ${tab}`);
  }
  if ((await allTabs()).some((t) => t.state !== "connected")) {
    fail(`not every session stayed connected: ${json(await allTabs())}`);
  }
  await screenshot("twenty-sessions");
  step("twenty sessions were open at once, each rendering when shown");
}

let exitCode = 0;
try {
  await main();
  console.log("smoke test passed");
} catch (error) {
  console.error(`smoke test FAILED: ${error.message}`);
  exitCode = 1;
} finally {
  for (const child of children) child.kill();
  releaseServer.close();
  rmSync(INSTALLED_MARKER, { force: true });
  rmSync(PORTABLE_MARKER, { force: true });
  await sleep(500);
  try {
    rmSync(userData, { recursive: true, force: true });
  } catch {
    // WebView2 can keep files locked for a moment; the folder is in temp.
  }
}
process.exit(exitCode);
