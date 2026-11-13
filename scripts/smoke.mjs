// End-to-end smoke test: launches the built app and the test SSH server,
// drives the real interface through the WebView2 DevTools protocol, and
// checks what a user would see.
//
//   npm run app:build && npm run smoke
//
// Windows only for now: it relies on WebView2's remote-debugging switch.

import { spawn, spawnSync } from "node:child_process";
import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
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
  const quickConnect = async (password) => {
    await until("the quick-connect form", () =>
      evaluate(`!!document.querySelector("input[name=host]")`),
    );
    const fields = {
      host: "127.0.0.1",
      port: String(SSH_PORT),
      username: USERNAME,
      password,
    };
    await evaluate(`(() => {
      const fields = ${JSON.stringify(fields)};
      for (const [name, value] of Object.entries(fields)) {
        const input = document.querySelector("input[name=" + name + "]");
        input.value = value;
        input.dispatchEvent(new Event("input", { bubbles: true }));
      }
      document.querySelector("form").requestSubmit();
    })()`);
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

  devtools.close();
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
