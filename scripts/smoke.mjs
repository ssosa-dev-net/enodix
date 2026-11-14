// End-to-end smoke test: launches the built app and the test SSH server,
// drives the real interface through the WebView2 DevTools protocol, and
// checks what a user would see.
//
//   npm run app:build && npm run smoke
//
// Windows only for now: it relies on WebView2's remote-debugging switch.

import { spawn, spawnSync } from "node:child_process";
import {
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

const SSH_PORT = 22722;
const DEVTOOLS_PORT = 9333;
const USERNAME = "engineer";
const PASSWORD = "correct horse";
const TIMEOUT_MS = 15000;

const children = [];
const userData = mkdtempSync(join(tmpdir(), "netterm-smoke-"));
// The app's own data folder, holding a last workspace to reopen on launch.
const appData = join(userData, "app-data");
const workspace = join(userData, "smoke-workspace");

/** A workspace with one host (the test server) and one malformed file. */
function createWorkspace() {
  mkdirSync(join(workspace, "Lab"), { recursive: true });
  writeFileSync(join(workspace, "Lab", "_folder.yaml"), "description: Lab\n");
  writeFileSync(
    join(workspace, "Lab", "smoke-r1.yaml"),
    `name: smoke-r1\naddress: 127.0.0.1\nport: ${SSH_PORT}\n`,
  );
  writeFileSync(join(workspace, "broken.yaml"), "name: [\n");
  mkdirSync(appData);
  // JSON is valid YAML, and quotes the Windows path safely.
  writeFileSync(
    join(appData, "state.yaml"),
    JSON.stringify({ last_workspace: workspace }),
  );
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
    [String(SSH_PORT)],
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
  const screen = () => text(".xterm-rows");
  const status = () => text("[role=status]");
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

  // Quick-connect opens an interactive shell.
  await quickConnect(PASSWORD);
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

  // Closing the session from the interface is reflected too.
  await click("New connection");
  await quickConnect(PASSWORD);
  await until(
    "the Connected status",
    async () => (await status()) === "Connected",
  );
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
  step("a wrong password is reported");

  // Double-clicking a host in the tree prompts for credentials and opens an
  // SSH session to its management address.
  await evaluate(
    `[...document.querySelectorAll("nav button[aria-expanded]")].find((b) => b.textContent.includes("Lab")).click()`,
  );
  const hostButton = '[data-host="Lab/smoke-r1.yaml"]';
  await until("the host in the tree", () =>
    evaluate(`!!document.querySelector(${JSON.stringify(hostButton)})`),
  );
  await doubleClick(hostButton);
  await until("the credentials prompt", async () =>
    (await text("h2")).includes("smoke-r1"),
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
  step("double-clicking a host opened a session to it");

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
  const hostFile = "address: 10.9.0.1\nvendor: Arista\nrack_unit: 12\n";

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
    () => contents("Edge/edge-r1.yaml") === hostFile,
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
  step("the side panel saved fields, tags and custom fields to the file");

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
