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
// A public key the test server does not have: stored as the server's key,
// it makes the server's real key look changed.
const OTHER_KEY_FILE = "crates/core/tests/fixtures/keys/rsa_client.pub";

const children = [];
const userData = mkdtempSync(join(tmpdir(), "netterm-smoke-"));
// The app's own data folder, holding a last workspace to reopen on launch.
const appData = join(userData, "app-data");
const workspace = join(userData, "smoke-workspace");

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
      // Agent profiles use the test server's agent.
      SSH_AUTH_SOCK: AGENT_PIPE,
    },
  });
  children.push(app);
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
  await startServer();
  startApp();
  const devtools = await connectDevTools();

  const evaluate = async (expression) => {
    const { result, exceptionDetails } = await devtools.send(
      "Runtime.evaluate",
      { expression, returnByValue: true },
    );
    if (exceptionDetails) fail(`page error: ${exceptionDetails.text}`);
    return result.value;
  };
  const text = (selector) =>
    evaluate(
      `document.querySelector(${JSON.stringify(selector)})?.textContent ?? ""`,
    );
  // The session on screen; others (duplicates) may run out of sight.
  const shownSession = ".slot:not([hidden])";
  const screen = () => text(`${shownSession} .xterm-rows`);
  const status = () => text(`${shownSession} [role=status]`);
  const click = (label) =>
    evaluate(
      `[...document.querySelectorAll("button")].find((b) => b.textContent.trim() === ${JSON.stringify(label)}).click()`,
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
  const centreOf = (selector) =>
    evaluate(`(() => {
      const box = document.querySelector(${JSON.stringify(selector)}).getBoundingClientRect();
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
      const spans = [...document.querySelectorAll(".xterm-rows span")];
      const has = (text, cls) => spans.some((span) => span.textContent === text && span.classList.contains(cls));
      return has("down", "xterm-bold") && has("up", "xterm-fg-46");
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

  // Duplicate opens a second session to the same host beside the first.
  const switchTo = (index) =>
    evaluate(
      `document.querySelectorAll("[data-session-switch]")[${index}].click()`,
    );
  await click("Duplicate");
  await until("two sessions", () =>
    evaluate(`document.querySelectorAll(".slot").length === 2`),
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
    evaluate(`document.querySelectorAll(".slot").length === 1`),
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
  await expectIcon("session header", ".session header", "router", orange);
  if (!(await text(".session header")).includes("Lab router")) {
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

  devtools.close();
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
  await evaluate(`document.querySelector(".xterm textarea")?.focus()`);
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
  if (!(await text(".session header")).includes(`${USERNAME}@smoke-r1`)) {
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
  if (!(await text(".session header")).includes(`${USERNAME}@smoke-r1`)) {
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
      `[...document.querySelectorAll("button")].some((b) => b.textContent.trim() === ${JSON.stringify(label)})`,
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
  const marker = ".slot:not([hidden]) [data-unencrypted]";
  const sidePanelButton = (way) => `.side [data-connect=${way}]`;
  const hasButton = (label) =>
    evaluate(
      `[...document.querySelectorAll("button")].some((b) => b.textContent.trim() === ${JSON.stringify(label)})`,
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
  if (await exists(".slot:not([hidden]) [data-unencrypted]")) {
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

let exitCode = 0;
try {
  await main();
  console.log("smoke test passed");
} catch (error) {
  console.error(`smoke test FAILED: ${error.message}`);
  exitCode = 1;
} finally {
  for (const child of children) child.kill();
  await sleep(500);
  try {
    rmSync(userData, { recursive: true, force: true });
  } catch {
    // WebView2 can keep files locked for a moment; the folder is in temp.
  }
}
process.exit(exitCode);
