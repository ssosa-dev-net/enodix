// End-to-end smoke test: launches the built app and the test SSH server,
// drives the real interface through the WebView2 DevTools protocol, and
// checks what a user would see.
//
//   npm run app:build && npm run smoke
//
// Windows only for now: it relies on WebView2's remote-debugging switch.
//
// Where it finds what it runs, for a computer with no Rust toolchain
// (`npm run smoke:sandbox`, scripts/sandbox.mjs, #139); none set, it builds
// and runs everything from this checkout as always:
//
//   SMOKE_APP_FOLDER   the folder of netterm.exe, netterm-cli.exe and
//                      python\ (default target/release); written to (the
//                      installed and portable markers)
//   SMOKE_TEST_SERVER  a test_ssh_server.exe built beforehand: run as it
//                      is, with no cargo build (default: built here)
//   SMOKE_PYTHON       the Python the test itself asks first (the app looks
//                      for one on its own, through py -3, python3, python)
//   SMOKE_WEBVIEW2_POLICY  set: WebView2's arguments also go in the
//                      machine's WebView2 policy (`webviewPolicy`; an
//                      elevated app ignores the environment variable)
//
// To repeat one part quickly (#141); none set, the whole test runs:
//
//   SMOKE_ONLY=severalInAWindow  only the part with several sessions in a
//                      session window (the split typing step among them),
//                      after three quick connections; the run then ends
//   SMOKE_SPLIT_TYPING_REPEAT=<n>  that part's typing in each of two
//                      panes done n times over (default once)

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  cpSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readdirSync,
  readFileSync,
  renameSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { createServer } from "node:http";
import { createServer as createTlsServer } from "node:https";
import { createServer as createTcpServer } from "node:net";
import { tmpdir } from "node:os";
import { basename, join } from "node:path";
import { setTimeout as sleep } from "node:timers/promises";

import {
  netLogArguments,
  netLogProblems,
  watchConnections,
} from "./connections.mjs";

/** The one part to run, if any (see the top). */
const ONLY = process.env.SMOKE_ONLY || null;
if (ONLY !== null && ONLY !== "severalInAWindow") {
  console.error(`SMOKE_ONLY=${ONLY}: only severalInAWindow can run alone`);
  process.exit(2);
}
/** How many times the split panes' typing is done (see the top). */
const SPLIT_TYPING_REPEAT = Number(process.env.SMOKE_SPLIT_TYPING_REPEAT || 1);
/** The built app's folder (see the top). */
const APP_FOLDER = process.env.SMOKE_APP_FOLDER || join("target", "release");
const SSH_PORT = 22722;
// A second test server, restricted to an old device's algorithms.
const OLD_DEVICE_PORT = 22723;
// The test telnet device (same username and password, asked in the terminal).
const TELNET_PORT = 22724;
// The device-mode SSH server (prompt `lab-r1# `), for jobs.
const DEVICE_PORT = 22725;
// A server with SCP only, no SFTP (#106), serving `scpFolder`.
const SCP_PORT = 22726;
// A server that sends an SSH login banner before the login (#134).
const BANNER_PORT = 22727;
const DEVTOOLS_PORT = 9333;
// A port of this computer nobody listens on, for the refused connection.
const CLOSED_PORT = 1;
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
/** The Preferences view, which opens as a tab (#68). */
const PREFERENCES = "[data-view=preferences]";
/** The job tab on screen (#69): a job from its choice of snippet or script
 * to its results. */
const JOB_TAB = ".slot.view:not([hidden]) [data-view=job]";
/** The kept run's tab on screen (#69). */
const RUN_TAB = ".slot.view:not([hidden]) [data-view=run]";
/** The review of a fill-in run on screen (#73). */
const ENRICH_TAB = ".slot.view:not([hidden]) [data-view=enrich]";
/** The Python setup tab on screen (#73). */
const PYTHON_SETUP_TAB = ".slot.view:not([hidden]) [data-view=python-setup]";
/** The Jobs side view (#69): running jobs, saved jobs, past runs. */
const JOBS_SIDE = "[data-side-view=jobs]";

/** Opens Preferences from the rail (a tab; its own if open) and waits for
 * it to be on screen. */
async function openPreferences(evaluate) {
  // The tab shown before, which closing Preferences shows again (else the
  // pane would show the tab beside it).
  await evaluate(`(() => {
    const shown = document.querySelector("${SHOWN_SESSION}");
    if (shown && !document.querySelector("[data-view=preferences]"))
      window.smokeBeforePreferences = shown.dataset.tab;
  })()`);
  await mouseClick(evaluate, `[data-open-preferences]`);
  await until("the Preferences tab", () =>
    evaluate(`(() => {
      const view = document.querySelector(${JSON.stringify(PREFERENCES)});
      return !!view && !view.closest("[hidden]");
    })()`),
  );
}

/** Closes the Preferences tab and shows again the session shown before
 * it opened (the core shows the closed tab's neighbour). */
async function closePreferences(evaluate) {
  await mouseClickIfThere(
    evaluate,
    "[data-tab-view=preferences] [data-tab-close]",
  );
  await until(
    "the Preferences tab closed",
    async () =>
      !(await evaluate(
        `!!document.querySelector(${JSON.stringify(PREFERENCES)})`,
      )),
  );
  const before = await evaluate(`(() => {
    const before = window.smokeBeforePreferences;
    window.smokeBeforePreferences = undefined;
    return before ?? null;
  })()`);
  if (before === null) return;
  const tab = `[data-tab-bar] [data-tab="${before}"]`;
  await mouseClickIfThere(evaluate, tab);
  await until("the tab shown before Preferences", () =>
    evaluate(`(() => {
      const tab = document.querySelector('${tab}');
      return !tab || tab.getAttribute("aria-selected") === "true";
    })()`),
  );
}

// A public key the test server does not have: stored as the server's key,
// it makes the server's real key look changed.
const OTHER_KEY_FILE = "crates/core/tests/fixtures/keys/rsa_client.pub";

const children = [];
/** Every copy of the app started, and the network log of each one's
 * WebView2 (#115). */
const apps = [];
const netLogs = { file: "", copies: 0, found: [] };
/** The watch on the app's connections over the whole run (#115). */
let connections = null;
const userData = mkdtempSync(join(tmpdir(), "netterm-smoke-"));
// The app's own data folder, listing workspaces but none to reopen, so the
// first window is the welcome view.
const appData = join(userData, "app-data");
const workspace = join(userData, "smoke-workspace");
// A workspace on the list whose folder is gone (marked in the welcome view).
const goneWorkspace = join(userData, "gone-workspace");
// Where the app writes the web links it would open in the browser (the
// smoke test never starts a browser).
const openedLinks = join(userData, "opened-links.txt");
/** The folder the test SSH server serves over SFTP, as `/` (#103). */
const sftpFolder = join(userData, "sftp");
/** The folder the SCP-only server serves, as `/` (#106). */
const scpFolder = join(userData, "scp");
// Beside the built program, the first makes the app an installed copy (the
// installer writes its uninstaller there; the smoke's is an empty file,
// never run), the second a portable one (the release script puts it in the
// portable zip). The first launch is "installed", the restart "portable".
const INSTALLED_MARKER = join(APP_FOLDER, "uninstall.exe");
const PORTABLE_MARKER = join(APP_FOLDER, "portable.txt");
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
    JSON.stringify({ workspaces: [workspace, goneWorkspace] }),
  );
}

/** The headless binary the app build puts beside the app prints the smoke
 * workspace's inventory, its problem reported but not fatal. */
function headlessInventory() {
  const result = spawnSync(
    join(APP_FOLDER, "netterm-cli.exe"),
    ["inventory", workspace, "--folder", "Lab"],
    { encoding: "utf8" },
  );
  if (result.error) fail(`netterm-cli did not start: ${result.error.message}`);
  if (result.status !== 0) {
    fail(`netterm-cli exited with ${result.status}: ${result.stderr}`);
  }
  const inventory = JSON.parse(result.stdout);
  const ids = inventory.hosts.map((host) => host.id).join(", ");
  if (ids !== "Lab/smoke-r1.yaml") fail(`netterm-cli exported ${ids}`);
  if (inventory.hosts[0].settings.port !== SSH_PORT) {
    fail(`netterm-cli gave port ${inventory.hosts[0].settings.port}`);
  }
  if (!inventory.problems.some((problem) => problem.file === "broken.yaml")) {
    fail("netterm-cli did not report broken.yaml");
  }
  if (!result.stderr.includes("warning: broken.yaml")) {
    fail(`netterm-cli did not warn about broken.yaml: ${result.stderr}`);
  }
  console.log("ok - the headless binary exports the inventory");
}

/** The headless binary asks the running app for `host`'s credentials
 * (#49), as a script would, with no console to ask a master password on
 * (`windowsHide`). */
function headlessCredentials(host) {
  return spawnSync(
    join(APP_FOLDER, "netterm-cli.exe"),
    ["credentials", workspace, host],
    {
      encoding: "utf8",
      windowsHide: true,
      env: { ...process.env, NETTERM_DATA_DIR: appData },
      timeout: 30_000,
    },
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

/**
 * Text on screen that does not stand out enough from what is behind it
 * (#79): WCAG AA, 4.5:1, or 3:1 for large text (24 px, or 18.66 px bold),
 * in the theme shown and then in the other one (the theme attribute is
 * swapped and put back at once, transitions off). Judged: every element
 * holding visible text and every filled field; not judged: terminals
 * (their colours are the device's), disabled controls (exempt in WCAG),
 * text over an image or gradient. The background is the element's own
 * and its ancestors' composited; opacity along the way dims the text.
 */
const CONTRAST_PROBE = `(() => {
  const parse = (c) => {
    let m = /^rgba?\\(([^)]+)\\)$/.exec(c);
    if (m) {
      const p = m[1].split(/[\\s,\\/]+/).filter(Boolean).map(parseFloat);
      return [p[0], p[1], p[2], p.length > 3 ? p[3] : 1];
    }
    m = /^color\\(srgb ([^)]+)\\)$/.exec(c);
    if (m) {
      const p = m[1].split(/[\\s\\/]+/).filter(Boolean).map(parseFloat);
      return [p[0] * 255, p[1] * 255, p[2] * 255, p.length > 3 ? p[3] : 1];
    }
    return null;
  };
  const over = (top, bottom) =>
    [0, 1, 2].map((i) => top[i] * top[3] + bottom[i] * (1 - top[3])).concat(1);
  const luminance = (c) => {
    const [r, g, b] = c.slice(0, 3).map((v) => {
      v /= 255;
      return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
    });
    return 0.2126 * r + 0.7152 * g + 0.0722 * b;
  };
  const ratio = (a, b) => {
    const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
    return (x + 0.05) / (y + 0.05);
  };
  const hex = (c) =>
    "#" + c.slice(0, 3).map((v) => Math.round(v).toString(16).padStart(2, "0")).join("");
  const describe = (element) => {
    const marks = [...element.attributes]
      .filter((a) => a.name.startsWith("data-") || a.name === "class" || a.name === "role")
      .slice(0, 3)
      .map((a) => "[" + a.name + "=" + a.value.slice(0, 40) + "]")
      .join("");
    const holder = element.closest("[data-view], [data-side-view], [data-dialog], [role=dialog], [role=alertdialog], header, nav, .slot");
    const where = holder && holder !== element
      ? holder.tagName.toLowerCase() + (holder.dataset.view ? "[data-view=" + holder.dataset.view + "]" : holder.dataset.sideView ? "[data-side-view=" + holder.dataset.sideView + "]" : "") + " "
      : "";
    const text = (element.value ?? element.textContent ?? "").trim().replace(/\\s+/g, " ").slice(0, 30);
    return where + element.tagName.toLowerCase() + marks + " " + JSON.stringify(text);
  };
  const judge = () => {
    // Styles and backgrounds are looked up once per element and theme.
    const styles = new Map();
    const styleOf = (e) => {
      let s = styles.get(e);
      if (!s) styles.set(e, (s = getComputedStyle(e)));
      return s;
    };
    const behinds = new Map();
    const opacities = new Map();
    const opacityOf = (e) => {
      if (!e) return 1;
      let o = opacities.get(e);
      if (o === undefined) {
        o = parseFloat(styleOf(e).opacity) * opacityOf(e.parentElement);
        opacities.set(e, o);
      }
      return o;
    };
    const rootColour = parse(getComputedStyle(document.documentElement).backgroundColor) ?? [255, 255, 255, 1];
    const background = (element) => {
      if (behinds.has(element)) return behinds.get(element);
      const layers = [];
      let image = false;
      for (let e = element; e; e = e.parentElement) {
        const s = styleOf(e);
        if (s.backgroundImage !== "none") {
          image = true;
          break;
        }
        if (s.backgroundImage !== "none") return null;
        const c = parse(s.backgroundColor);
        if (c && c[3] > 0) {
          layers.push(c);
          if (c[3] >= 1) break;
        }
      }
      let colour = rootColour[3] >= 1 ? rootColour : over(rootColour, [255, 255, 255, 1]);
      for (const layer of layers.reverse()) colour = over(layer, colour);
      if (image) colour = null;
      behinds.set(element, colour);
      return colour;
    };
    const elements = new Set();
    const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (node.textContent.trim() && node.parentElement) elements.add(node.parentElement);
    }
    for (const field of document.querySelectorAll("input, textarea, select")) {
      if (["checkbox", "radio", "range", "color", "hidden", "file"].includes(field.type)) continue;
      if (field.value) elements.add(field);
    }
    const found = [];
    for (const element of elements) {
      if (["SCRIPT", "STYLE", "OPTION", "TITLE", "NOSCRIPT", "DATALIST"].includes(element.tagName)) continue;
      if (element.closest(".xterm, [hidden], :disabled, [aria-disabled=true]")) continue;
      if (!element.checkVisibility({ visibilityProperty: true, opacityProperty: true })) continue;
      const box = element.getBoundingClientRect();
      if (box.width < 2 || box.height < 2) continue;
      const style = styleOf(element);
      const colour = parse(style.color);
      const behind = background(element);
      if (!colour || !behind) continue;
      const opacity = opacityOf(element);
      const shown = over([colour[0], colour[1], colour[2], colour[3] * opacity], behind);
      const size = parseFloat(style.fontSize);
      const large = size >= 24 || (size >= 18.66 && parseInt(style.fontWeight, 10) >= 700);
      const needed = large ? 3 : 4.5;
      const got = ratio(shown, behind);
      if (got < needed - 0.005) {
        found.push(describe(element) + ": " + hex(shown) + " on " + hex(behind) + " is " + got.toFixed(2) + ":1");
      }
    }
    return found;
  };
  const calm = document.createElement("style");
  calm.textContent = "*, *::before, *::after { transition: none !important; animation: none !important; }";
  document.head.append(calm);
  const root = document.documentElement;
  const theme = root.dataset.theme === "light" ? "light" : "dark";
  const other = theme === "light" ? "dark" : "light";
  const result = {};
  try {
    result[theme] = judge();
    root.dataset.theme = other;
    result[other] = judge();
  } finally {
    root.dataset.theme = theme;
    calm.remove();
  }
  return result;
})()`;

/** Contrast failures found so far, by theme and element, with where. */
const contrastFailures = new Map();

/** Checks the contrast of what is on screen now in both themes (#79);
 * failures are kept and reported together at the end of the run. */
async function checkContrast(evaluate, where) {
  const found = await evaluate(CONTRAST_PROBE);
  for (const [theme, failures] of Object.entries(found ?? {})) {
    for (const failure of failures) {
      const key = `${theme}: ${failure}`;
      if (!contrastFailures.has(key)) contrastFailures.set(key, where);
    }
  }
}

/** The side view's width the sidePanels part leaves for the restart. */
const SIDE_PANEL_WIDTH = 400;
/** The side view it leaves selected, for the restart to find. */
const SIDE_VIEW_KEPT = "snippets";

/** Shows the side view `view` by its rail icon, unless it is shown. */
async function showSideView(evaluate, view) {
  const shown = `!!document.querySelector("[data-side-view=${view}]")`;
  if (!(await evaluate(shown))) {
    await mouseClick(evaluate, `[data-rail=${view}]`);
  }
  await until(`the ${view} side view`, () => evaluate(shown));
  // Saved before going on, so that a restart finds it (hosts is the default).
  await until(`the ${view} side view saved`, () => {
    const file = join(appData, "preferences.yaml");
    const saved = existsSync(file)
      ? (/^side_view: *(\S+)/m.exec(readFileSync(file, "utf8"))?.[1] ?? "hosts")
      : "hosts";
    return saved === view;
  });
}

/** Opens the details sheet on the tree's selection by its row's details
 * control, unless the sheet is open (it follows the selection). */
async function showDetails(evaluate) {
  const open = `!!document.querySelector("[data-side-view=hosts] [data-details-sheet]")`;
  if (!(await evaluate(open))) {
    await mouseClickElement(
      evaluate,
      `document.querySelector("nav [data-item][aria-pressed=true], nav [data-item].selected")?.parentElement.querySelector(":scope > [data-row-details]")`,
      "the selected row's details control",
    );
  }
  await until("the details sheet", () => evaluate(open));
}

/** Opens the run `id` (else the first listed) from the Jobs side view in
 * its tab, then shows the Hosts view again; resolves to the run's id. */
async function openRun(evaluate, id = null) {
  await showSideView(evaluate, "jobs");
  const row = id
    ? `${JOBS_SIDE} [data-run="${id}"]`
    : `${JOBS_SIDE} [data-run]`;
  const run = await until("the run listed", () =>
    evaluate(
      `document.querySelector(${JSON.stringify(row)})?.dataset.run ?? null`,
    ),
  );
  await mouseClick(evaluate, row);
  await until("the run's tab", () =>
    evaluate(
      `!!document.querySelector(${JSON.stringify(`${RUN_TAB} [data-run-details="${run}"]`)})`,
    ),
  );
  await showSideView(evaluate, "hosts");
  return run;
}

/** Closes, by its tab's close button, the tab whose slot holds (or is)
 * `selector`. */
async function closeTabOf(evaluate, selector) {
  const tab = await evaluate(
    `document.querySelector(${JSON.stringify(selector)})?.closest(".slot")?.dataset.tab ?? null`,
  );
  if (tab === null) fail(`no tab holds ${selector}`);
  await mouseClick(
    evaluate,
    `[data-tab-bar] [data-tab="${tab}"] [data-tab-close]`,
  );
  await until(
    `the tab of ${selector} closed`,
    async () =>
      !(await evaluate(`!!document.querySelector('.slot[data-tab="${tab}"]')`)),
  );
}

/** The Credentials side view (profiles, secrets, the vault's state). */
const CREDENTIALS = "[data-credentials]";

/** Shows the Credentials side view with its lists (no form open). */
async function openProfiles(evaluate) {
  await showSideView(evaluate, "credentials");
  await until("the credentials lists", () =>
    evaluate(`!!document.querySelector("${CREDENTIALS}[data-mode=list]")`),
  );
}

/** Opens the new-profile form of the Credentials side view. */
async function newProfile(evaluate) {
  await openProfiles(evaluate);
  await mouseClick(evaluate, `${CREDENTIALS} [data-profile-new]`);
  await until("the profile form", () =>
    evaluate(`!!document.querySelector("${CREDENTIALS} [name=profile-name]")`),
  );
}

// The rail and the side view: each icon shows its view and says so,
// clicking the open one folds the side view and any icon brings it back;
// each view has its shortcut. Dragging the side view's edge resizes it
// (saved when the drag ends), double-clicking resets it and the terminal
// keeps its room. There is no right column: details open as a sheet over
// the Hosts view (from the row, the context menu or a key), follow the
// selection and close back to the tree.
async function sidePanels({
  evaluate,
  mouse,
  centreOf,
  devtools,
  screenshot,
  step,
}) {
  const width = (selector) =>
    evaluate(
      `Math.round(document.querySelector(${JSON.stringify(selector)})?.getBoundingClientRect().width ?? -1)`,
    );
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${JSON.stringify(selector)})`);
  const left = "[data-side-view]";
  const drag = async (selector, dx) => {
    const from = await centreOf(selector);
    // Off the hide button in the middle of the handle.
    const start = { x: from.x, y: from.y - 80 };
    await mouse("mouseMoved", start);
    await mouse("mousePressed", start, { button: "left", clickCount: 1 });
    for (let step = 1; step <= 5; step++) {
      await mouse(
        "mouseMoved",
        { x: start.x + (dx * step) / 5, y: start.y },
        { button: "left", buttons: 1 },
      );
    }
    await mouse(
      "mouseReleased",
      { x: start.x + dx, y: start.y },
      { button: "left", clickCount: 1 },
    );
  };
  const shortcut = async (letter) => {
    const press = {
      key: letter,
      code: `Key${letter}`,
      windowsVirtualKeyCode: letter.charCodeAt(0),
      modifiers: 2 | 8,
    };
    await devtools.send("Input.dispatchKeyEvent", {
      type: "keyDown",
      ...press,
    });
    await devtools.send("Input.dispatchKeyEvent", { type: "keyUp", ...press });
  };
  const railClick = (view) => mouseClick(evaluate, `[data-rail=${view}]`);
  const openView = () =>
    evaluate(
      `document.querySelector("[data-rail][data-active]")?.dataset.rail ?? null`,
    );

  // Room for wider panels: the window is widened for this part (the
  // terminal keeps 320 px, so at 800 px neither panel could grow).
  const { windowId, bounds } = await devtools.send(
    "Browser.getWindowForTarget",
  );
  await devtools.send("Browser.setWindowBounds", {
    windowId,
    bounds: { width: 1400, height: bounds.height },
  });
  await until(
    "the wider window",
    async () => (await evaluate("window.innerWidth")) >= 1300,
  );

  // Every icon has a name and a tooltip; Hosts is open.
  const icons = await evaluate(`[...document.querySelectorAll(
    "[data-rail], [data-open-preferences]")].map((b) => ({
      name: b.getAttribute("aria-label"), title: b.title }))`);
  if (
    icons.length !== 6 ||
    icons.some((icon) => !icon.name || !icon.title.includes(icon.name))
  ) {
    fail(`the rail's icons: ${JSON.stringify(icons)}`);
  }
  await showSideView(evaluate, "hosts");
  if ((await openView()) !== "hosts") fail("the rail does not show Hosts");
  // The toolbar of text buttons is gone.
  if (await exists(".toolbar")) fail("the toolbar is still there");
  step(`the rail has ${icons.length} named icons with tooltips`);

  // Each icon shows its view; the open one folds it; any icon brings it back.
  for (const view of ["snippets", "jobs", "backups", "credentials", "hosts"]) {
    await railClick(view);
    await until(
      `the ${view} view from the rail`,
      async () =>
        (await exists(`[data-side-view=${view}]`)) &&
        (await openView()) === view,
    );
  }
  await railClick("hosts");
  await until(
    "the side view folded",
    async () => !(await exists(left)) && (await openView()) === null,
  );
  await screenshot("side-view-folded");
  await railClick("jobs");
  await until("the jobs view after folding", () =>
    exists("[data-side-view=jobs]"),
  );
  step("the rail switches the side view, folds it, and opens it again");

  // By keyboard: each view's letter, and Ctrl+Shift+B folds.
  for (const [letter, view] of [
    ["H", "hosts"],
    ["S", "snippets"],
    ["J", "jobs"],
    ["G", "backups"],
    ["L", "credentials"],
  ]) {
    await shortcut(letter);
    await until(`the ${view} view by Ctrl+Shift+${letter}`, () =>
      exists(`[data-side-view=${view}]`),
    );
  }
  await shortcut("B");
  await until(
    "the side view folded by Ctrl+Shift+B",
    async () => !(await exists(left)),
  );
  await shortcut("B");
  await until("the side view back by Ctrl+Shift+B", () =>
    exists("[data-side-view=credentials]"),
  );
  await shortcut("H");
  await until("the hosts view by Ctrl+Shift+H", () =>
    exists("[data-side-view=hosts]"),
  );
  step("each side view and folding have a shortcut");

  // A host is selected: no details until asked, and no right column.
  const sheet = "[data-side-view=hosts] [data-details-sheet]";
  const sheetName = () =>
    evaluate(`document.querySelector("${sheet} h2")?.textContent ?? null`);
  await mouseClick(evaluate, `nav [data-host]`);
  await sleep(300);
  if (await exists(sheet)) fail("selecting a host opened its details");
  if (
    await exists(
      "[data-panel-resizer=right], [data-reveal-panel], .layout > aside.side",
    )
  ) {
    fail("the right column is still there");
  }
  const columnsRoom = await evaluate(`(() => {
    const rail = document.querySelector(".layout > :first-child").getBoundingClientRect().width;
    const side = document.querySelector("[data-side-view]").getBoundingClientRect().width;
    const centre = document.querySelector(".layout > .centre").getBoundingClientRect();
    return Math.abs(rail + side + centre.width - window.innerWidth) <= 2;
  })()`);
  if (!columnsRoom)
    fail("the terminal area does not take the rest of the window");
  step(
    "with a host selected there is no right column; the terminals take the space",
  );

  // From the row's details control: a sheet over the side view, never over
  // a terminal.
  const firstHost = await evaluate(
    `document.querySelector("nav [data-host]").dataset.label`,
  );
  await mouseClick(evaluate, `nav li:has(> [data-host]) > [data-row-details]`);
  await until(
    "the details sheet from the row",
    async () => (await sheetName()) === firstHost,
  );
  const sheetPlace = await evaluate(`(() => {
    const sheet = document.querySelector("${sheet}").getBoundingClientRect();
    const side = document.querySelector("[data-side-view]").getBoundingClientRect();
    const centre = document.querySelector(".layout > .centre").getBoundingClientRect();
    const sections = [...document.querySelectorAll("${sheet} [data-details-section]")].map((s) => s.dataset.detailsSection);
    return { inside: sheet.left >= side.left - 0.5 && sheet.right <= side.right + 0.5,
      clear: sheet.right <= centre.left + 0.5, tree: !!document.querySelector("[data-side-view=hosts] nav"),
      ways: !!document.querySelector("${sheet} [data-ways-in] [data-connect=default]"),
      sections };
  })()`);
  if (
    !sheetPlace.inside ||
    !sheetPlace.clear ||
    !sheetPlace.tree ||
    !sheetPlace.ways
  ) {
    fail(`the details sheet's place: ${JSON.stringify(sheetPlace)}`);
  }
  if (
    sheetPlace.sections.join(",") !==
    "device,connection,tags,custom,description"
  ) {
    fail(`the sheet's sections: ${sheetPlace.sections.join(",")}`);
  }
  await screenshot("details-sheet");
  step(
    "a row's details control opened the sheet over the side view, with the tree above it",
  );

  // It follows the selection in the tree, folders too (the workspace's
  // settings control selects the top folder).
  const firstId = await evaluate(
    `document.querySelector("nav [data-host]").dataset.host`,
  );
  const rootName = await evaluate(
    `document.querySelector("[data-open-workspaces]").textContent.trim()`,
  );
  await mouseClick(evaluate, `[data-workspace-settings]`);
  await until(
    "the sheet following the selection to a folder",
    async () =>
      (await exists("[data-details-sheet=folder]")) &&
      (await sheetName()) === rootName,
  );
  await mouseClick(evaluate, `nav [data-host="${firstId}"]`);
  await until(
    "the sheet back on the host",
    async () => (await sheetName()) === firstHost,
  );
  step("the open sheet follows the selection in the tree");

  // Back to the tree.
  await mouseClick(evaluate, `${sheet} [data-details-close]`);
  await until("the sheet closed by Back", async () => !(await exists(sheet)));
  // By key.
  await shortcut("D");
  await until("the sheet opened by Ctrl+Shift+D", () => exists(sheet));
  await shortcut("D");
  await until(
    "the sheet closed by Ctrl+Shift+D",
    async () => !(await exists(sheet)),
  );
  // From the context menu, which holds every action of the details.
  await mouseClick(evaluate, "nav [data-host]", { button: "right" });
  await until("the host's context menu", () =>
    exists("[role=menu] [data-menu]"),
  );
  const menuEntries = await until("the menu's ways in", () =>
    evaluate(`(() => {
      const ids = [...document.querySelectorAll("[role=menu] [data-menu]")].map((b) => b.dataset.menu);
      return ids.filter((id) => id.startsWith("connect-")).length === 2 && ids;
    })()`),
  );
  for (const entry of [
    "connect",
    "details",
    "run-snippet",
    "run-script",
    "back-up",
    "enrich",
    "config-history",
    "log-folder",
    "move",
    "delete",
  ]) {
    if (!menuEntries.includes(entry)) {
      fail(`the context menu has no ${entry}: ${menuEntries.join(", ")}`);
    }
  }
  await screenshot("host-context-menu");
  await mouseClick(evaluate, `[role=menu] [data-menu=details]`);
  await until("the sheet opened from the context menu", () => exists(sheet));
  // Another side view takes its place; Hosts comes back without it.
  await railClick("snippets");
  await until("the snippets view", () => exists("[data-side-view=snippets]"));
  await railClick("hosts");
  await until("the hosts view", () => exists("[data-side-view=hosts]"));
  if (await exists(sheet)) fail("the sheet outlived its side view");
  step(
    `details open from the context menu (${menuEntries.length} entries) and by Ctrl+Shift+D, and close back to the tree`,
  );

  const before = await width(left);
  await drag("[data-panel-resizer=left]", 150);
  await until(
    "the wider side view",
    async () => Math.abs((await width(left)) - (before + 150)) <= 2,
  );
  step(
    `dragging the side view's edge widened it (${before} to ${before + 150} px)`,
  );

  // As far as it goes: the terminal keeps at least 320 px.
  await drag("[data-panel-resizer=left]", 3000);
  await until(
    "the terminal area keeps its room",
    async () => (await width(".layout > .centre")) >= 318,
  );
  step("the side view cannot squeeze the terminal below 320 px");

  // On the handle, off the hide button in its middle (as for the drags).
  const handle = await centreOf("[data-panel-resizer=left]");
  const onHandle = { x: handle.x, y: handle.y - 80 };
  if (
    !(await evaluate(
      `document.elementFromPoint(${onHandle.x}, ${onHandle.y}) === document.querySelector("[data-panel-resizer=left]")`,
    ))
  )
    fail("the side view's edge cannot be reached off its hide button");
  await mouseAt(evaluate, onHandle, { double: true });
  await until(
    "the default width after a double-click",
    async () => (await width(left)) === 288,
  );
  step("double-clicking the edge restored the default width");

  // The edge's own button folds the side view too.
  await mouseClick(evaluate, `[data-hide-panel=left]`);
  await until(
    "the side view folded by its edge",
    async () => !(await exists(left)),
  );
  await railClick("hosts");
  await until("the side view back", () => exists(left));

  // A width for the restart to find, set by dragging.
  await drag(
    "[data-panel-resizer=left]",
    SIDE_PANEL_WIDTH - (await width(left)),
  );
  await until(
    `the side view at ${SIDE_PANEL_WIDTH} px`,
    async () => (await width(left)) === SIDE_PANEL_WIDTH,
  );
  await showSideView(evaluate, SIDE_VIEW_KEPT);
  // The width is saved through the core after the drag ends.
  await until(`the width ${SIDE_PANEL_WIDTH} saved`, () =>
    new RegExp(`^left_panel_width: *${SIDE_PANEL_WIDTH}$`, "m").test(
      readFileSync(join(appData, "preferences.yaml"), "utf8"),
    ),
  );
  await screenshot("panels-resized");
  await devtools.send("Browser.setWindowBounds", { windowId, bounds });
}

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
    await sleep(100);
  }
  fail(`timed out waiting for ${description} (last: ${String(last)})`);
}

/**
 * Fails unless a mouse at the centre of `selector` would reach it: nothing
 * drawn over it takes the click (the terminal's WebGL layers once covered
 * the questions drawn over a terminal). Resolves to that centre.
 */
async function reachable(evaluate, selector) {
  return reachableElement(
    evaluate,
    `document.querySelector(${JSON.stringify(selector)})`,
    selector,
  );
}

/**
 * `reachable` for the element the page expression `element` gives (one
 * found by its text, say); `what` names it in a failure.
 */
async function reachableElement(evaluate, element, what) {
  const hit = await hitOf(evaluate, element);
  if (!hit) fail(`${what} is not on screen`);
  if (!hit.reached) fail(`a click on ${what} would land on ${hit.top}`);
  return hit;
}

/** Where a mouse at the centre of `element` would land (null: no such
 * element). */
function hitOf(evaluate, element) {
  return evaluate(`(() => {
    const element = (${element});
    if (!element) return null;
    element.scrollIntoView({ block: "nearest" });
    // The part a user sees: the box cut by every ancestor that clips
    // (a tab half out of its scrolled strip is clicked on its visible half).
    const box = element.getBoundingClientRect();
    let [left, top_, right, bottom] = [box.left, box.top, box.right, box.bottom];
    for (let around = element.parentElement; around; around = around.parentElement) {
      if (getComputedStyle(around).overflow === "visible") continue;
      const clip = around.getBoundingClientRect();
      left = Math.max(left, clip.left);
      top_ = Math.max(top_, clip.top);
      right = Math.min(right, clip.right);
      bottom = Math.min(bottom, clip.bottom);
    }
    left = Math.max(left, 0);
    top_ = Math.max(top_, 0);
    right = Math.min(right, window.innerWidth);
    bottom = Math.min(bottom, window.innerHeight);
    const seen = right > left && bottom > top_;
    const x = seen ? (left + right) / 2 : box.x + box.width / 2;
    const y = seen ? (top_ + bottom) / 2 : box.y + box.height / 2;
    const top = document.elementFromPoint(x, y);
    return {
      x,
      y,
      reached: !!top && (top === element || element.contains(top)),
      // What is there, by the control (or element) holding that point.
      top: top
        ? (top.closest("button, a, input, [role], [data-tab]") ?? top).cloneNode(false).outerHTML.slice(0, 160)
        : null,
    };
  })()`);
}

/**
 * Presses and releases a mouse button at `point` with real input events
 * over DevTools (`evaluate` from `evaluateIn` knows its page). `double`
 * sends a second click that the page sees as a double-click; `button`
 * "right" opens a context menu. `mayClose`: the click may close the page
 * (a window's Close), so a send it no longer answers is not a failure
 * (whether the control was reachable is checked before). `modifiers`: the
 * keys held, as DevTools counts them (`CTRL` 2, `SHIFT` 8).
 */
async function mouseAt(
  evaluate,
  { x, y },
  { button = "left", double = false, mayClose = false, modifiers = 0 } = {},
) {
  const send = (type, clickCount) =>
    evaluate.devtools.send("Input.dispatchMouseEvent", {
      type,
      x,
      y,
      button: type === "mouseMoved" ? "none" : button,
      clickCount,
      modifiers,
    });
  try {
    await send("mouseMoved", 0);
    for (const clickCount of double ? [1, 2] : [1]) {
      await send("mousePressed", clickCount);
      await send("mouseReleased", clickCount);
    }
  } catch (error) {
    if (!mayClose) throw error;
  }
}

/**
 * Clicks `selector` with the mouse, as a user does (#94): fails unless it
 * is the top element at its centre, so a control something is drawn over
 * fails the test. `options` as `mouseAt`.
 */
async function mouseClick(evaluate, selector, options) {
  await mouseClickElement(
    evaluate,
    `document.querySelector(${JSON.stringify(selector)})`,
    selector,
    options,
  );
}

/** `mouseClick` on the element the page expression `element` gives. */
async function mouseClickElement(evaluate, element, what, options) {
  // The mouse gets there first, as a user's does: controls shown on hover
  // (a snippet's Edit) appear.
  const over = await hitOf(evaluate, element);
  if (over) {
    await evaluate.devtools.send("Input.dispatchMouseEvent", {
      type: "mouseMoved",
      x: over.x,
      y: over.y,
      button: "none",
    });
  }
  await mouseAt(evaluate, await settled(evaluate, element, what), options);
}

/**
 * The reachable centre of `element` once it stays put: a list that is
 * still filling in (the palette's hits, say) can move the element between
 * the look and the click, as it would under a user's mouse.
 */
async function settled(evaluate, element, what) {
  let last = await reachableElement(evaluate, element, what);
  for (let tries = 0; tries < 20; tries++) {
    await sleep(40);
    const now = await reachableElement(evaluate, element, what);
    if (now.x === last.x && now.y === last.y) return now;
    last = now;
  }
  fail(`${what} kept moving`);
}

/** The page expression for the visible button labelled `label` (within
 * `scope`, a selector). */
const buttonLabelled = (label, scope = "") =>
  `[...document.querySelectorAll(${JSON.stringify(`${scope} button`.trim())})].find((b) => b.textContent.trim() === ${JSON.stringify(label)} && b.checkVisibility())`;

/** Clicks with the mouse the visible button labelled `label`. */
const clickButton = (evaluate, label, scope = "") =>
  mouseClickElement(
    evaluate,
    buttonLabelled(label, scope),
    `the button ${label}`,
  );

/** Opens the tree's folder row `selector` with the mouse unless it is
 * open. */
async function expandFolder(evaluate, selector) {
  const open = await evaluate(
    `document.querySelector(${JSON.stringify(selector)}).getAttribute("aria-expanded") === "true"`,
  );
  if (!open) await mouseClick(evaluate, selector);
}

/** Each pane tool's button in a tab bar (and the new-tab button), by its
 * `data-session-action` in the "…" menu that holds them when they are
 * folded (#100). */
const PANE_TOOLS = {
  "new-tab": "[data-new-tab]",
  "split-right": "[data-split=right]",
  "split-down": "[data-split=down]",
  "close-pane": "[data-close-pane]",
  "join-pane": "[data-join-pane]",
};

/**
 * Uses the pane tool `tool` of the tab bar the page expression `bar`
 * gives, with the mouse: its button, or, when the bar has folded its tools
 * (a narrow top-right pane, #100), the "…" button and the entry of its
 * menu. `what` names the pane in a failure. Resolves to whether the tools
 * were folded.
 */
async function paneTool(evaluate, bar, tool, what) {
  const more = `(${bar}).querySelector("[data-pane-tools-more]")`;
  if (!(await evaluate(`!!${more}`))) {
    await mouseClickElement(
      evaluate,
      `(${bar}).querySelector(${JSON.stringify(PANE_TOOLS[tool])})`,
      `${what}: ${tool}`,
    );
    return false;
  }
  await mouseClickElement(evaluate, more, `${what}: pane actions`);
  const entry = `[data-session-actions] [data-session-action=${tool}]`;
  await until(`${what}: ${tool} in the pane actions`, () =>
    evaluate(`!!document.querySelector(${JSON.stringify(entry)})`),
  );
  await mouseClick(evaluate, entry);
  return true;
}

/**
 * The entries of the "…" menu of the tab bar the page expression `bar`
 * gives (#100), opened with the mouse and closed with Escape; null when
 * the bar's tools are not folded. Fails unless the window's buttons and
 * the bar's shown tab can still be clicked.
 */
async function foldedToolEntries(evaluate, bar, what) {
  const more = `(${bar}).querySelector("[data-pane-tools-more]")`;
  if (!(await evaluate(`!!${more}`))) return null;
  await reachable(evaluate, "[data-window-control=close]");
  await reachableElement(
    evaluate,
    `(${bar}).querySelector("[data-tab][aria-selected=true]")`,
    `${what}: the shown tab`,
  );
  await mouseClickElement(evaluate, more, `${what}: pane actions`);
  const entries = await until(`${what}: the pane actions`, () =>
    evaluate(`(() => {
      const ids = [...document.querySelectorAll("[data-session-actions] [data-session-action]")].map((entry) => entry.dataset.sessionAction);
      return ids.length > 0 && ids;
    })()`),
  );
  for (const type of ["keyDown", "keyUp"]) {
    await evaluate.devtools.send("Input.dispatchKeyEvent", {
      type,
      key: "Escape",
      code: "Escape",
      windowsVirtualKeyCode: 27,
    });
  }
  await until(
    `${what}: the pane actions closed`,
    async () =>
      !(await evaluate(`!!document.querySelector("[data-session-actions]")`)),
  );
  return entries;
}

/** Clicks `selector` with the mouse if it is on the page. */
async function mouseClickIfThere(evaluate, selector) {
  if (await evaluate(`!!document.querySelector(${JSON.stringify(selector)})`))
    await mouseClick(evaluate, selector);
}

/** The ids of the open session menu's entries (header or tab, #67). */
const menuEntries = (evaluate) =>
  evaluate(
    `[...document.querySelectorAll("[data-session-actions] [data-session-action]")].map((b) => b.dataset.sessionAction)`,
  );

/**
 * Runs one of a session's actions (`data-session-action`: duplicate,
 * disconnect, reconnect, new-connection, multi-send, end-group,
 * send-break, details) from the menu on its header line (#67). The menu
 * is opened again until the action is offered (a state that just changed
 * may not have reached the menu yet: the entry was missing now and then
 * right after a connect); throws if it still is not after `within` ms.
 */
async function sessionAction(
  evaluate,
  action,
  session = SHOWN_SESSION,
  within = 5000,
) {
  const menu = `${session} [data-session-menu]`;
  const deadline = Date.now() + within;
  for (;;) {
    await mouseClick(evaluate, menu);
    await until("the session's menu", () =>
      evaluate(`!!document.querySelector("[data-session-actions]")`),
    );
    const entry = `[data-session-actions] [data-session-action=${action}]`;
    const offered = await evaluate(`(() => {
      const entry = document.querySelector(${JSON.stringify(entry)});
      return !!entry && !entry.disabled;
    })()`);
    if (offered) {
      await mouseClick(evaluate, entry);
      break;
    }
    await mouseClick(evaluate, menu);
    await until("the session's menu closed", async () =>
      evaluate(`!document.querySelector("[data-session-actions]")`),
    );
    if (Date.now() >= deadline) {
      throw new Error(`the session offers no ${action} now`);
    }
    await sleep(250);
  }
  await until("the session's menu closed", async () =>
    evaluate(`!document.querySelector("[data-session-actions]")`),
  );
}

/**
 * Fails unless the session's header is still one line of its own height,
 * whatever markers it shows (`expected`: selectors that must be in it).
 */
async function checkHeaderLine(evaluate, session, expected = []) {
  const line = await evaluate(`(() => {
    const header = document.querySelector(${JSON.stringify(`${session} .session header`)});
    const box = header.getBoundingClientRect();
    const outside = [...header.children].filter((child) => {
      const each = child.getBoundingClientRect();
      return each.width > 0 && (each.top < box.top - 0.5 || each.bottom > box.bottom + 0.5);
    }).length;
    const missing = ${JSON.stringify(expected)}.filter((selector) => !header.querySelector(selector));
    return { height: box.height, outside, missing };
  })()`);
  if (line.height !== 30 || line.outside > 0 || line.missing.length > 0) {
    fail(`the session header is not one line: ${JSON.stringify(line)}`);
  }
}

function startServer() {
  // One built beforehand (SMOKE_TEST_SERVER) runs as it is.
  let program = process.env.SMOKE_TEST_SERVER;
  if (!program) {
    const build = spawnSync(
      "cargo",
      ["build", "-p", "netterm-core", "--example", "test_ssh_server"],
      { stdio: "inherit" },
    );
    if (build.status !== 0) fail("could not build the test SSH server");
    program = "target/debug/examples/test_ssh_server.exe";
  }
  const server = spawn(
    program,
    [
      String(SSH_PORT),
      "--agent",
      AGENT_PIPE,
      "--old-device",
      String(OLD_DEVICE_PORT),
      "--telnet",
      String(TELNET_PORT),
      "--device",
      String(DEVICE_PORT),
      "--sftp",
      sftpFolder,
      "--scp-only",
      String(SCP_PORT),
      scpFolder,
      "--banner",
      String(BANNER_PORT),
    ],
    { stdio: ["ignore", "pipe", "inherit"] },
  );
  children.push(server);
  return new Promise((resolve, reject) => {
    server.stdout.once("data", resolve);
    server.once("exit", () => reject(new Error("the test SSH server exited")));
  });
}

/**
 * WebView2 (runtime 150 and later) ignores WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS
 * in an elevated process, and in Windows Sandbox every process is elevated
 * (no UAC): there the arguments go in the machine's WebView2 policy for
 * netterm.exe instead (SMOKE_WEBVIEW2_POLICY, set by the sandbox run only;
 * it needs administrator rights, which only the sandbox's account has).
 */
function webviewPolicy(browserArguments) {
  const done = spawnSync(
    "reg",
    [
      "add",
      String.raw`HKLM\Software\Policies\Microsoft\Edge\WebView2\AdditionalBrowserArguments`,
      "/v",
      "netterm.exe",
      "/d",
      browserArguments,
      "/f",
    ],
    { encoding: "utf8", windowsHide: true },
  );
  if (done.status !== 0)
    fail(`the WebView2 policy could not be set: ${done.stderr}`);
}

/** Starts the app; `env` adds to (or overrides) its usual environment. */
function startApp(env = {}) {
  // WebView2 logs its requests and the names it looks up. Every copy
  // writes the same file (WebView2 refuses other arguments for a data
  // folder whose browser is still ending), so the last copy's log is read
  // before the next starts and truncates it.
  // SMOKE_NET_LOGS=<folder> keeps the last copy's log there.
  netLogs.file ||= join(
    process.env.SMOKE_NET_LOGS ?? userData,
    "webview-net-log.json",
  );
  harvestNetLog();
  netLogs.copies += 1;
  const browserArguments = `--remote-debugging-port=${DEVTOOLS_PORT} ${netLogArguments(netLogs.file)}`;
  if (process.env.SMOKE_WEBVIEW2_POLICY) webviewPolicy(browserArguments);
  const app = spawn(join(APP_FOLDER, "netterm.exe"), [], {
    stdio: "inherit",
    env: {
      ...process.env,
      WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: browserArguments,
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
      ...env,
    },
  });
  children.push(app);
  apps.push(app);
  return app;
}

/** Keeps what the network log of the copies so far shows. */
function harvestNetLog() {
  if (netLogs.copies === 0) return;
  for (const problem of netLogProblems(netLogs.file)) {
    const found = `copy ${netLogs.copies}: ${problem}`;
    if (!netLogs.found.includes(found)) netLogs.found.push(found);
  }
}

/** Fails if the app connected anywhere it was not sent since the last
 * check, naming too what WebView2's network logs show so far (the names
 * behind the addresses). */
function checkConnections() {
  nothingAskedOfThePagePorts();
  try {
    connections.check();
  } catch (error) {
    harvestNetLog();
    const logged = netLogs.found.map((problem) => `  ${problem}`);
    if (logged.length > 0) {
      error.message += ["", "WebView2's network logs so far:", ...logged].join(
        String.fromCharCode(10),
      );
    }
    throw error;
  }
}

/**
 * The end of the whole-run watch (#115): every copy of the app is ended,
 * then its connections seen over the run and its WebView2's network logs
 * must show nothing but this computer.
 */
async function noConnectionOfItsOwn(step) {
  // A copy killed earlier has a signal, not an exit code.
  const running = apps.filter(
    (app) => app.exitCode === null && app.signalCode === null,
  );
  await Promise.all(
    running.map((app) => {
      const exited = new Promise((resolve) => app.once("exit", resolve));
      app.kill();
      return exited;
    }),
  );
  // The last samples, and WebView2's last log lines.
  await sleep(2000);
  checkConnections();
  harvestNetLog();
  const logged = netLogs.found.map((problem) => `  ${problem}`);
  if (logged.length > 0) {
    fail(
      ["WebView2 reached beyond this computer:", ...logged].join(
        String.fromCharCode(10),
      ),
    );
  }
  const { processes, connections: seen, udp, probes } = connections.summary();
  const names = Object.entries(processes)
    .map(([name, count]) => `${name} ×${count}`)
    .join(", ");
  const total = Object.values(seen).reduce((sum, count) => sum + count, 0);
  if (process.env.SMOKE_PAGE_PORT_REPORT) {
    console.log(
      `PAGE PORT REPORT: port ${CLEAR_PORT}: ${pagePorts.clear.connections} connections, ${pagePorts.clear.bytes} bytes; port ${TLS_PORT}: ${pagePorts.tls.connections} connections, ${pagePorts.tls.handshakes} completed TLS handshakes, ${pagePorts.tls.requests.length} requests`,
    );
    for (const request of [
      ...pagePorts.clear.requests,
      ...pagePorts.tls.requests,
    ])
      console.log(`  ${request}`);
  }
  const listened = (port, state, got) =>
    state.listening.length > 0
      ? `port ${port}: ${state.connections} connections, ${got}`
      : `port ${port} was not the test's to listen on (${state.refused})`;
  const pagePort = [
    listened(CLEAR_PORT, pagePorts.clear, "no byte"),
    listened(
      TLS_PORT,
      pagePorts.tls,
      "none trusting the test's certificate, no request",
    ),
  ].join("; ");
  step(
    `over the whole run the app's processes (${names}) made ${total} connections, all to the test or to each other on this computer (where the test listened in place of another program: ${pagePort}), ${udp} UDP sockets (${probes} of them WebView2's IPv6 check, which sends nothing), and WebView2 (the network logs of ${netLogs.copies} copies) looked up, requested and sent nothing beyond it`,
  );
}

/**
 * The ports of this computer WebView2 takes the app's own addresses for
 * (#142). The app's pages are at `https://tauri.localhost/` and its commands
 * go to `https://ipc.localhost/`; WebView2 answers both itself. But for each
 * new page it opens a connection ahead of time (a preconnect), and a command
 * in flight when its window closes or reloads is let through to the network
 * once the window is gone. Over `http://`, as the app was built until 0.4.1,
 * such a command reached port 80 in clear: `POST /session_dock`, with its
 * arguments, to whoever listened there. So the test listens in place of
 * another program, on both loopback addresses:
 *
 * - on port 80, where nothing may arrive at all now;
 * - on port 443, as a web server with a certificate of its own making for
 *   those names: WebView2 must not trust it, so no TLS handshake may
 *   complete and no request may arrive.
 */
const CLEAR_PORT = 80;
const TLS_PORT = 443;
const pagePorts = {
  clear: {
    listening: [],
    refused: "",
    connections: 0,
    bytes: 0,
    first: "",
    requests: [],
  },
  tls: {
    listening: [],
    refused: "",
    connections: 0,
    handshakes: 0,
    requests: [],
  },
  /** The step running, for what arrives during it. */
  at: "at the start",
};

/** Starts `server` on `port` of both loopback addresses' worth of `make`d
 * servers; a port that is taken or refused is said in the run's last step. */
async function listenOnLoopback(port, state, make) {
  for (const address of ["127.0.0.1", "::1"]) {
    await new Promise((resolve) => {
      const server = make();
      server.once("error", (error) => {
        state.refused ||= `${address}: ${error.code ?? error.message}`;
        resolve();
      });
      server.listen(port, address, () => {
        server.unref();
        state.listening.push(address);
        resolve();
      });
    });
  }
}

/** Listens where WebView2 could send the app's pages and commands
 * ([`pagePorts`]). */
async function listenOnThePagePorts() {
  const { clear, tls } = pagePorts;
  await listenOnLoopback(CLEAR_PORT, clear, () =>
    createTcpServer((socket) => {
      clear.connections += 1;
      socket.on("data", (chunk) => {
        clear.bytes += chunk.length;
        const text = chunk.toString("latin1");
        clear.first ||= text.slice(0, 120);
        // Each request's first line and host, never its body.
        for (const [, line] of text.matchAll(
          /^((?:GET|POST|PUT|HEAD|OPTIONS|DELETE) \S+) HTTP\//gm,
        )) {
          const host = /^Host: (\S+)/im.exec(text)?.[1] ?? "?";
          clear.requests.push(`${line} (${host}), ${pagePorts.at}`);
        }
        // SMOKE_PAGE_PORT_REPORT: list what came instead of failing, and
        // hang up as a closed port would, so the app goes on as usual.
        if (process.env.SMOKE_PAGE_PORT_REPORT) socket.destroy();
      });
      socket.on("error", () => {});
    }),
  );
  const fixtures = join("scripts", "fixtures");
  await listenOnLoopback(TLS_PORT, tls, () => {
    const server = createTlsServer(
      {
        key: readFileSync(join(fixtures, "page-port-key.pem")),
        cert: readFileSync(join(fixtures, "page-port-cert.pem")),
      },
      (request, response) => {
        tls.requests.push(
          `${request.method} ${request.url} (${request.headers.host ?? "?"}) over TLS, ${pagePorts.at}`,
        );
        // Answered as a server that knows nothing of it.
        response.writeHead(404).end();
      },
    );
    server.on("connection", () => (tls.connections += 1));
    server.on("secureConnection", () => (tls.handshakes += 1));
    server.on("tlsClientError", () => {});
    return server;
  });
}

/** Fails if WebView2 sent anything of the app's to this computer's ports
 * 80 or 443 (#142). */
function nothingAskedOfThePagePorts() {
  if (process.env.SMOKE_PAGE_PORT_REPORT) return;
  const { clear, tls } = pagePorts;
  if (clear.bytes > 0) {
    fail(
      `WebView2 sent ${clear.bytes} bytes in clear to port ${CLEAR_PORT} of this computer, where another program could listen: ${JSON.stringify(clear.requests)}; they start ${JSON.stringify(clear.first)}`,
    );
  }
  if (tls.handshakes > 0 || tls.requests.length > 0) {
    fail(
      `WebView2 trusted a certificate of the test's making on port ${TLS_PORT} of this computer (${tls.handshakes} TLS handshakes completed) and asked it: ${JSON.stringify(tls.requests)}`,
    );
  }
}

/** The app's pages (one per window) DevTools lists. */
async function pageTargets() {
  const response = await fetch(`http://127.0.0.1:${DEVTOOLS_PORT}/json/list`);
  return (await response.json()).filter((target) => target.type === "page");
}

/** A minimal DevTools protocol client for the app's page. */
async function connectDevTools() {
  const page = await until("the app's webview", async () =>
    (await pageTargets()).at(0),
  );
  return connectTarget(page);
}

/**
 * Connects to the window labelled `label` (#77: a detached run window is
 * `run-<id>`, a page of its own); resolves to its client and `evaluate`.
 */
async function connectWindow(label) {
  return until(`the window ${label}`, async () => {
    for (const target of await pageTargets()) {
      const devtools = await connectTarget(target);
      const evaluate = evaluateIn(devtools);
      const shown = await evaluate(
        "window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label ?? null",
      );
      if (shown === label) return { devtools, evaluate };
      devtools.close();
    }
    return null;
  });
}

/** Whether a window labelled `label` is open (its page listed). */
async function windowOpen(label) {
  for (const target of await pageTargets()) {
    const devtools = await connectTarget(target);
    const shown = await evaluateIn(devtools)(
      "window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label ?? null",
    ).catch(() => null);
    devtools.close();
    if (shown === label) return true;
  }
  return false;
}

/** A DevTools protocol client for one page target. */
async function connectTarget(page) {
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
  // A page that goes away (a window closed by the call itself) never
  // answers: its calls fail instead of waiting forever, which hung the
  // run now and then (#79). A call unanswered for a minute fails too.
  let closed = false;
  socket.addEventListener("close", () => {
    closed = true;
    for (const [id, waiter] of pending) {
      pending.delete(id);
      waiter.reject(new Error(`the page went away during ${waiter.method}`));
    }
  });
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      if (closed) {
        reject(new Error(`the page is gone (${method})`));
        return;
      }
      const id = ++nextId;
      const timer = setTimeout(() => {
        if (pending.delete(id)) {
          reject(new Error(`no answer to ${method} within 60 s`));
        }
      }, 60_000);
      const settle = (how) => (value) => {
        clearTimeout(timer);
        how(value);
      };
      pending.set(id, {
        method,
        resolve: settle(resolve),
        reject: settle(reject),
      });
      socket.send(JSON.stringify({ id, method, params }));
    });
  return { send, close: () => socket.close() };
}

/** The answer to the window command `command` about the page's own
 * window (`evaluate` does not wait for promises: the answer is kept on the
 * page and read back). */
async function windowCommand(evaluate, command) {
  const key = `__smokeWindow_${command}`;
  await evaluate(`(() => {
    window.${key} = null;
    const internals = window.__TAURI_INTERNALS__;
    internals
      .invoke("plugin:window|${command}", { label: internals.metadata.currentWindow.label })
      .then((value) => (window.${key} = { value }));
  })()`);
  const answer = await until(`the window's answer to ${command}`, () =>
    evaluate(`window.${key}`),
  );
  return answer.value;
}

/**
 * The window's buttons (#80), in the order shown: their marker, accessible
 * name, tooltip, and whether they sit in a bar marked as the window's top
 * bar, flush with the window's top right corner.
 */
function windowButtons(evaluate) {
  return evaluate(`[...document.querySelectorAll("[data-window-control]")]
    .filter((button) => button.checkVisibility())
    .map((button) => {
      const box = button.getBoundingClientRect();
      return {
        control: button.dataset.windowControl,
        name: button.getAttribute("aria-label"),
        title: button.title,
        inBar: !!button.closest("[data-window-bar]"),
        top: Math.round(box.top),
        right: Math.round(window.innerWidth - box.right),
      };
    })`);
}

/** Checks the window has no system frame and its own three buttons, at
 * its top right corner, named and with tooltips (#80). */
async function checkOwnFrame(evaluate, where, maximizeName = "Maximize") {
  const json = (value) => JSON.stringify(value);
  if ((await windowCommand(evaluate, "is_decorated")) !== false)
    fail(`${where} has the system's title bar`);
  const buttons = await windowButtons(evaluate);
  const expected = [
    ["minimize", "Minimize"],
    ["maximize", maximizeName],
    ["close", "Close"],
  ];
  const ok =
    buttons.length === 3 &&
    expected.every(
      ([control, name], at) =>
        buttons[at].control === control &&
        buttons[at].name === name &&
        buttons[at].title === name &&
        buttons[at].inBar &&
        buttons[at].top === 0,
    ) &&
    buttons[2].right === 0;
  if (!ok) fail(`${where}'s window buttons are ${json(buttons)}`);
}

/**
 * The window `evaluate` reaches at its narrowest (#120: below about 560 px
 * with the side view open, the window's buttons left the window): its
 * least width `least`, the window's minimum (DevTools sets any width, the
 * minimum not kept: asked for 200 px, the page was 200 px wide; a user
 * cannot make it narrower than `least`). Its three buttons are then
 * whole inside the window at its top right, each the top element at its
 * centre and at its corners, after everything else of their bar (nothing
 * of the bar runs under them); the bar keeps an empty part that drags the
 * window; Maximize maximizes there and Restore brings the narrow window
 * back. Then the window's width as it was. Minimize is checked reachable,
 * not clicked (a minimized window is hard to bring back from here, #80).
 * Resolves to what was measured, for the step's words.
 */
async function narrowFrame(evaluate, where, least) {
  const json = (value) => JSON.stringify(value);
  const devtools = evaluate.devtools;
  const { windowId, bounds } = await devtools.send(
    "Browser.getWindowForTarget",
  );
  await devtools.send("Browser.setWindowBounds", {
    windowId,
    bounds: { width: least, height: bounds.height },
  });
  const width = await until(`${where} at its narrowest`, async () => {
    const now = await evaluate("window.innerWidth");
    return now >= least - 1 && now <= least + 1 && now;
  });
  // The bar folds its tools once it has measured itself.
  await sleep(300);
  const measure = () =>
    evaluate(`(() => {
      const controls = document.querySelector("[data-window-controls]");
      const bar = controls?.closest("[data-window-bar]");
      if (!bar) return { missing: true };
      const box = controls.getBoundingClientRect();
      const buttons = [...controls.querySelectorAll("[data-window-control]")].map((button) => {
        const own = button.getBoundingClientRect();
        // The centre and points 3 px in from each corner.
        const points = [
          [own.left + own.width / 2, own.top + own.height / 2],
          [own.left + 3, own.top + 3],
          [own.right - 3, own.top + 3],
          [own.left + 3, own.bottom - 3],
          [own.right - 3, own.bottom - 3],
        ];
        const covered = points
          .map(([x, y]) => document.elementFromPoint(x, y))
          .filter((top) => !top || !(top === button || button.contains(top)))
          .map((top) => top ? top.cloneNode(false).outerHTML.slice(0, 120) : "nothing");
        return {
          control: button.dataset.windowControl,
          left: Math.round(own.left),
          right: Math.round(own.right),
          top: Math.round(own.top),
          width: Math.round(own.width),
          covered,
        };
      });
      // Everything else of the bar ends before the buttons begin.
      const under = [...bar.children]
        .filter((child) => child !== controls && child.checkVisibility())
        .filter((child) => child.getBoundingClientRect().width > 0)
        .filter((child) => child.getBoundingClientRect().right > box.left + 0.5)
        .map((child) => child.cloneNode(false).outerHTML.slice(0, 120));
      // An empty part of the bar that drags the window: its spacer (a
      // pane's bar) or its title (a run or config window).
      const empty = bar.querySelector(".spacer, [data-window-title]");
      const room = empty?.getBoundingClientRect();
      const at = room && document.elementFromPoint(room.left + room.width / 2, room.top + room.height / 2);
      const drags = !!at?.closest("[data-tauri-drag-region]") &&
        !at.closest("button, input, select, textarea, a, [role=tab]");
      return {
        width: window.innerWidth,
        buttons,
        under,
        drag: { width: Math.round(room?.width ?? 0), drags },
        barRight: Math.round(bar.getBoundingClientRect().right),
      };
    })()`);
  const found = await measure();
  const names = found.buttons?.map((button) => button.control) ?? [];
  const inside = found.buttons?.every(
    (button) =>
      button.left >= 0 &&
      button.right <= found.width &&
      button.top === 0 &&
      button.width >= 40 &&
      button.covered.length === 0,
  );
  if (
    found.missing ||
    json(names) !== json(["minimize", "maximize", "close"]) ||
    !inside ||
    found.buttons[2].right !== found.width ||
    found.under.length > 0 ||
    found.drag.width < 16 ||
    !found.drag.drags
  )
    fail(`${where} at ${width} px: ${json(found)}`);
  await reachable(evaluate, "[data-window-control=minimize]");
  const maximized = () => windowCommand(evaluate, "is_maximized");
  await mouseClick(evaluate, "[data-window-control=maximize]");
  await until(`${where} maximized by its button at ${width} px`, maximized);
  await until(`${where}'s button offering Restore`, () =>
    evaluate(
      `document.querySelector("[data-window-control=maximize]").getAttribute("aria-label") === "Restore"`,
    ),
  );
  await mouseClick(evaluate, "[data-window-control=maximize]");
  // Restored to the size Windows kept for it, which is not the one
  // DevTools set (the first run waited for the narrow width in vain).
  await until(
    `${where} restored by its button`,
    async () => !(await maximized()),
  );
  await devtools.send("Browser.setWindowBounds", {
    windowId,
    bounds: { width: bounds.width, height: bounds.height },
  });
  await until(
    `${where} as wide as before`,
    async () =>
      Math.abs((await evaluate("window.innerWidth")) - bounds.width) <= 1,
  );
  return width;
}

/**
 * The app's own frame (#80): no system title bar; the top bar's buttons
 * minimize (not clicked: a minimized window is hard to bring back from
 * here), maximize and restore; a double click on the bar's empty part
 * maximizes and restores; the bar's empty part is a drag region, its tabs
 * and buttons are not.
 */
async function windowFrame({ evaluate, mouse, step }) {
  const maximized = () => windowCommand(evaluate, "is_maximized");
  const label = () =>
    evaluate(
      `document.querySelector("[data-window-control=maximize]").getAttribute("aria-label")`,
    );
  await checkOwnFrame(evaluate, "the main window");
  const regions = await evaluate(`(() => {
    const bar = document.querySelector("[data-tab-bar]");
    return {
      bar: bar?.dataset.tauriDragRegion ?? null,
      inside: [...bar.querySelectorAll("[data-tauri-drag-region]")].length,
      side: document.querySelector("[data-side-view] [data-window-bar]")?.dataset.tauriDragRegion ?? null,
    };
  })()`);
  if (regions.bar !== "deep" || regions.inside !== 0 || regions.side !== "deep")
    fail(`the top bar's drag regions are ${JSON.stringify(regions)}`);
  step(
    "no system title bar: the app's top bar (tabs and side view heading) drags the window and holds Minimize, Maximize and Close, named, with tooltips",
  );

  if (await maximized()) fail("the window starts maximized");
  await mouseClick(evaluate, `[data-window-control=maximize]`);
  await until("the window maximized by its button", maximized);
  await until(
    "the button offers Restore",
    async () => (await label()) === "Restore",
  );
  await checkOwnFrame(evaluate, "the maximized window", "Restore");
  await mouseClick(evaluate, `[data-window-control=maximize]`);
  await until(
    "the window restored by its button",
    async () => !(await maximized()),
  );
  await until(
    "the button offers Maximize",
    async () => (await label()) === "Maximize",
  );
  step("Maximize maximized the window and became Restore, which restored it");

  // A double click on the bar's empty part (right of the new-tab button).
  const empty = await evaluate(`(() => {
    const plus = document.querySelector("[data-tab-bar] [data-new-tab]").getBoundingClientRect();
    return { x: Math.round(plus.right + 24), y: Math.round(plus.top + plus.height / 2) };
  })()`);
  const hit = await evaluate(
    `document.elementFromPoint(${empty.x}, ${empty.y})?.closest("[data-tauri-drag-region]")?.matches("[data-tab-bar]") ?? false`,
  );
  if (!hit) fail("the bar's empty part is not where the smoke looked");
  const doubleClickAt = async () => {
    for (const clickCount of [1, 2]) {
      for (const type of ["mousePressed", "mouseReleased"]) {
        await mouse(type, empty, { button: "left", clickCount });
      }
    }
  };
  await doubleClickAt();
  await until("the window maximized by a double click on the bar", maximized);
  await doubleClickAt();
  await until(
    "the window restored by a double click on the bar",
    async () => !(await maximized()),
  );
  step(
    "a double click on the top bar's empty part maximized and restored the window",
  );
}

/**
 * The first window, with no workspace open: the welcome view lists the
 * known workspaces (the gone one marked, and removed from the list), offers
 * open, create and quick connect, and refuses to create over a folder with
 * files; the side views say they need a workspace. It ends by opening the
 * smoke workspace from its row.
 */
async function welcome({ evaluate, text, screenshot, step }) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const rows = () =>
    evaluate(`[...document.querySelectorAll("[data-welcome-workspace]")].map(
      (row) => row.dataset.welcomeWorkspace + (row.dataset.unavailable === undefined ? "" : " (gone)"),
    )`);
  await until("the welcome view", () =>
    exists(".slot.connect [data-welcome] [data-welcome-workspaces]"),
  );
  await until("the known workspaces", async () => (await rows()).length === 2);
  if (json(await rows()) !== json([workspace, `${goneWorkspace} (gone)`])) {
    fail(`the welcome view lists ${json(await rows())}`);
  }
  for (const selector of [
    "[data-welcome-form=open] [name=welcome-folder]",
    "[data-welcome-form=create] [name=welcome-folder]",
    "[data-welcome] [data-quick-mode=ssh]",
    "[data-welcome] [data-quick-mode=serial]",
    "[data-welcome] input[name=host]",
  ]) {
    if (!(await exists(selector))) fail(`the welcome view lacks ${selector}`);
  }
  // Side views that need a workspace say so, the Hosts view naming the
  // control above it (#116).
  if (
    !(await text("[data-side-view=hosts] [data-needs-workspace]")).includes(
      "“No workspace” above",
    )
  ) {
    fail("the Hosts view does not say it needs a workspace");
  }
  await mouseClick(evaluate, `[data-rail=snippets]`);
  await until("the Snippets view saying it needs a workspace", () =>
    exists("[data-side-view=snippets] [data-needs-workspace]"),
  );
  await mouseClick(evaluate, `[data-rail=hosts]`);
  await until("the Hosts view back", () => exists("[data-side-view=hosts]"));
  await screenshot("welcome");

  // Creating over a folder with files in it is refused, in the view.
  await evaluate(`(() => {
    const form = document.querySelector("[data-welcome-form=create]");
    const input = form.querySelector("[name=welcome-folder]");
    input.value = ${json(workspace)};
    input.dispatchEvent(new Event("input", { bubbles: true }));
    form.requestSubmit();
  })()`);
  // It says what to do instead, in the view's own words (#116).
  await until("the refusal", async () =>
    (await text("[data-welcome-error]")).includes(
      "not empty; open it as a workspace instead",
    ),
  );
  // A folder that is not there is said plainly, with a way on, not in
  // the system's words (#116).
  await evaluate(`(() => {
    const form = document.querySelector("[data-welcome-form=open]");
    const input = form.querySelector("[name=welcome-folder]");
    input.value = ${json(join(userData, "no-such-folder"))};
    input.dispatchEvent(new Event("input", { bubbles: true }));
    form.requestSubmit();
  })()`);
  await until("the missing folder said", async () =>
    (await text("[data-welcome-error]")).includes(
      "there is no such folder; check the path",
    ),
  );
  if ((await text("[data-welcome-error]")).includes("os error")) {
    fail(`the welcome view says: ${await text("[data-welcome-error]")}`);
  }
  // A folder of other things, chosen by mistake, is asked about first,
  // with Cancel focused; nothing is written before the answer nor after
  // Cancel (#131).
  const documents = otherThingsFolder();
  const documentsBefore = entriesBelow(documents);
  await evaluate(`(() => {
    const form = document.querySelector("[data-welcome-form=open]");
    const input = form.querySelector("[name=welcome-folder]");
    input.value = ${json(documents)};
    input.dispatchEvent(new Event("input", { bubbles: true }));
    form.requestSubmit();
  })()`);
  await until("the question about a folder of other things", () =>
    exists("[data-confirm-open-folder] [data-confirm-folder=cancel]"),
  );
  await checkOpenFolderQuestion(evaluate, text, documents);
  if (json(entriesBelow(documents)) !== json(documentsBefore))
    fail("the folder was written into before the answer");
  await screenshot("welcome-other-folder");
  await mouseClick(evaluate, "[data-confirm-folder=cancel]");
  await until(
    "the question gone",
    async () => !(await exists("[data-confirm-open-folder]")),
  );
  if (!(await exists("[data-welcome]"))) fail("Cancel left the welcome view");
  if (await exists("[data-welcome-error]"))
    fail(`Cancel says: ${await text("[data-welcome-error]")}`);
  // The shell's own answers come after the question; give a late write a
  // moment to show.
  await sleep(500);
  if (json(entriesBelow(documents)) !== json(documentsBefore))
    fail("the folder was written into after Cancel");
  step(
    "a folder of other things is asked about first (Cancel focused) and nothing is written before the answer nor after Cancel",
  );
  await evaluate(`(() => {
    const input = document.querySelector("[data-welcome-form=open] [name=welcome-folder]");
    input.value = "";
    input.dispatchEvent(new Event("input", { bubbles: true }));
  })()`);

  // The gone folder is taken off the list.
  await mouseClickElement(
    evaluate,
    `document.querySelector(${json(`[data-welcome-workspace=${json(goneWorkspace)}]`)}).closest("li").querySelector("[data-welcome-forget]")`,
    "the gone workspace's Forget",
  );
  await until(
    "the gone workspace off the list",
    async () => json(await rows()) === json([workspace]),
  );
  step(
    "the welcome view lists the known workspaces (a gone one marked and removed), offers open, create and quick connect; side views ask for a workspace",
  );

  await mouseClick(evaluate, `[data-welcome-workspace=${json(workspace)}]`);
  await until(
    "the welcome view gone",
    async () => !(await exists("[data-welcome]")),
  );
}

/**
 * A folder of other things and nothing of a workspace, as a Documents
 * folder chosen by mistake (#131): made once, the same for every part.
 */
function otherThingsFolder() {
  const folder = join(userData, "documents");
  if (!existsSync(folder)) {
    mkdirSync(join(folder, "Photos"), { recursive: true });
    writeFileSync(join(folder, "letter.txt"), "Dear ...\n");
    writeFileSync(
      join(folder, "docker-compose.yml"),
      "services:\n  web:\n    image: nginx\n",
    );
    writeFileSync(join(folder, "Photos", "beach.jpg"), "jpeg");
  }
  return folder;
}

/** Every entry below `folder` (hidden ones too), by path, with its text. */
function entriesBelow(folder, found = {}) {
  for (const entry of readdirSync(folder, { withFileTypes: true })) {
    const path = join(folder, entry.name);
    if (entry.isDirectory()) {
      found[`${path}/`] = "";
      entriesBelow(path, found);
    } else found[path] = readFileSync(path, "utf8");
  }
  return found;
}

/**
 * The question before a folder of other things becomes a workspace (#131):
 * what was found, what opening does, the folder's path, Cancel focused.
 */
async function checkOpenFolderQuestion(evaluate, text, folder) {
  const said = await text("[data-confirm-open-folder]");
  for (const words of [
    "Make this folder a workspace?",
    "No host files here",
    "_workspace.yaml",
    folder,
  ]) {
    if (!said.includes(words))
      fail(`the question does not say ${words}: ${said}`);
  }
  const focused = await evaluate(
    "document.activeElement?.dataset.confirmFolder ?? null",
  );
  if (focused !== "cancel") fail(`the question focuses ${focused}, not Cancel`);
}

/** Every file of the folder below `root`, by path, with its text. */
function filesBelow(root, folder = root, found = {}) {
  for (const entry of readdirSync(folder, { withFileTypes: true })) {
    const path = join(folder, entry.name);
    if (entry.isDirectory()) filesBelow(root, path, found);
    else
      found[path.slice(root.length + 1).replaceAll("\\", "/")] = readFileSync(
        path,
        "utf8",
      );
  }
  return found;
}

async function main() {
  createWorkspace();
  // What the workspace holds before the app opens it: opening must not
  // change a file (#61: an upgrade leaves workspace files alone).
  const workspaceBefore = filesBelow(workspace);
  headlessInventory();
  rmSync(PORTABLE_MARKER, { force: true });
  writeFileSync(INSTALLED_MARKER, "");
  await startReleaseServer();
  await startServer();
  await listenOnThePagePorts();
  // From here on, every connection of the app's processes is watched: only
  // the test's own servers on this computer may be reached (#115).
  connections = await watchConnections({
    ports: [
      SSH_PORT,
      OLD_DEVICE_PORT,
      TELNET_PORT,
      DEVICE_PORT,
      SCP_PORT,
      BANNER_PORT,
      DEVTOOLS_PORT,
      CLOSED_PORT,
      Number(new URL(releaseServer.url).port),
    ],
  });
  // Where the test could not listen on the page port, WebView2's
  // preconnect goes to whoever does, or to nobody: let through unjudged.
  if (pagePorts.clear.listening.length < 2) connections.allow(CLEAR_PORT);
  if (pagePorts.tls.listening.length < 2) connections.allow(TLS_PORT);
  const app = startApp();
  const devtools = await connectDevTools();

  const evaluate = evaluateIn(devtools);
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
  const click = (label) => clickButton(evaluate, label);
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
  const changeField = async (selector, value) => {
    await showDetails(evaluate);
    // The panel's fields are drawn a moment after it shows (#139: slower in
    // Windows Sandbox).
    await until(`the side panel's ${selector}`, () =>
      evaluate(
        `!!document.querySelector(".side " + ${JSON.stringify(selector)})`,
      ),
    );
    await evaluate(`(() => {
      const input = document.querySelector(".side " + ${JSON.stringify(selector)});
      input.value = ${JSON.stringify(value)};
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
  };
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
  const doubleClick = (selector) =>
    mouseClick(evaluate, selector, { double: true });
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
  // With the mouse: a button something is drawn over fails here.
  const decideHostKey = (decision) =>
    mouseClick(evaluate, `[data-decision=${decision}]`);
  // A step passes only if the app connected nowhere else meanwhile.
  const step = (message) => {
    checkConnections();
    console.log(`ok - ${message}`);
    connections.during(`the step after "${message}"`);
    pagePorts.at = `after "${message.slice(0, 70)}"`;
  };
  // Set SMOKE_SCREENSHOTS to a folder to keep pictures of the window.
  // Every screen photographed is also checked for contrast, in both
  // themes (#79), whether or not pictures are kept.
  const screenshot = async (name) => {
    await checkContrast(evaluate, name);
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

  // SMOKE_ONLY=severalInAWindow (#141): only the part with several
  // sessions in a session window, after the least it needs (the workspace
  // open, three quick connections), to repeat it quickly. Unset, the whole
  // test runs as always.
  if (ONLY === "severalInAWindow") {
    await until("the welcome view", () =>
      evaluate(
        `!!document.querySelector(${JSON.stringify(`[data-welcome-workspace=${JSON.stringify(workspace)}]`)})`,
      ),
    );
    await mouseClick(
      evaluate,
      `[data-welcome-workspace=${JSON.stringify(workspace)}]`,
    );
    for (let count = 1; count <= 3; count++) {
      if (count > 1) {
        await mouseClick(evaluate, "[data-tab-bar] [data-new-tab]");
        await until(`the connect view for session ${count}`, () =>
          evaluate(
            `!!document.querySelector(".slot.connect input[name=host]")`,
          ),
        );
      }
      await fillForm(
        {
          host: "127.0.0.1",
          port: String(SSH_PORT),
          username: USERNAME,
          password: PASSWORD,
        },
        ".slot.connect form:has(input[name=host])",
      );
      if (count === 1) {
        await hostKeyCheck("first-use");
        await decideHostKey("trustAndRemember");
      }
      await until(`session ${count} connected`, () =>
        evaluate(
          `document.querySelectorAll('[role=tab][data-tab-state=connected]').length === ${count}`,
        ),
      );
    }
    step("three quick connections open (SMOKE_ONLY=severalInAWindow)");
    // The clipboard, as the whole run grants it (editInventory).
    await devtools.send("Browser.grantPermissions", {
      permissions: ["clipboardReadWrite", "clipboardSanitizedWrite"],
    });
    await sessionWindows({ evaluate, devtools, screenshot, step });
    return;
  }

  // The interface typeface and icons ship inside the app: the font is
  // loaded, and nothing was requested from anywhere but the app itself.
  const fontLoaded = () =>
    evaluate(
      `[...document.fonts].some((f) => f.family.includes("Inter") && f.status === "loaded")`,
    );
  await until("the bundled interface typeface", fontLoaded);
  const assets = await evaluate(`(() => {
    return {
      font: [...document.fonts].some((f) => f.family.includes("Inter") && f.status === "loaded"),
      icons: document.querySelectorAll("svg.lucide-icon").length,
      outside: performance.getEntriesByType("resource")
        .map((r) => r.name)
        .filter((n) => new URL(n).origin !== location.origin && !/^(ipc|asset):/.test(n) && !/\\/\\/ipc\\.localhost\\//.test(n)),
    };
  })()`);
  if (!assets.font || assets.icons === 0 || assets.outside.length > 0) {
    fail(
      `the bundled font and icons are not what is shown: ${JSON.stringify(assets)}`,
    );
  }
  step("the interface typeface and icons are bundled: no request left the app");

  // The app draws its own frame (#80).
  await windowFrame({ evaluate, mouse, step });

  // With no workspace to reopen, the first window is the welcome view: the
  // known workspaces (a gone folder marked), open, create and quick
  // connect; the side views say they need a workspace.
  await welcome({ evaluate, text, screenshot, step });

  // The workspace opened from the welcome view: its tree is shown, and the
  // malformed file is reported by name without stopping the rest from
  // loading.
  await until("the opened workspace's tree", () =>
    evaluate(
      `[...document.querySelectorAll("nav button")].some((b) => b.textContent.includes("Lab"))`,
    ),
  );
  // One line at the foot of the Hosts view, which opens the list.
  await until("the malformed file reported", async () =>
    (await text("[data-problems]")).includes("could not be loaded"),
  );
  {
    // Only the identity file is added (as by every release since #25).
    const after = filesBelow(workspace);
    const changed = Object.keys({ ...workspaceBefore, ...after }).filter(
      (path) =>
        path !== "_workspace.yaml" && workspaceBefore[path] !== after[path],
    );
    if (changed.length > 0) {
      fail(`opening the workspace changed ${JSON.stringify(changed)}`);
    }
    if (
      workspaceBefore["_workspace.yaml"] !== undefined &&
      workspaceBefore["_workspace.yaml"] !== after["_workspace.yaml"]
    ) {
      fail("opening the workspace rewrote its identity file");
    }
  }
  step("opening the workspace changed none of its files");
  await until("the malformed file reported, still", async () =>
    (await text("[data-problems]")).includes("could not be loaded"),
  );
  await mouseClick(evaluate, `[data-problems]`);
  await until("the malformed file listed", async () =>
    (await text("[data-problems-list]")).includes("broken.yaml"),
  );
  await mouseClick(evaluate, `[data-problems]`);
  // Folded away, the Hosts icon still says so.
  if (
    await evaluate(
      `!!document.querySelector("[data-rail=hosts][data-problems]")`,
    )
  ) {
    fail("the Hosts icon is marked while the Hosts view shows the problems");
  }
  await mouseClick(evaluate, `[data-rail=hosts]`);
  await until("the Hosts icon marked while folded", () =>
    evaluate(`!!document.querySelector("[data-rail=hosts][data-problems]")`),
  );
  await screenshot("problems-folded");
  await mouseClick(evaluate, `[data-rail=hosts]`);
  await until("the Hosts view back", () =>
    evaluate(`!!document.querySelector("[data-side-view=hosts]")`),
  );
  step(
    "the workspace opened, with the malformed file reported at the foot (and on the folded Hosts icon)",
  );

  // With a workspace open, the empty pane offers the palette as well as
  // quick connect (SSH and serial).
  await until("the connect view", () =>
    evaluate(
      `!!document.querySelector(".slot.connect [data-connect-view] [data-empty-pane=palette]")`,
    ),
  );
  if (
    !(await evaluate(
      `!!document.querySelector(".slot.connect [data-quick-mode=ssh]") && !!document.querySelector(".slot.connect [data-quick-mode=serial]")`,
    ))
  ) {
    fail("the empty pane does not offer SSH and serial quick connect");
  }
  if (await evaluate(`!!document.querySelector("[data-welcome]")`)) {
    fail("the welcome view stayed with a workspace open");
  }
  // It says how a host of the tree is connected (#116).
  if (
    !(await text(".slot.connect [data-connect-hint]")).startsWith(
      "Double-click a host in the Hosts view to connect",
    )
  ) {
    fail("the connect view does not say a double-click connects");
  }
  await mouseClick(evaluate, `.slot.connect [data-empty-pane=palette]`);
  await until("the palette from the empty pane", () =>
    evaluate(`!!document.querySelector("input[name=palette-query]")`),
  );
  await until("the palette's field focused", () =>
    evaluate(`document.activeElement?.name === "palette-query"`),
  );
  await devtools.send("Input.dispatchKeyEvent", {
    type: "keyDown",
    key: "Escape",
    code: "Escape",
    windowsVirtualKeyCode: 27,
  });
  await until(
    "the palette closed",
    async () =>
      !(await evaluate(
        `!!document.querySelector("input[name=palette-query]")`,
      )),
  );
  await screenshot("empty-pane");
  step("the empty pane offers the palette and quick connect (SSH, serial)");

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

  await inputOrigins({ type, evaluate, mouse, centreOf, devtools, step });

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

  // The session's header is one quiet line; its actions are in a menu
  // there and on the tab's context menu (#67).
  await checkHeaderLine(evaluate, shownSession, [
    "[data-session-protocol=ssh]",
    "[data-session-menu]",
  ]);
  if (
    (await evaluate(
      `document.querySelectorAll(${JSON.stringify(`${shownSession} .session header button`)}).length`,
    )) !== 1
  ) {
    fail("the session header has buttons besides its menu");
  }
  await mouseClick(evaluate, `${shownSession} [data-session-menu]`);
  await until("the header's menu", () => menuEntries(evaluate));
  const headerEntries = await menuEntries(evaluate);
  if (
    JSON.stringify(headerEntries) !==
    JSON.stringify(["duplicate", "open-in-window", "disconnect", "multi-send"])
  ) {
    fail(`the header's menu offers ${JSON.stringify(headerEntries)}`);
  }
  for (const type of ["keyDown", "keyUp"]) {
    await devtools.send("Input.dispatchKeyEvent", {
      type,
      key: "Escape",
      code: "Escape",
      windowsVirtualKeyCode: 27,
    });
  }
  await until(
    "the header's menu closed",
    async () => (await menuEntries(evaluate)).length === 0,
  );
  // The status line: the vault and the active terminal's size.
  const sizesBefore = [...(await screen()).matchAll(/size\s*\d+x\d+/g)].length;
  await type("size\r");
  const reported = await until("the terminal's size", async () => {
    const all = [...(await screen()).matchAll(/size\s*(\d+)x(\d+)/g)];
    return all.length > sizesBefore ? all.at(-1) : null;
  });
  const statusLine = await evaluate(`(() => {
    const size = document.querySelector("[data-status-line] [data-status-size]");
    return {
      vault: document.querySelector("[data-status-line] [data-status-vault]")?.dataset.statusVault,
      cols: size?.dataset.cols,
      rows: size?.dataset.rows,
    };
  })()`);
  if (
    statusLine.vault !== "off" ||
    statusLine.cols !== reported[1] ||
    statusLine.rows !== reported[2]
  ) {
    fail(
      `the status line shows ${JSON.stringify(statusLine)}, the terminal is ${reported[1]}x${reported[2]}`,
    );
  }
  step(
    `the session header is one line with its actions in a menu; the status line shows the vault (off) and ${reported[1]}x${reported[2]}`,
  );

  // Duplicate opens a second session to the same host in a tab beside the
  // first.
  const switchTo = (index) =>
    mouseClickElement(
      evaluate,
      `document.querySelectorAll("[role=tab]")[${index}]`,
      `tab ${index}`,
    );
  // From the tab's context menu: the header's actions, then Close tab.
  await mouseClick(evaluate, "[role=tab][aria-selected=true]", {
    button: "right",
  });
  await until("the tab's context menu", () => menuEntries(evaluate));
  const tabEntries = await menuEntries(evaluate);
  if (
    JSON.stringify(tabEntries) !==
    JSON.stringify([...headerEntries, "close-tab"])
  ) {
    fail(`the tab's context menu offers ${JSON.stringify(tabEntries)}`);
  }
  await mouseClick(
    evaluate,
    `[data-session-actions] [data-session-action=duplicate]`,
  );
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
  const clickShown = (label) => clickButton(evaluate, label, shownSession);
  await sessionAction(evaluate, "disconnect");
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

  await sessionAction(evaluate, "disconnect");
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
  // It says what to check (#116), and Enter reconnects (a quick connection
  // as it was made; a host's with what it has then: `credentialProfiles`
  // corrects a profile's password and reconnects, #130).
  const failedBar = `${shownSession} [data-session-ended]`;
  await until("the authentication failure's advice", async () =>
    (await text(`${failedBar} [data-session-advice]`)).includes(
      "Check the username and password",
    ),
  );
  const failedWords = await text(failedBar);
  if (
    !failedWords.includes("Press Enter to reconnect.") ||
    failedWords.includes("same settings")
  ) {
    fail(`the failure's bar says: ${failedWords}`);
  }
  await screenshot("authentication-failed");
  step(
    "a wrong password is reported with what to check (the remembered key was not asked about)",
  );

  // A port nobody listens on is reported as refused.
  await click("New connection");
  await until("the quick-connect form", () =>
    evaluate(`!!document.querySelector("input[name=host]")`),
  );
  await fillForm({
    host: "127.0.0.1",
    port: String(CLOSED_PORT),
    username: USERNAME,
    password: PASSWORD,
  });
  await until(
    "the refused connection",
    async () =>
      (await status()) ===
      `Closed: 127.0.0.1 refused the connection on port ${CLOSED_PORT}`,
  );
  await until("the refused connection's advice", async () =>
    (
      await text(`${shownSession} [data-session-ended] [data-session-advice]`)
    ).includes("Nothing accepts connections on port 1: check the Port setting"),
  );
  step(
    `a refused connection is reported, with what to check: "${await status()}"`,
  );

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
  await sessionAction(evaluate, "disconnect");
  await until("the closed status", async () =>
    (await status()).startsWith("Closed"),
  );
  step("a changed host key was blocked, then replaced on request");

  // Double-clicking a host in the tree prompts for credentials and opens an
  // SSH session to its management address, on the port its folder sets.
  await mouseClickElement(
    evaluate,
    `[...document.querySelectorAll("nav button[aria-expanded]")].find((b) => b.textContent.includes("Lab"))`,
    "the Lab folder",
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
  await showDetails(evaluate);
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
    type,
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

  await multiSend({
    evaluate,
    devtools,
    doubleClick,
    fillForm,
    text,
    status,
    screenshot,
    step,
  });

  await sessionWindows({ evaluate, devtools, screenshot, step });

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

  await safetyInSessionWindows({ evaluate, devtools, screenshot, step });

  await closingSessionWindows({ evaluate, screenshot, step });

  await crashedSessionWindow({ evaluate, devtools, step });

  await hungSessionWindow({ evaluate, devtools, step });

  await webLinks({ evaluate, text, screenshot, step });

  await csvImport({
    evaluate,
    fillForm,
    text,
    mouse,
    centreOf,
    screenshot,
    step,
  });

  await nornirImport({
    evaluate,
    fillForm,
    text,
    mouse,
    centreOf,
    screenshot,
    step,
  });

  await jobs({
    evaluate,
    mouse,
    centreOf,
    click,
    fillForm,
    doubleClick,
    status,
    screen,
    text,
    type,
    screenshot,
    step,
  });

  await savedJobs({ evaluate, fillForm, text, screenshot, step });

  await twoPreflights({ evaluate, screenshot, step });

  await hostSelection({ evaluate, devtools, fillForm, screenshot, step });

  await filesTab({
    evaluate,
    devtools,
    fillForm,
    doubleClick,
    screen,
    type,
    screenshot,
    step,
  });

  await fileTransfers({
    evaluate,
    fillForm,
    doubleClick,
    screen,
    type,
    screenshot,
    step,
  });

  await fileJob({ evaluate, fillForm, screenshot, step });

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

  await scriptJobWithoutPython({ evaluate, screenshot, step });

  const pythonInterpreter = await pythonSetup({
    evaluate,
    fillForm,
    screenshot,
    step,
  });

  await pythonBridge({ evaluate, screenshot, step, ready: pythonInterpreter });

  await enrichment({ evaluate, screenshot, step, ready: pythonInterpreter });

  await sidePanels({ evaluate, mouse, centreOf, devtools, screenshot, step });

  const cutByClosing = await quittingWithLiveSessions({
    app,
    evaluate,
    devtools,
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
  const restartedApp = startApp();
  const restarted = await connectDevTools();
  const evaluateAgain = evaluateIn(restarted);
  await until("the light theme after a restart", () =>
    evaluateAgain(`document.documentElement.dataset.theme === "light"`),
  );
  await openPreferences(evaluateAgain);
  const expected = {
    theme: "light",
    family: "Consolas",
    size: "15",
    scrollback: "2000",
    copyOnSelect: true,
    rightClickPaste: true,
    confirmMultiLinePaste: false,
  };
  const kept = await until("the Preferences tab after a restart", async () => {
    const shown = await evaluateAgain(`(() => {
      const dialog = document.querySelector("[data-view=preferences]");
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
    })()`);
    // The dialog shows the defaults until the core's answer arrives.
    return JSON.stringify(shown) === JSON.stringify(expected) ? shown : null;
  });
  if (JSON.stringify(kept) !== JSON.stringify(expected)) {
    fail(`preferences after a restart: ${JSON.stringify(kept)}`);
  }
  step("the preferences were kept across a restart");
  const panels = await until("the side view after a restart", () =>
    evaluateAgain(`(() => {
      const left = document.querySelector("[data-side-view]");
      return left && {
        view: left.dataset.sideView,
        rail: document.querySelector("[data-rail][data-active]")?.dataset.rail,
        left: Math.round(left.getBoundingClientRect().width),
      };
    })()`),
  );
  if (
    panels.left !== SIDE_PANEL_WIDTH ||
    panels.view !== SIDE_VIEW_KEPT ||
    panels.rail !== SIDE_VIEW_KEPT
  ) {
    fail(`the side view after a restart: ${JSON.stringify(panels)}`);
  }
  step("the side view and its width were kept across a restart");
  await showSideView(evaluateAgain, "hosts");

  // The known workspaces survive it too, and the last one opened reopens.
  await until(
    "the smoke workspace reopened",
    async () =>
      (await evaluateAgain(
        `document.querySelector("[data-open-workspaces]")?.textContent.trim()`,
      )) === "smoke-workspace",
  );
  await mouseClick(evaluateAgain, "[data-open-workspaces]");
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
  // Closed by Escape, as a user would before going on.
  for (const type of ["keyDown", "keyUp"]) {
    await restarted.send("Input.dispatchKeyEvent", {
      type,
      key: "Escape",
      code: "Escape",
      windowsVirtualKeyCode: 27,
    });
  }
  await until(
    "the switcher closed",
    async () =>
      !(await evaluateAgain(
        `!!document.querySelector("[data-dialog=workspaces]")`,
      )),
  );

  await jobCutByClosing({ evaluate: evaluateAgain, run: cutByClosing, step });

  await pythonAfterRestart({
    evaluate: evaluateAgain,
    expected: pythonInterpreter,
    step,
  });

  await portableUpdateCheck({
    evaluate: evaluateAgain,
    requestsBeforeRestart,
    step,
  });

  const last = await vault({
    app: restartedApp,
    devtools: restarted,
    evaluate: evaluateAgain,
    step,
  });
  last.close();

  await noConnectionOfItsOwn(step);
}

/** Runs `expression` in the page `devtools` is connected to. */
function evaluateIn(devtools) {
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
  // Its page's client, for the mouse (`mouseClick`).
  evaluate.devtools = devtools;
  return evaluate;
}

/** Ends `app` and starts it again (with `env` added to its environment);
 * resolves to the new one and its page. */
async function restartApp(app, devtools, env = {}) {
  devtools.close();
  app.kill();
  await new Promise((resolve) =>
    app.exitCode === null ? app.once("exit", resolve) : resolve(),
  );
  await sleep(1500);
  const next = startApp(env);
  const page = await connectDevTools();
  return { app: next, devtools: page, evaluate: evaluateIn(page) };
}

const MASTER_PASSWORD = "smoke master passw0rd";

/**
 * The vault: a profile is created, the vault turned on in Preferences
 * (the password twice, the no-recovery warning), the app restarted. The
 * unlock screen comes before the workspace, refuses a wrong password, then
 * unlocks, and the profile (now only in the vault: the smoke's keychain is
 * in memory and gone with the restart) logs in. Resolves to the last page.
 */
async function vault({ app, devtools, evaluate, step }) {
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${JSON.stringify(selector)})`);
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const click = (label) => clickButton(evaluate, label);
  const fillForm = (fields, form) =>
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
  const vaultFile = join(appData, "credentials.vault");
  const hostRow = 'nav [data-host="vault-r1.yaml"]';
  const profilesDialog = CREDENTIALS;

  // A host that names a profile, and the profile.
  writeFileSync(
    join(workspace, "vault-r1.yaml"),
    `name: vault-r1\naddress: 127.0.0.1\nport: ${SSH_PORT}\ncredential_profile: vault-login\n`,
  );
  // The workspace switcher is still open from the restart checks.
  await evaluate(
    `window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))`,
  );
  await until(
    "the switcher closed",
    async () => !(await exists("[data-dialog=workspaces]")),
  );
  await until("the new host in the tree", () => exists(hostRow));
  // The Credentials view says where profiles are kept, marks the one the
  // host names as not set up here, and sets it up in place.
  await openProfiles(evaluate);
  await until("the keychain named in the Credentials view", () =>
    exists(`${profilesDialog} [data-credentials-vault=off]`),
  );
  const missingRow = `${profilesDialog} [data-profile="vault-login"][data-profile-missing]`;
  await until("the missing profile marked", () => exists(missingRow));
  await clickOn(`${missingRow} [data-profile-setup]`);
  await until(
    "the set-up form",
    async () =>
      (await exists(`${profilesDialog} [name=profile-username]`)) &&
      !(await exists(`${profilesDialog} [name=profile-name]`)),
  );
  await fillForm(
    { "profile-username": USERNAME, "profile-password": PASSWORD },
    `${profilesDialog} form`,
  );
  await until("the profile listed as set up", () =>
    exists(
      `${profilesDialog}[data-mode=list] [data-profile="vault-login"]:not([data-profile-missing])`,
    ),
  );
  step("the Credentials view marked a missing profile and set it up");

  // Turning the vault on asks for the password twice, with the warning.
  await clickOn("[data-open-preferences]");
  await until("the keychain in use", () => exists("[data-vault-status=off]"));
  await clickOn("[data-vault=turn-on]");
  const warning = await until("the no-recovery warning", () =>
    evaluate(
      `document.querySelector("[data-vault-warning]")?.textContent ?? ""`,
    ),
  );
  if (!/forget/.test(warning) || !/recover/.test(warning)) {
    fail(`the vault warning: ${warning}`);
  }
  await evaluate(`(() => {
    const form = document.querySelector("[data-vault-form]");
    for (const [name, value] of [["vault-password", "one"], ["vault-repeat", "two"]]) {
      const input = form.querySelector("[name=" + name + "]");
      input.value = value;
      input.dispatchEvent(new Event("input", { bubbles: true }));
    }
  })()`);
  await until("different passwords refused", () =>
    evaluate(
      `!!document.querySelector("[data-vault-differ]") && document.querySelector("[data-vault=confirm]").disabled`,
    ),
  );
  await fillForm(
    { "vault-password": MASTER_PASSWORD, "vault-repeat": MASTER_PASSWORD },
    "[data-vault-form]",
  );
  await until("the vault in use", () => exists("[data-vault-status=unlocked]"));
  if (!existsSync(vaultFile)) fail("turning the vault on wrote no file");
  const everyFile = readdirSync(appData, {
    recursive: true,
    withFileTypes: true,
  })
    .filter((entry) => entry.isFile())
    .map((entry) => readFileSync(join(entry.parentPath, entry.name)))
    .map((bytes) => bytes.toString("latin1"))
    .join("\n");
  for (const secret of [MASTER_PASSWORD, PASSWORD]) {
    if (everyFile.includes(secret)) fail("a secret is in the data folder");
  }
  await closePreferences(evaluate);
  await until("the vault unlocked in the Credentials view", () =>
    exists(`${profilesDialog} [data-credentials-vault=unlocked]`),
  );
  await until("the vault unlocked on the status line", () =>
    exists("[data-status-line] [data-status-vault=unlocked]"),
  );
  await showSideView(evaluate, "hosts");
  step(
    "the vault was turned on: password twice, warned, nothing plain on disk",
  );

  // After a restart, the unlock screen comes first and refuses a wrong
  // password.
  const next = await restartApp(app, devtools);
  const again = next.evaluate;
  const shown = (selector) =>
    again(`!!document.querySelector(${JSON.stringify(selector)})`);
  const unlock = (password) =>
    again(`(() => {
      const form = document.querySelector("[data-unlock-screen] form");
      const input = form.querySelector("[name=vault-unlock-password]");
      input.value = ${JSON.stringify(password)};
      input.dispatchEvent(new Event("input", { bubbles: true }));
      form.requestSubmit();
    })()`);
  await until("the unlock screen", () => shown("[data-unlock-screen]"));
  if (!(await shown("[data-unlock=quit]")))
    fail("the unlock screen has no Quit");
  // The new look: the lock mark and the app name over the form.
  if (
    !(await again(
      `!!document.querySelector("[data-unlock-screen] [data-unlock-mark] svg.lucide-icon") && document.querySelector("[data-unlock-screen] h1")?.textContent.length > 0`,
    ))
  )
    fail("the unlock screen lacks its mark and title");
  // It covers the top bar, so it has the window's buttons and a strip
  // that drags the window of its own (#80).
  if (
    (await again(
      `[...document.querySelectorAll("[data-unlock-screen] [data-window-bar][data-tauri-drag-region] [data-window-control]")].map((b) => b.getAttribute("aria-label")).join()`,
    )) !== "Minimize,Maximize,Close"
  )
    fail("the unlock screen lacks the window's buttons");
  await checkContrast(again, "unlock screen");
  const whileLocked = headlessCredentials("vault-r1.yaml");
  if (
    whileLocked.status !== 1 ||
    !whileLocked.stderr.includes("running but its vault is locked")
  ) {
    fail(
      `netterm-cli with the vault locked: ${whileLocked.status} ${whileLocked.stderr} ${whileLocked.error ?? ""}`,
    );
  }
  if (await shown("nav [data-host]")) {
    fail("the workspace opened before unlocking");
  }
  await unlock("not the master password");
  await until("the wrong password reported", () =>
    shown("[data-unlock-error=wrongPassword]"),
  );
  if (await shown("nav [data-host]")) {
    fail("a wrong password opened the workspace");
  }
  step(
    "after a restart the unlock screen came first and refused a wrong password",
  );

  // A file that cannot be the vault (#127: untested until then) is said to
  // be damaged, whatever the password; the vault is put back after.
  const sealed = readFileSync(vaultFile);
  writeFileSync(vaultFile, "this is not a vault file\n");
  try {
    await unlock(MASTER_PASSWORD);
    await until("the damaged vault reported", () =>
      shown("[data-unlock-error=damaged]"),
    );
    const said = await again(
      `document.querySelector("[data-unlock-error=damaged]")?.textContent ?? ""`,
    );
    if (!said.includes("cannot be opened with any password"))
      fail(`the damaged vault is said as ${JSON.stringify(said)}`);
    if (await shown("nav [data-host]")) {
      fail("a damaged vault opened the workspace");
    }
  } finally {
    writeFileSync(vaultFile, sealed);
  }
  step(
    "a vault file that is not a vault was said to be damaged, nothing opened",
  );

  // Unlocked, the workspace opens and the profile logs in from the vault.
  await unlock(MASTER_PASSWORD);
  await until(
    "the unlock screen gone",
    async () => !(await shown("[data-unlock-screen]")),
  );
  await until("the workspace after unlocking", () => shown(hostRow));
  await mouseClick(again, hostRow, { double: true });
  await until(
    "the Connected status",
    async () =>
      (await again(
        `document.querySelector("${SHOWN_SESSION} [role=status]")?.textContent ?? ""`,
      )) === "Connected",
  );
  await until("the shell prompt", async () =>
    (await screenOf(again, SHOWN_SESSION)).includes("test-ssh$"),
  );
  if (await shown("input[name=password]")) fail("a password was asked for");
  step("unlocked, the workspace opened and the vault's profile logged in");

  const unlocked = headlessCredentials("vault-r1.yaml");
  if (unlocked.status !== 0) {
    fail(
      `netterm-cli with the vault unlocked: ${unlocked.status} ${unlocked.stderr}`,
    );
  }
  const login = JSON.parse(unlocked.stdout);
  if (login.username !== USERNAME || login.auth.password !== PASSWORD) {
    fail(`netterm-cli got ${login.username} / ${login.auth.kind}`);
  }
  step(
    "the headless binary was refused while the vault was locked, and got the login from the unlocked app",
  );

  return vaultUpkeep({ current: next, vaultFile, hostRow, step });
}

const NEW_MASTER_PASSWORD = "smoke n3w master password";

/**
 * Vault upkeep (#39), on the unlocked app `current`: the master password is
 * changed (a wrong current one refused), and after a restart only the new
 * one unlocks; the vault is turned off (the file goes) and the next start
 * has no unlock screen; then, turned on again and restarted, "start over"
 * from the unlock screen (its warning, Keep, then confirmed) deletes the
 * vault and the app opens with the keychain. Resolves to the last page.
 */
async function vaultUpkeep({ current, vaultFile, hostRow, step }) {
  let page = current;
  const ev = (expression) => page.evaluate(expression);
  // The page changes with each restart; the mouse follows it.
  Object.defineProperty(ev, "devtools", { get: () => page.devtools });
  const shown = (selector) =>
    ev(`!!document.querySelector(${JSON.stringify(selector)})`);
  const clickOn = (selector) => mouseClick(ev, selector);
  const textOf = (selector) =>
    ev(
      `document.querySelector(${JSON.stringify(selector)})?.textContent ?? ""`,
    );
  const fillForm = (fields, form) =>
    ev(`(() => {
      const fields = ${JSON.stringify(fields)};
      const form = document.querySelector(${JSON.stringify(form)});
      for (const [name, value] of Object.entries(fields)) {
        const input = form.querySelector("[name=" + name + "]");
        input.value = value;
        input.dispatchEvent(new Event("input", { bubbles: true }));
      }
      form.requestSubmit();
    })()`);
  const closeDialog = () => closePreferences(ev);
  const openSecurity = async (status) => {
    await clickOn("[data-open-preferences]");
    await until(`the vault ${status} in Preferences`, () =>
      shown(`[data-vault-status=${status}]`),
    );
  };
  const restart = async () => {
    page = await restartApp(page.app, page.devtools);
  };
  const unlockWith = (password) =>
    ev(`(() => {
      const form = document.querySelector("[data-unlock-screen] form");
      const input = form.querySelector("[name=vault-unlock-password]");
      input.value = ${JSON.stringify(password)};
      input.dispatchEvent(new Event("input", { bubbles: true }));
      form.requestSubmit();
    })()`);

  // Changing the password needs the current one.
  await openSecurity("unlocked");
  await clickOn("[data-vault=change]");
  await until("the change form", () => shown("[data-vault-change-form]"));
  await fillForm(
    {
      "vault-current": "not the master password",
      "vault-new": NEW_MASTER_PASSWORD,
      "vault-new-repeat": NEW_MASTER_PASSWORD,
    },
    "[data-vault-change-form]",
  );
  await until("a wrong current password refused", () =>
    shown("[data-vault-error]"),
  );
  await fillForm(
    {
      "vault-current": MASTER_PASSWORD,
      "vault-new": NEW_MASTER_PASSWORD,
      "vault-new-repeat": NEW_MASTER_PASSWORD,
    },
    "[data-vault-change-form]",
  );
  await until("the password changed", () => shown("[data-vault-done]"));
  await closeDialog();

  await restart();
  await until("the unlock screen", () => shown("[data-unlock-screen]"));
  await unlockWith(MASTER_PASSWORD);
  await until("the old password refused", () =>
    shown("[data-unlock-error=wrongPassword]"),
  );
  // Quit on the unlock screen ends the app (#127: untested until then);
  // it is started again. Clicked from a timer: the page goes with the app.
  await ev(
    `(setTimeout(() => document.querySelector("[data-unlock=quit]").click(), 200), true)`,
  );
  const quitting = page.app;
  await until(
    "the app ended by Quit on the unlock screen",
    async () => quitting.exitCode !== null || quitting.signalCode !== null,
  );
  try {
    page.devtools.close();
  } catch {
    // Gone with the app.
  }
  await sleep(1500);
  {
    const app = startApp();
    const devtools = await connectDevTools();
    page = { app, devtools, evaluate: evaluateIn(devtools) };
  }
  await until("the unlock screen after Quit", () =>
    shown("[data-unlock-screen]"),
  );
  step("Quit on the unlock screen ended the app");
  await unlockWith(NEW_MASTER_PASSWORD);
  await until("the workspace after unlocking", () => shown(hostRow));
  step("the master password was changed: afterwards only the new one unlocked");

  // Turning the vault off asks for the password, then removes the file.
  await openSecurity("unlocked");
  await clickOn("[data-vault=turn-off]");
  await until("the turn-off form", () => shown("[data-vault-off-form]"));
  await fillForm(
    { "vault-off-password": NEW_MASTER_PASSWORD },
    "[data-vault-off-form]",
  );
  await until("the keychain in use again", () =>
    shown("[data-vault-status=off]"),
  );
  if (existsSync(vaultFile)) fail("turning the vault off left its file");
  await closeDialog();
  await restart();
  await until("the workspace without unlocking", () => shown(hostRow));
  if (await shown("[data-unlock-screen]")) {
    fail("an unlock screen after turning the vault off");
  }
  step("the vault was turned off: no file, and no unlock screen at start");

  // Turned on again, then started over from the unlock screen.
  await openSecurity("off");
  await clickOn("[data-vault=turn-on]");
  await until("the turn-on form", () => shown("[data-vault-form]"));
  await fillForm(
    { "vault-password": MASTER_PASSWORD, "vault-repeat": MASTER_PASSWORD },
    "[data-vault-form]",
  );
  await until("the vault in use", () => shown("[data-vault-status=unlocked]"));
  await closeDialog();
  // A profile kept only in the vault, which starting over deletes.
  await newProfile(ev);
  await fillForm(
    {
      "profile-name": "vault-login",
      "profile-username": USERNAME,
      "profile-password": PASSWORD,
    },
    `${CREDENTIALS} form`,
  );
  await until("the profile listed", () =>
    shown(`${CREDENTIALS}[data-mode=list] [data-profile="vault-login"]`),
  );
  await showSideView(ev, "hosts");
  await restart();
  await until("the unlock screen", () => shown("[data-unlock-screen]"));
  await clickOn("[data-unlock=start-over]");
  const warning = await until("the start-over warning", () =>
    textOf("[data-start-over-warning]"),
  );
  if (!/profiles/.test(warning) || !/secrets/.test(warning)) {
    fail(`the start-over warning: ${warning}`);
  }
  if (!/gone/.test(warning)) fail(`the start-over warning: ${warning}`);
  await checkContrast(ev, "start over");
  await clickOn("[data-start-over=cancel]");
  await until("back at the unlock form", () =>
    shown("[data-unlock-screen] [name=vault-unlock-password]"),
  );
  if (!existsSync(vaultFile)) fail("keeping the vault deleted it");
  await clickOn("[data-unlock=start-over]");
  await until("the start-over confirmation", () =>
    shown("[data-start-over=confirm]"),
  );
  await clickOn("[data-start-over=confirm]");
  await until(
    "the unlock screen gone",
    async () => !(await shown("[data-unlock-screen]")),
  );
  await until("the workspace after starting over", () => shown(hostRow));
  if (existsSync(vaultFile)) fail("starting over left the vault file");
  await openProfiles(ev);
  await until("the vault-login profile shown as not set up", () =>
    shown(`${CREDENTIALS} [data-profile="vault-login"][data-profile-missing]`),
  );
  if (
    await shown(`${CREDENTIALS} [data-profile]:not([data-profile-missing])`)
  ) {
    fail("a profile is left after starting over");
  }
  if (!(await shown(`${CREDENTIALS} [data-credentials-vault=off]`))) {
    fail("the Credentials view does not say the vault is off");
  }
  await showSideView(ev, "hosts");
  await openSecurity("off");
  await closeDialog();
  step(
    "started over from the unlock screen: warned, vault deleted, no profiles left",
  );
  return noKeychain({ current: page, vaultFile, step });
}

/**
 * A system without a keychain (#57, Linux), played on Windows by starting
 * the app with `NETTERM_CREDENTIAL_STORE=none`: Preferences say the vault
 * is the only store; saving a profile explains it and offers the vault,
 * turned on from there, after which the profile typed is saved into it;
 * turning the vault off explains there is no other store and leaves it.
 * Resolves to the last page.
 */
async function noKeychain({ current, vaultFile, step }) {
  const page = await restartApp(current.app, current.devtools, {
    NETTERM_CREDENTIAL_STORE: "none",
  });
  const ev = page.evaluate;
  const shown = (selector) =>
    ev(`!!document.querySelector(${JSON.stringify(selector)})`);
  const clickOn = (selector) => mouseClick(ev, selector);
  const textOf = (selector) =>
    ev(
      `document.querySelector(${JSON.stringify(selector)})?.textContent ?? ""`,
    );
  const fillForm = (fields, form) =>
    ev(`(() => {
      const fields = ${JSON.stringify(fields)};
      const form = document.querySelector(${JSON.stringify(form)});
      for (const [name, value] of Object.entries(fields)) {
        const input = form.querySelector("[name=" + name + "]");
        input.value = value;
        input.dispatchEvent(new Event("input", { bubbles: true }));
      }
      form.requestSubmit();
    })()`);
  const closeDialog = () => closePreferences(ev);
  const profilesDialog = CREDENTIALS;

  await until("the workspace without a keychain", () =>
    shown('nav [data-host="vault-r1.yaml"]'),
  );
  if (existsSync(vaultFile)) fail("a vault file before turning it on");
  await clickOn("[data-open-preferences]");
  await until("Preferences without a keychain", () =>
    shown("[data-vault-status=off][data-vault-keychain=false]"),
  );
  if (
    !/only be kept in the vault/.test(await textOf("[data-vault-no-keychain]"))
  )
    fail("Preferences do not say the vault is the only store");
  await closeDialog();

  await openProfiles(ev);
  await until("the Credentials view's vault note", () =>
    shown(`${profilesDialog} [data-profiles-needs-vault]`),
  );
  await newProfile(ev);
  await fillForm(
    {
      "profile-name": "vault-login",
      "profile-username": USERNAME,
      "profile-password": PASSWORD,
    },
    `${profilesDialog} form`,
  );
  const why = await until("saving explains the vault", () =>
    textOf(`${profilesDialog} [data-needs-vault=explain]`),
  );
  if (!/no keychain/.test(why) || !/Turn the vault on/.test(why))
    fail(`the explanation on saving: ${why}`);
  await clickOn(`${profilesDialog} [data-needs-vault-action=turn-on]`);
  await until("the vault form inside the Credentials view", () =>
    shown(`${profilesDialog} [data-needs-vault=turn-on] [data-vault-form]`),
  );
  if (/keychain/.test(await textOf(`${profilesDialog} [data-vault-warning]`)))
    fail("the vault warning speaks of a keychain where there is none");
  await fillForm(
    { "vault-password": MASTER_PASSWORD, "vault-repeat": MASTER_PASSWORD },
    `${profilesDialog} [data-vault-form]`,
  );
  await until("the profile saved into the vault", () =>
    shown(
      `${profilesDialog} [data-profile="vault-login"]:not([data-profile-missing])`,
    ),
  );
  await until("the vault unlocked in the Credentials view", () =>
    shown(`${profilesDialog} [data-credentials-vault=unlocked]`),
  );
  if (!existsSync(vaultFile)) fail("no vault file after turning it on");
  if (await shown(`${profilesDialog} [data-profiles-needs-vault]`))
    fail("the vault note stayed after turning the vault on");
  await showSideView(ev, "hosts");
  step(
    "without a keychain, saving a profile offered the vault, and the profile went into it",
  );

  await clickOn("[data-open-preferences]");
  await until("the vault on without a keychain", () =>
    shown("[data-vault-status=unlocked][data-vault-keychain=false]"),
  );
  await clickOn("[data-vault=turn-off]");
  const off = await until("turning off explained", () =>
    textOf("[data-vault-off-no-keychain]"),
  );
  if (!/no other store/.test(off))
    fail(`turning off without a keychain: ${off}`);
  if (await shown("[data-vault-off-form]"))
    fail("a turn-off form where there is no keychain");
  await clickOn("[data-vault-off-no-keychain] [data-vault=cancel]");
  await until(
    "the explanation closed",
    async () => !(await shown("[data-vault-off-no-keychain]")),
  );
  if (!existsSync(vaultFile)) fail("the vault file went");
  await closeDialog();
  step(
    "without a keychain, turning the vault off said there is no other store",
  );
  await closingTheMainWindow({ app: page.app, evaluate: ev, step });
  return page.devtools;
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

  await mouseClick(evaluate, `[data-update-install]`);
  const refused = await until("the install refused", () =>
    text("[data-update-error]"),
  );
  if (!/cannot install updates/.test(refused)) {
    fail(`installing in a development build: ${refused}`);
  }
  step("installing waits for a click, and a non-release build refuses it");

  await mouseClick(evaluate, `[data-update-later]`);
  await until("the notice closed", () =>
    evaluate(`!document.querySelector("[data-update]")`),
  );

  await openPreferences(evaluate);
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
  await mouseClick(evaluate, `[name=preference-check-for-updates]`);
  await until("the update check saved as off", () =>
    readFileSync(join(appData, "preferences.yaml"), "utf8").includes(
      "check_for_updates: false",
    ),
  );
  await closePreferences(evaluate);
  step("the update check was turned off in the preferences");
}

/**
 * After the restart, as a portable copy with the check off: no request at
 * start; "Check now" asks once and only announces the release, with its
 * download page (recorded, never opened in a browser) and no install.
 */
/** Where the smoke's script jobs keep their scripts (removed after). */
const BRIDGE_SCRIPT = "Jobs/bridge_check.py";
const SLOW_SCRIPT = "Jobs/bridge_slow.py";

/**
 * The script a script job (#52) runs against the example server: it lists
 * the inventory the job gives it (only the Jobs folder's hosts), reads the
 * SSH banner of each SSH host itself and asks the app for its login (the
 * username only is printed), then exits 0.
 */
const BRIDGE_SCRIPT_TEXT = `import socket
import sys

import netterm

export = netterm.inventory()
print("hosts", len(export["hosts"]))
for host in export["hosts"]:
    print("target", host["id"])
    if host["settings"]["protocol"] != "ssh":
        continue
    with socket.create_connection((host["address"], host["settings"]["port"]), timeout=10) as s:
        banner = s.recv(256).decode("ascii", "replace").strip()
    print("banner", host["id"], banner)
    print("login", host["id"], netterm.credentials(None, host["id"])["username"])
print("finished on stderr", file=sys.stderr)
`;

/** Opens the run-script dialog on the Jobs folder. */
async function openScriptJob(evaluate) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const folderRow = 'nav button[data-item=folder][data-path="Jobs"]';
  await until("the Jobs folder in the tree", () => exists(folderRow));
  await mouseClick(evaluate, folderRow);
  await showDetails(evaluate);
  await until("the Jobs folder's run-script button", () =>
    exists(".side [data-folder-run-script]"),
  );
  await mouseClick(evaluate, `.side [data-folder-run-script]`);
  await until("the run-script tab", () =>
    exists(`${JOB_TAB}[data-step=script]`),
  );
}

/**
 * Before Python is set up, "Run script…" lists the workspace's scripts but
 * says how to turn Python on instead of running one.
 */
async function scriptJobWithoutPython({ evaluate, screenshot, step }) {
  const json = (value) => JSON.stringify(value);
  const dialog = JOB_TAB;
  writeFileSync(join(workspace, BRIDGE_SCRIPT), BRIDGE_SCRIPT_TEXT);
  await openScriptJob(evaluate);
  const shown = await until("the scripts listed, Python off", () =>
    evaluate(`(() => {
      const d = document.querySelector(${json(dialog)});
      const scripts = [...d.querySelectorAll("[name=job-script] option")].map((o) => o.value);
      const off = d.querySelector("[data-job-python-off]");
      // Not while the interpreter is still being checked.
      return scripts.length > 0 && off && !/Checking/.test(off.textContent) && {
        scripts,
        off: off.textContent,
        preferences: !!d.querySelector("[data-job-python-preferences]"),
      };
    })()`),
  );
  if (!shown.scripts.includes(BRIDGE_SCRIPT))
    fail(`the scripts listed: ${json(shown.scripts)}`);
  if (!/Python features are off/.test(shown.off) || !shown.preferences)
    fail(`run script without Python: ${json(shown)}`);
  await screenshot("script-job-python-off");
  step(
    "Run script… lists the workspace's scripts and, without Python, says how to set it up",
  );
  await mouseClick(evaluate, `${dialog} [data-job=dismiss]`);
  await until(
    "the run-script dialog closed",
    async () => !(await evaluate(`!!document.querySelector("${dialog}")`)),
  );
}

/**
 * Script jobs (#52), with the app's environment set up by pythonSetup: a
 * script runs for the Jobs folder's hosts (pre-flight and the production
 * confirmation as for snippets), sees only them, reaches the example
 * server and gets a login from the app; its output streams into the
 * job tab and the run tab (searchable) and it ends by exit status. A
 * second, slow script is cancelled, keeping its output. Skipped without a
 * Python 3.10+.
 */
async function pythonBridge({ evaluate, screenshot, step, ready }) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const dialog = JOB_TAB;
  const atStep = (name) => exists(`${dialog}[data-step=${name}]`);
  const pickScript = (path) =>
    evaluate(`(() => {
      const select = document.querySelector("${dialog} [name=job-script]");
      select.value = ${json(path)};
      select.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
  const result = () =>
    evaluate(`(() => {
      const row = document.querySelector('${dialog} [data-job-result="script"]');
      return row && {
        state: row.dataset.hostState,
        exit: row.dataset.exitStatus ?? null,
        reason: row.querySelector("[data-job-reason]")?.textContent ?? "",
        output: row.querySelector("[data-job-output]").textContent,
      };
    })()`);
  const slowFile = join(workspace, SLOW_SCRIPT);
  try {
    if (!ready) {
      step("pythonBridge SKIPPED: no Python 3.10 or newer on this machine");
      return;
    }
    writeFileSync(
      slowFile,
      `import time\nprint("waiting for ever", flush=True)\ntime.sleep(120)\nprint("never")\n`,
    );
    await openScriptJob(evaluate);
    await until("the bridge script listed", () =>
      exists(`${dialog} [name=job-script] option[value="${BRIDGE_SCRIPT}"]`),
    );
    if (await exists(`${dialog} [data-job-python-off]`))
      fail("Python is ready, yet the run-script dialog says it is off");
    await pickScript(BRIDGE_SCRIPT);
    await clickOn(`${dialog} [data-job=plan]`);
    await until("the script's pre-flight", () => atStep("preflight"));
    const plan = await evaluate(`(() => {
      const d = document.querySelector("${dialog}");
      return {
        hosts: [...d.querySelectorAll("[data-job-host]")].map((r) => r.dataset.jobHost).sort(),
        production: d.querySelector("[data-job-production-names]")?.textContent ?? "",
        startDisabled: d.querySelector("[data-job=start]").disabled,
        parallel: !!d.querySelector("[data-job-parallel]"),
      };
    })()`);
    const targets = [
      "Jobs/jobs-odd.yaml",
      "Jobs/jobs-r1.yaml",
      "Jobs/jobs-telnet.yaml",
    ];
    if (json(plan.hosts) !== json(targets))
      fail(`the script's pre-flight lists ${json(plan)}`);
    if (plan.production !== "jobs-r1" || !plan.startDisabled || plan.parallel)
      fail(`the script's production confirmation: ${json(plan)}`);
    await clickOn(`${dialog} [name=job-confirm-production]`);
    await until("the script's start enabled", () =>
      evaluate(
        `!document.querySelector("${dialog} [data-job=start]").disabled`,
      ),
    );
    await screenshot("script-job-preflight");
    step("a script job's pre-flight lists its hosts and confirms production");

    await clickOn(`${dialog} [data-job=start]`);
    await until(
      "the script finished",
      () => exists(`${dialog} [data-job-progress][data-finished]`),
      60000,
    );
    const done = await result();
    const lines = done.output.split("\n").filter(Boolean);
    const expected = [
      "hosts 3",
      ...targets.map((id) => `target ${id}`),
      "login Jobs/jobs-r1.yaml " + USERNAME,
      "finished on stderr",
    ];
    if (done.state !== "done" || done.exit !== "0")
      fail(`the script job ended ${json(done)}`);
    for (const line of expected)
      if (!lines.includes(line))
        fail(`the script's output lacks ${json(line)}: ${done.output}`);
    if (
      !lines.some((line) => /^banner Jobs\/jobs-r1\.yaml SSH-2\.0-/.test(line))
    )
      fail(`the script did not reach the example server: ${done.output}`);
    if (done.output.includes(PASSWORD))
      fail("the script job's output shows the password");
    await screenshot("script-job-done");
    step(
      "the script saw only the job's hosts, reached the example server, got its login, and succeeded by exit status",
    );

    // The run's tab keeps it: one output, its exit status, searchable.
    await clickOn(`${dialog} [data-job=view]`);
    await until("the script run in its tab", () =>
      exists(`${RUN_TAB} [data-run-host="script"]`),
    );
    if (await exists(dialog)) fail("Open run left the job tab open");
    const kept = await until("the kept script output", () =>
      evaluate(`(() => {
        const view = document.querySelector("${RUN_TAB}");
        const output = view.querySelector('[data-run-output="script"]')?.textContent ?? "";
        return output.includes("finished on stderr") && {
          exit: view.querySelector('[data-run-host="script"] [data-run-exit-status]')?.dataset.runExitStatus,
          targets: [...view.querySelectorAll("[data-run-target]")].map((r) => r.dataset.runTarget).sort(),
        };
      })()`),
    );
    if (kept.exit !== "0" || json(kept.targets) !== json(targets))
      fail(`the kept script run: ${json(kept)}`);
    await evaluate(`(() => {
      const input = document.querySelector("${RUN_TAB} [name=run-search]");
      input.value = "banner";
      input.dispatchEvent(new Event("input", { bubbles: true }));
    })()`);
    await mouseClick(evaluate, `${RUN_TAB} [data-run-action=search]`);
    const hit = await until("the search in the script's output", () =>
      evaluate(
        `document.querySelector('${RUN_TAB} [data-search-hit="script"]')?.dataset.count ?? null`,
      ),
    );
    // jobs-r1 and jobs-odd are SSH hosts; jobs-telnet is not.
    if (hit !== "2")
      fail(`"banner" found on ${hit} lines of the script's output`);
    step(
      "the script run is kept: its output, exit status and targets, searchable",
    );
    // Run again plans the same script for the same target.
    await clickOn(`${RUN_TAB} [data-run-action=again]`);
    const again = await until("the script planned again", () =>
      evaluate(`(() => {
        const d = document.querySelector("${dialog}[data-step=preflight]");
        return d && {
          summary: d.querySelector("[data-job-summary]").textContent,
          hosts: d.querySelectorAll("[data-job-host]").length,
        };
      })()`),
    );
    if (!again.summary.includes(BRIDGE_SCRIPT) || again.hosts !== 3)
      fail(`Run again of the script run: ${json(again)}`);
    step("Run again plans the same script for the same hosts");
    await clickOn(`${dialog} [data-job=dismiss]`);
    await until("the job tab closed", async () => !(await exists(dialog)));
    await closeTabOf(evaluate, ".slot[data-view-slot=run]");

    // A script that does not end is cancelled, keeping its output.
    await openScriptJob(evaluate);
    await until("the slow script listed", () =>
      exists(`${dialog} [name=job-script] option[value="${SLOW_SCRIPT}"]`),
    );
    await pickScript(SLOW_SCRIPT);
    await clickOn(`${dialog} [data-job=plan]`);
    await until("the slow script's pre-flight", () => atStep("preflight"));
    await clickOn(`${dialog} [name=job-confirm-production]`);
    await until("the slow script's start enabled", () =>
      evaluate(
        `!document.querySelector("${dialog} [data-job=start]").disabled`,
      ),
    );
    await clickOn(`${dialog} [data-job=start]`);
    await until(
      "the slow script's first line while it runs",
      async () => {
        const shown = await result();
        return (
          shown?.state === "running" &&
          shown.output.includes("waiting for ever")
        );
      },
      30000,
    );
    step("a script job's output appears while it runs");
    await clickOn(`${dialog} [data-job=cancel]`);
    await until("the slow script cancelled", () =>
      exists(`${dialog} [data-job-progress][data-finished]`),
    );
    const cancelled = await result();
    if (
      cancelled.state !== "cancelled" ||
      !cancelled.output.includes("waiting for ever") ||
      cancelled.output.includes("never")
    )
      fail(`the cancelled script: ${json(cancelled)}`);
    step("cancelling a script job stops it and keeps its output");
    await clickOn(`${dialog} [data-job=close]`);
    await until("the job tab closed", async () => !(await exists(dialog)));
  } finally {
    rmSync(join(workspace, BRIDGE_SCRIPT), { force: true });
    rmSync(slowFile, { force: true });
  }
}

/** A stand-in for NAPALM, put into the smoke's own Python environment. */
const FAKE_NAPALM = `"""NAPALM stood in for by the smoke test: facts made up per driver."""


def get_network_driver(name):
    class Device:
        def __init__(self, hostname, username, password, timeout=60, optional_args=None):
            if not username or not password:
                raise ValueError("no login was handed over")
            self.port = (optional_args or {}).get("port")

        def open(self):
            pass

        def get_facts(self):
            return {
                "vendor": {"ios": "Cisco", "eos": "Arista"}.get(name, name),
                "model": name + "-smoke",
                "serial_number": "SN-%s-%s" % (name, self.port),
                "os_version": "1.2.3",
                "hostname": "lab-r1",
            }

        def close(self):
            pass

    return Device
`;

/**
 * Fill in from devices (#55): on a folder with an IOS host (a typed vendor),
 * an EOS host and an FRR host (no NAPALM driver). Without NAPALM every host
 * fails saying so; with a stand-in NAPALM in the smoke's environment the
 * review shows new and replacing values, applying fills the empty ones and
 * keeps the typed vendor, and the host's own "Fill in from device" with the
 * host ticked replaces it. SKIPPED without Python.
 */
async function enrichment({ evaluate, screenshot, step, ready }) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const dialog = JOB_TAB;
  const review = ENRICH_TAB;
  const folder = join(workspace, "Enrich");
  if (!ready) {
    step("enrichment SKIPPED: no Python 3.10 or newer on this machine");
    return;
  }
  const site = spawnSync(
    ready,
    ["-c", "import sysconfig; print(sysconfig.get_paths()['purelib'])"],
    { encoding: "utf8", windowsHide: true },
  );
  if (site.status !== 0)
    fail(`the environment's site-packages: ${site.stderr}`);
  const fakeNapalm = join(site.stdout.trim(), "napalm");
  const r1File = join(folder, "enrich-r1.yaml");
  try {
    mkdirSync(folder, { recursive: true });
    writeFileSync(
      join(folder, "_folder.yaml"),
      `credential_profile: jobs-login\nport: ${DEVICE_PORT}\n`,
    );
    writeFileSync(
      r1File,
      "address: 127.0.0.1\nplatform: cisco_ios\nvendor: OldVendor\nowner: noc\n",
    );
    writeFileSync(
      join(folder, "enrich-r2.yaml"),
      "address: 127.0.0.1\nplatform: eos\n",
    );
    writeFileSync(
      join(folder, "enrich-frr.yaml"),
      "address: 127.0.0.1\nplatform: frr\n",
    );
    const folderRow = 'nav button[data-item=folder][data-path="Enrich"]';
    await until("the Enrich folder in the tree", () => exists(folderRow));

    /** Fill in from the side panel's `button`; resolves at the review. */
    const fillIn = async (row, button, hosts) => {
      await clickOn(row);
      await showDetails(evaluate);
      await until("the side panel's Fill in button", () =>
        exists(`.side ${button}`),
      );
      await clickOn(`.side ${button}`);
      const plan = await until("the fill-in pre-flight", () =>
        evaluate(`(() => {
          const d = document.querySelector("${dialog}[data-step=preflight]");
          return d && d.querySelector("[data-job-enrich-note]") && {
            hosts: [...d.querySelectorAll("[data-job-host]")].map((r) => r.dataset.jobHost).sort(),
            skipped: [...d.querySelectorAll("[data-job-skipped]")].map((r) => r.dataset.jobSkipped + " " + r.dataset.skipReason),
            saveable: !!d.querySelector("[data-save-job]"),
          };
        })()`),
      );
      if (json(plan.hosts) !== json(hosts))
        fail(`the fill-in pre-flight lists ${json(plan)}`);
      if (plan.saveable) fail("filling in offers to be saved as a job");
      await until("the fill-in's start enabled", () =>
        evaluate(
          `!document.querySelector("${dialog} [data-job=start]").disabled`,
        ),
      );
      await clickOn(`${dialog} [data-job=start]`);
      await until(
        "the fill-in finished",
        () => exists(`${dialog} [data-job=review]`),
        60000,
      );
      await clickOn(`${dialog} [data-job=review]`);
      await until("the review", () => exists(`${review}[data-step=preview]`));
      // A tab in the job tab's place (#73), not a dialog.
      await until(
        "the job tab replaced by the review",
        async () => !(await exists(JOB_TAB)),
      );
      const tabs = await evaluate(`({
        enrich: document.querySelectorAll("[data-tab-bar] [data-tab-view=enrich]").length,
        job: !!document.querySelector(${json(JOB_TAB)}),
        dialog: !!document.querySelector("[data-dialog=enrich]"),
      })`);
      if (tabs.enrich !== 1 || tabs.job || tabs.dialog)
        fail(`the review's tab: ${json(tabs)}`);
      return plan;
    };
    const rows = () =>
      evaluate(`Object.fromEntries([...document.querySelectorAll("${review} [data-enrich-host]")].map((r) => [
        r.dataset.enrichHost,
        {
          outcome: r.dataset.outcome,
          reason: r.querySelector("[data-enrich-reason]")?.textContent ?? "",
          changes: [...r.querySelectorAll("[data-enrich-change]")].map((c) => c.dataset.enrichChange + ":" + c.dataset.change),
          tick: !!r.querySelector("[data-enrich-replace]"),
        },
      ]))`);
    const report = () =>
      evaluate(
        `Object.fromEntries([...document.querySelectorAll("${review} [data-enrich-report]")].map((s) => [s.dataset.enrichReport, Number(s.dataset.count)]))`,
      );
    const closeReview = async (button) => {
      await clickOn(`${review} [data-enrich=${button}]`);
      await until(
        "the review's tab closed",
        async () => !(await exists("[data-tab-view=enrich]")),
      );
    };

    // Without NAPALM: every host fails, saying so.
    rmSync(fakeNapalm, { recursive: true, force: true });
    const plan = await fillIn(folderRow, "[data-folder-enrich]", [
      "Enrich/enrich-r1.yaml",
      "Enrich/enrich-r2.yaml",
    ]);
    if (json(plan.skipped) !== json(["Enrich/enrich-frr.yaml noNapalmDriver"]))
      fail(`the fill-in skips ${json(plan.skipped)}`);
    const without = await rows();
    for (const id of ["Enrich/enrich-r1.yaml", "Enrich/enrich-r2.yaml"])
      if (
        without[id]?.outcome !== "failed" ||
        !without[id].reason.includes("NAPALM is not installed")
      )
        fail(`without NAPALM, ${id}: ${json(without[id])}`);
    if (without["Enrich/enrich-frr.yaml"]?.outcome !== "skipped")
      fail(`the FRR host in the review: ${json(without)}`);
    if (
      !(await evaluate(
        `document.querySelector("${review} [data-enrich=apply]").disabled`,
      ))
    )
      fail("the review offers to fill in with nothing to fill in");
    await screenshot("enrich-without-napalm");
    step(
      "without NAPALM, filling in lists every host as failed, saying why, and skips FRR",
    );
    await closeReview("cancel");

    // With a stand-in NAPALM: new values and a typed one to replace.
    mkdirSync(fakeNapalm, { recursive: true });
    writeFileSync(join(fakeNapalm, "__init__.py"), FAKE_NAPALM);
    const before = readFileSync(r1File, "utf8");
    await fillIn(folderRow, "[data-folder-enrich]", [
      "Enrich/enrich-r1.yaml",
      "Enrich/enrich-r2.yaml",
    ]);
    const shown = await rows();
    const r1 = shown["Enrich/enrich-r1.yaml"];
    const r2 = shown["Enrich/enrich-r2.yaml"];
    if (
      json(r1?.changes) !==
        json([
          "vendor:replace",
          "model:new",
          "serial_number:new",
          "os_version:new",
        ]) ||
      !r1.tick
    )
      fail(`the review of enrich-r1: ${json(r1)}`);
    if (
      r2?.changes.length !== 4 ||
      r2.tick ||
      r2.changes.some((change) => !change.endsWith(":new"))
    )
      fail(`the review of enrich-r2: ${json(r2)}`);
    if (readFileSync(r1File, "utf8") !== before)
      fail("the review wrote to a host file before applying");
    await screenshot("enrich-review");
    step(
      "the review shows what is new and what would replace a typed value, writing nothing",
    );
    await clickOn(`${review} [data-enrich=apply]`);
    await until("the fill-in report", () =>
      exists(`${review}[data-step=report]`),
    );
    const counts = await report();
    if (
      json(counts) !== json({ filled: 2, unchanged: 0, skipped: 1, failed: 0 })
    )
      fail(`the fill-in report: ${json(counts)}`);
    const r1Text = readFileSync(r1File, "utf8");
    if (
      !r1Text.includes("vendor: OldVendor") ||
      !r1Text.includes("model: ios-smoke") ||
      !r1Text.includes(`serial_number: SN-ios-${DEVICE_PORT}`) ||
      !r1Text.includes("owner: noc") ||
      !/custom:\n {2}os_version: 1\.2\.3/.test(r1Text)
    )
      fail(`enrich-r1 after filling in: ${r1Text}`);
    const r2Text = readFileSync(join(folder, "enrich-r2.yaml"), "utf8");
    if (
      !r2Text.includes("vendor: Arista") ||
      !r2Text.includes("model: eos-smoke")
    )
      fail(`enrich-r2 after filling in: ${r2Text}`);
    await screenshot("enrich-report");
    step(
      "applying fills the empty fields (OS version as a custom field) and keeps the typed vendor",
    );
    await closeReview("close");

    // The host's own: ticked, the typed vendor is replaced.
    await expandFolder(evaluate, folderRow);
    const hostRow = 'nav [data-host="Enrich/enrich-r1.yaml"]';
    await until("enrich-r1 in the tree", () => exists(hostRow));
    await fillIn(hostRow, "[data-host-enrich]", ["Enrich/enrich-r1.yaml"]);
    const again = (await rows())["Enrich/enrich-r1.yaml"];
    if (json(again?.changes) !== json(["vendor:replace"]))
      fail(`the second review of enrich-r1: ${json(again)}`);
    await clickOn(`${review} [data-enrich-replace="Enrich/enrich-r1.yaml"]`);
    await clickOn(`${review} [data-enrich=apply]`);
    await until("the second report", () =>
      exists(`${review}[data-step=report]`),
    );
    if (!readFileSync(r1File, "utf8").includes("vendor: Cisco"))
      fail(`the ticked host kept its vendor: ${readFileSync(r1File, "utf8")}`);
    step("filling in one ticked host from its panel replaces its typed vendor");
    await closeReview("close");

    // The run stays reviewable from its tab, opened in the Jobs view.
    await openRun(evaluate);
    await until("the review action in the run's tab", () =>
      exists(`${RUN_TAB} [data-run-action=review]`),
    );
    await clickOn(`${RUN_TAB} [data-run-action=review]`);
    await until("the review from the run tab", () =>
      exists(`${review}[data-step=preview]`),
    );
    const later = await rows();
    if (later["Enrich/enrich-r1.yaml"]?.outcome !== "unchanged")
      fail(`the review from the run tab: ${json(later)}`);
    if (!(await exists("[data-tab-bar] [data-tab-view=run]")))
      fail("the run's tab closed when its review opened");
    step(
      "a kept fill-in run can be reviewed again from its tab, in a tab beside it",
    );
    await closeReview("cancel");
    await closeTabOf(evaluate, ".slot[data-view-slot=run]");
  } finally {
    rmSync(fakeNapalm, { recursive: true, force: true });
    rmSync(folder, { recursive: true, force: true });
  }
}

/** A Python 3.10 or newer of this machine ({ executable, version }), or null. */
function systemPython() {
  const told = process.env.SMOKE_PYTHON ? [[process.env.SMOKE_PYTHON]] : [];
  for (const [command, ...args] of [
    ...told,
    ["py", "-3"],
    ["python3"],
    ["python"],
  ]) {
    const done = spawnSync(
      command,
      [
        ...args,
        "-c",
        "import sys; print(sys.version_info >= (3, 10), '%d.%d' % sys.version_info[:2], sys.executable)",
      ],
      { encoding: "utf8", windowsHide: true },
    );
    if (done.status !== 0) continue;
    const [ok, version, ...executable] = done.stdout.trim().split(" ");
    if (ok === "True") return { executable: executable.join(" "), version };
  }
  return null;
}

/**
 * The Python environment (#51): off until set, the user's own interpreter
 * (stored, nothing installed, the missing package reported), then the app's
 * own environment in the smoke's data folder with no libraries (so no
 * network: only the wheel shipped beside the built app). Resolves to the
 * environment's interpreter, or null without a Python 3.10+ (then only the
 * "not found" message is checked).
 */
async function pythonSetup({ evaluate, fillForm, screenshot, step }) {
  const json = (value) => JSON.stringify(value);
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const clickIn = (scope, label) => clickButton(evaluate, label, scope);
  const shownStatus = () =>
    evaluate(`(() => {
      const status = document.querySelector("[data-python-status]");
      return status && {
        kind: status.dataset.pythonStatus,
        text: status.textContent.trim(),
        interpreter: document.querySelector("[data-python-interpreter]")?.textContent ?? null,
      };
    })()`);
  const untilStatus = (description, kind) =>
    until(description, async () => {
      const status = await shownStatus();
      return status?.kind === kind ? status : null;
    });
  const dialog = PYTHON_SETUP_TAB;
  const dialogStep = () =>
    evaluate(`document.querySelector(${json(dialog)})?.dataset.step ?? null`);
  const environment = join(appData, "python");

  await clickOn("[data-open-preferences]");
  const off = await untilStatus("Python off before it is set up", "notSet");
  if (
    !/Python features are off/.test(off.text) ||
    !/Preferences/.test(off.text)
  )
    fail(`the message while Python is not set: ${json(off)}`);
  step("Python features are off until set up, saying how to turn them on");

  const found = systemPython();
  if (!found) {
    await clickOn("[data-python=set-up]");
    const failed = await until("the setup without Python", async () =>
      (await dialogStep()) === "failed"
        ? evaluate(
            `document.querySelector("[data-python-setup-result=failed]").textContent`,
          )
        : null,
    );
    if (!/No Python was found/.test(failed))
      fail(`the setup without Python: ${failed}`);
    step("pythonSetup SKIPPED: no Python 3.10 or newer on this machine");
    await clickIn(dialog, "Close");
    return null;
  }

  // The user's own interpreter: stored, checked, nothing installed.
  await clickOn("[data-python=use-own]");
  await until("the interpreter field", () =>
    evaluate(`!!document.querySelector("[name=python-interpreter]")`),
  );
  await fillForm(
    { "python-interpreter": found.executable },
    "[data-python-form]",
  );
  const mine = await until("the own interpreter checked", async () => {
    const status = await shownStatus();
    return status && status.kind !== "notSet" ? status : null;
  });
  if (
    mine.kind === "problem" &&
    !/cannot import the app's package/.test(mine.text)
  )
    fail(`the own interpreter without the package: ${json(mine)}`);
  if (mine.interpreter !== found.executable)
    fail(`the own interpreter shown: ${json(mine)}`);
  step(
    `the own interpreter (Python ${found.version}) is stored and checked: ${mine.kind}`,
  );
  await screenshot("python-own-interpreter");
  await clickOn("[data-python=turn-off]");
  await untilStatus("Python off again", "notSet");

  // The app's own environment: nothing is installed before "Install".
  await clickOn("[data-python=set-up]");
  await until("the setup offer", async () => (await dialogStep()) === "offer");
  // A tab (#73): Preferences stays open beside it, and asking again shows
  // the same tab where it was.
  if (await evaluate(`!!document.querySelector("[data-dialog=python-setup]")`))
    fail("the Python setup is still a dialog");
  await clickOn("[data-tab-bar] [data-tab-view=preferences]");
  await until("Preferences shown again", async () =>
    evaluate(
      `!!document.querySelector(".slot.view:not([hidden]) [data-python=set-up]")`,
    ),
  );
  await clickOn(".slot.view:not([hidden]) [data-python=set-up]");
  await until("the setup tab shown again", async () => {
    const tabs = await evaluate(
      `document.querySelectorAll("[data-tab-bar] [data-tab-view=python-setup]").length`,
    );
    if (tabs !== 1) fail(`Python setup tabs: ${tabs}`);
    return (await dialogStep()) === "offer";
  });
  step("the Python setup opens as one tab beside Preferences");
  const offer = await evaluate(`(() => ({
    base: document.querySelector("[data-python-base]").textContent,
    environment: document.querySelector("[data-python-environment]").textContent,
    libraries: [...document.querySelectorAll(${json(dialog + " input[type=checkbox]")})].map((box) => box.name),
  }))()`);
  if (offer.environment !== environment)
    fail(`the environment offered: ${json(offer)}`);
  const libraries = ["nornir", "netmiko", "scrapli", "napalm"];
  if (
    json(offer.libraries) !==
    json(libraries.map((library) => `python-library-${library}`))
  )
    fail(`the libraries offered: ${json(offer)}`);
  // No libraries, so no package index.
  const ticked = await evaluate(
    `[...document.querySelectorAll(${json(dialog + " input[type=checkbox]")})].map((box) => box.checked)`,
  );
  for (const [index, checked] of ticked.entries()) {
    if (checked)
      await mouseClickElement(
        evaluate,
        `document.querySelectorAll(${json(dialog + " input[type=checkbox]")})[${index}]`,
        `library box ${index}`,
      );
  }
  await clickOn("[data-python-setup=continue]");
  await until(
    "the confirmation",
    async () => (await dialogStep()) === "confirm",
  );
  const installs = await evaluate(
    `[...document.querySelectorAll("[data-python-install]")].map((item) => item.dataset.pythonInstall + " " + item.dataset.source)`,
  );
  if (installs.length !== 1 || !/^netterm-.*\.whl wheel$/.test(installs[0]))
    fail(`what the setup lists: ${json(installs)}`);
  if (existsSync(environment))
    fail("the environment exists before the setup was confirmed");
  step(`the setup lists ${json(installs)} and installed nothing yet`);
  await screenshot("python-setup-confirm");
  await clickOn("[data-python-setup=install]");
  const result = await until(
    "the environment set up",
    async () => {
      const at = await dialogStep();
      if (at === "failed")
        fail(
          `the setup failed: ${await evaluate(`document.querySelector("[data-python-setup-result]").textContent`)}`,
        );
      return at === "done"
        ? evaluate(
            `document.querySelector("[data-python-setup-result=done]").textContent`,
          )
        : null;
    },
    180000,
  );
  if (!/Python features are on/.test(result))
    fail(`the setup's result: ${result}`);
  await screenshot("python-setup-done");
  await clickIn(dialog, "Close");
  await clickOn("[data-open-preferences]");
  const ready = await untilStatus("Python ready", "ready");
  if (!ready.interpreter?.startsWith(environment))
    fail(`the environment's interpreter: ${json(ready)}`);
  step(`the app's environment was created: ${ready.interpreter}`);
  await closePreferences(evaluate);
  return ready.interpreter;
}

/** The Python choice survives a restart (the Preferences tab is open). */
async function pythonAfterRestart({ evaluate, expected, step }) {
  if (!expected) return;
  const status = await until("the Python status after a restart", () =>
    evaluate(`(() => {
      const status = document.querySelector("[data-python-status]");
      return status && {
        kind: status.dataset.pythonStatus,
        interpreter: document.querySelector("[data-python-interpreter]")?.textContent ?? null,
      };
    })()`),
  );
  if (status.kind !== "ready" || status.interpreter !== expected)
    fail(`the Python choice after a restart: ${JSON.stringify(status)}`);
  step("the Python choice was kept across a restart");
}

async function portableUpdateCheck({ evaluate, requestsBeforeRestart, step }) {
  if (!(await evaluate(`!!document.querySelector("[data-view=preferences]")`)))
    await mouseClick(evaluate, "[data-open-preferences]");
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

  await mouseClick(evaluate, `[data-update-check-now]`);
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
  await mouseClick(evaluate, `[data-update-page]`);
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
  // The session tried on: once the rules' tab is beside it, the other
  // pane may be the active one.
  const sessionTab = await evaluate(
    `document.querySelector("${SHOWN_SESSION}").dataset.tab`,
  );
  const sessionSlot = `.slot[data-tab="${sessionTab}"]`;
  // The last row holding `mark`, its text, the cells coloured on it, and
  // the terminal's palette colours the rules below name.
  const row = (mark) =>
    evaluate(`(() => {
      const slot = document.querySelector('${sessionSlot} .terminal');
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
      const field = document.querySelector("[data-view=preferences] [name=${name}]");
      field.checked = ${json(value)};
      field.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
  const RULES = "[data-view=highlights]";
  /** Opens the rules' tab from the preferences (which name the rules
   * beside the switch) and splits it into a pane of its own, below the
   * terminal the rules are tried on (down, so long lines do not wrap). */
  const openRulesBeside = async () => {
    await openPreferences(evaluate);
    await mouseClick(
      evaluate,
      `[data-view=preferences] [data-open-highlights]`,
    );
    await until("the highlight rules tab", () =>
      evaluate(`(() => {
        const view = document.querySelector("${RULES}");
        return !!view && !view.closest("[hidden]");
      })()`),
    );
    // The Preferences tab would sit over the terminal.
    await mouseClick(evaluate, `[data-tab-view=preferences] [data-tab-close]`);
    await until(
      "the Preferences tab closed",
      async () => !(await exists("[data-tab-view=preferences]")),
    );
    await mouseClick(evaluate, `[data-tab-view=highlights]`);
    await paneTool(
      evaluate,
      `document.querySelector("[data-tab-view=highlights]").closest("[data-tab-bar]")`,
      "split-down",
      "the rules tab's pane",
    );
    await until("the rules' tab in a pane of its own", () =>
      evaluate(
        `document.querySelector("[data-tab-view=highlights]").closest("[data-tab-bar]").querySelectorAll("[data-tab]").length === 1`,
      ),
    );
    await mouseClick(evaluate, `[data-tab-bar] [data-tab="${sessionTab}"]`);
    await until("the rules' tab beside the terminal", () =>
      evaluate(`(() => {
        const view = document.querySelector(".slot[data-view-slot=highlights]");
        const session = document.querySelector('${sessionSlot}');
        return !!view && !view.hidden && !session.hidden &&
          view.dataset.pane !== session.dataset.pane;
      })()`),
    );
  };
  const closeRules = async () => {
    await mouseClick(evaluate, `${RULES} [data-rule=close]`);
    await until(
      "the highlight rules tab closed",
      async () =>
        !(await exists(RULES)) && !(await exists("[data-tab-view=highlights]")),
    );
  };
  const focusTerminal = () =>
    evaluate(
      `document.querySelector('${sessionSlot} .xterm textarea').focus()`,
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
  // (The Preferences tab hides the terminal, so it is closed to look.)
  await openPreferences(evaluate);
  await setPreference("preference-highlighting", false);
  await until("highlighting off saved", () =>
    readFileSync(join(appData, "preferences.yaml"), "utf8").includes(
      "highlighting: false",
    ),
  );
  await closePreferences(evaluate);
  await until("no highlights", () => coloured("hlcheck", []));
  await openPreferences(evaluate);
  await setPreference("preference-highlighting", true);
  await closePreferences(evaluate);
  await until("the highlights back", () => coloured("hlcheck", builtIn));
  step("the preference turned highlighting off and on again");

  // A workspace rule, added in the rules' tab beside the terminal, is
  // saved to the workspace and shows on that terminal.
  await openRulesBeside();
  await mouseClick(evaluate, `[data-rule=new]`);
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
  await focusTerminal();
  await type("hlrule SMOKEWORD\r");
  await until("the workspace rule's highlight", () =>
    coloured("hlrule", [["SMOKEWORD", "magenta"]]),
  );
  step(
    "a rule added in the rules' tab was saved to the workspace and applied to the terminal beside it",
  );

  // One rule turned off: only its highlights go.
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
  await closeRules();
  rmSync(rulesFile);
  step("a broken pattern was reported and skipped; the other rules work");
}

/**
 * Snippets: the empty Snippets view offers the first, which is created
 * and saved to the workspace; the view's filter narrows the list; it is
 * sent to a host session from the view and from the palette (its host's
 * fields filled in, the other variable asked for, line by line), edited,
 * deleted, and one written on disk shows in the view.
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
  const clickOn = (selector) => mouseClick(evaluate, selector);
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

  // With none yet, the Snippets view says what they are and offers the
  // first; created there, saved to the workspace's snippets file.
  await showSideView(evaluate, "snippets");
  await until("the empty Snippets view", () =>
    exists("[data-snippets] [data-snippets-empty] [data-snippet-first]"),
  );
  await clickOn("[data-snippet-first]");
  // The form opens in the view, in place of the list (#79): nothing
  // window-modal, Escape goes back to the list.
  const inView =
    "[data-side-view=snippets] [data-snippets][data-mode=edit] form[data-snippet-form]";
  await until("the snippet form in the view", () => exists(inView));
  if (await exists("[aria-modal=true]")) fail("the snippet form is modal");
  await evaluate(
    `document.querySelector("input[name=snippet-name]").dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }))`,
  );
  await until("the list again after Escape", () =>
    exists("[data-snippets][data-mode=list] [data-snippet-first]"),
  );
  await clickOn("[data-snippet-first]");
  await until("the snippet form", () => exists(inView));
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

  // The filter narrows the list as the user types (the core matches every
  // word in the name, group or text), keeping the groups.
  const saved = fileText();
  writeFileSync(
    file,
    `${saved}Clock: show clock
`,
  );
  await until("a second snippet in the view", () =>
    exists('[data-snippet-group=""] [data-snippet=Clock]'),
  );
  const filter = async (query) => {
    await evaluate(`(() => {
      const input = document.querySelector("input[name=snippet-filter]");
      input.value = ${json(query)};
      input.dispatchEvent(new Event("input", { bubbles: true }));
    })()`);
  };
  const listed = () =>
    evaluate(
      `[...document.querySelectorAll("[data-snippets] [data-snippet]")].map((b) => b.dataset.snippet)`,
    );
  const filtered = async (query, expected) => {
    await filter(query);
    await until(`the snippets matching ${json(query)}`, async () => {
      const names = await listed();
      return json(names) === json(expected);
    });
  };
  await filtered("snipb", ["Smoke check"]);
  await filtered("SMOKE {{vlan}}", ["Smoke check"]);
  await filtered("clock", ["Clock"]);
  await filtered("nothing-like-it", []);
  await until("the no-match message", () => exists("[data-snippets-none]"));
  await filtered("", ["Clock", "Smoke check"]);
  writeFileSync(file, saved);
  await until(
    "the second snippet gone",
    async () => json(await listed()) === json(["Smoke check"]),
  );
  const target = await evaluate(
    `document.querySelector("[data-snippet-target]").dataset.snippetTarget`,
  );
  if (target !== "snip-r1") fail(`snippets are sent to ${json(target)}`);
  step("the Snippets view filtered by name, group and text as typed");

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
        const dialog = document.querySelector("${SHOWN_SESSION} [data-snippet-variables]");
        return dialog && [...dialog.querySelectorAll("[data-variable]")].map((i) => i.dataset.variable);
      })()`),
    );
    if (json(asked) !== json(["vlan"])) fail(`asked for ${json(asked)}`);
    // Over its own pane only (#79): nothing window-modal.
    if (await exists("[aria-modal=true]"))
      fail("the variables question is modal");
    await fillForm(
      { "variable-0": vlan },
      `${SHOWN_SESSION} [data-snippet-variables] form`,
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
  await showSideView(evaluate, "hosts");
  await clickOn('[aria-label="Search hosts"]');
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
  await showSideView(evaluate, "snippets");
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
  await clickButton(evaluate, "Delete", "[role=dialog]");
  await until("the snippets file gone", () => !existsSync(file));
  await until(
    "the snippet gone from the panel",
    async () => !(await exists("[data-snippet]")),
  );
  await until("the empty Snippets view again", () =>
    exists("[data-snippets-empty]"),
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
  await showSideView(evaluate, "hosts");
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
  const clickOn = (selector) => mouseClick(evaluate, selector);
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
  await expandFolder(evaluate, folderRow);
  const hostRow = 'nav [data-host="Firewalls/fw-r1.yaml"]';
  await until("fw-r1 in the tree", () => exists(hostRow));
  await clickOn(hostRow);
  await showDetails(evaluate);
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
async function csvImport({
  evaluate,
  fillForm,
  text,
  mouse,
  centreOf,
  screenshot,
  step,
}) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const dialog = ".slot.view [data-view=csv-import]";
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
        "imp-port;10.9.0.6:2222;;;;;",
        "",
      ].join("\r\n"),
  );
  const oddFile = join(workspace, "odd.yaml");
  const oddBefore = readFileSync(oddFile, "utf8");

  const importTabs = () =>
    evaluate(
      `document.querySelectorAll(".slot.view[data-view-slot=csv-import]").length`,
    );
  // Opened from the Hosts view's add control, or from a folder's context
  // menu (`fromFolder`), whose folder is then the target.
  const readFile = async (fromFolder) => {
    if (fromFolder === undefined) {
      await clickOn("[data-add-menu]");
      await clickOn("[data-add-entries] [data-open-csv-import]");
    } else {
      await mouseClick(
        evaluate,
        `nav button[data-item=folder][data-path="${fromFolder}"]`,
        { button: "right" },
      );
      await until("the folder's context menu", () =>
        exists("[role=menu] [data-menu=import-csv]"),
      );
      await clickOn("[role=menu] [data-menu=import-csv]");
    }
    await until("the CSV import tab", () => atStep("file"));
    if (
      !(await exists(".slot.view[data-view-slot=csv-import][data-view-active]"))
    )
      fail("the CSV import tab is not the active pane's shown tab");
    if (!(await exists("nav [data-host]")))
      fail("the tree is not in view beside the CSV import tab");
    await fillForm({ "csv-file": csvFile }, `${dialog} form`);
    await until("the column mapping", () => atStep("map"));
    if (fromFolder !== undefined) {
      const target = await evaluate(
        `document.querySelector("${dialog} select[name=csv-target]").value`,
      );
      if (target !== fromFolder)
        fail(`the CSV import opened on ${fromFolder} targets ${json(target)}`);
    }
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
  const expected = "2:create 3:create 4:exists 5:skip 6:fail 7:fail";
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
  const first = "created=2 updated=1 skipped=2 failed=2";
  await until(`the report ${first}`, async () => (await counts()) === first);
  const failed = await text(`${dialog} [data-report=failed]`);
  if (!failed.includes("Row 6")) fail(`the failed row: ${failed}`);
  // An address with its port, refused as New host refuses it (#131).
  if (
    !failed.includes("Row 7") ||
    !failed.includes("write the address without the port (10.9.0.6)")
  )
    fail(`the failed row with a port: ${failed}`);
  if (existsSync(join(workspace, "imp-port.yaml")))
    fail("the row with a port in its address was imported");
  // New hosts come with the next step: connecting, and a profile (#116).
  if (
    !(await text(`${dialog} [data-report-next]`)).includes(
      "name a credential profile",
    )
  ) {
    fail("the CSV report does not say what to do next");
  }
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
  // The tree beside the tab shows what the import wrote, the tab still open.
  await until("the imported folder in the tree beside the tab", () =>
    exists('nav button[data-item=folder][data-path="Europe"]'),
  );
  if (!(await atStep("report"))) fail("the CSV import tab left its report");
  await clickOn(`${dialog} [data-csv=close]`);
  await until("the CSV import tab closed", async () => !(await exists(dialog)));
  step(
    "importing creates folders under the target, replaces only the confirmed host and reports every row, the tree updating beside the tab",
  );

  // Again, from a folder's context menu (its folder the target), confirming
  // nothing: every host exists now and none is written. Opening the import
  // again meanwhile brings its one tab forward.
  const madridBefore = readFileSync(
    join(workspace, "Europe", "Madrid", "imp-1.yaml"),
  );
  writeFileSync(oddFile, oddBefore);
  await readFile("Europe");
  await clickOn("[data-add-menu]");
  await clickOn("[data-add-entries] [data-open-csv-import]");
  await until("the CSV import still at its mapping", () => atStep("map"));
  if ((await importTabs()) !== 1)
    fail(`opening the CSV import twice made ${await importTabs()} tabs`);
  await preview();
  await until(
    "every host existing",
    async () =>
      (await outcomes()) === "2:exists 3:exists 4:exists 5:skip 6:fail 7:fail",
  );
  // Nothing new and nothing ticked: "Import 0 hosts" is off (#131), on
  // once a host is ticked, off again when it is not.
  const importButton = `document.querySelector("${dialog} [data-csv=import]")`;
  await until("Import 0 hosts, disabled", () =>
    evaluate(
      `${importButton}.disabled && ${importButton}.textContent.replace(/\\s+/g, " ").includes("Import 0 hosts")`,
    ),
  );
  await clickOn(`${dialog} [data-replace="odd.yaml"]`);
  await until("Import 1 host, enabled", () =>
    evaluate(`!${importButton}.disabled`),
  );
  await clickOn(`${dialog} [data-replace="odd.yaml"]`);
  await until("Import 0 hosts, disabled again", () =>
    evaluate(`${importButton}.disabled`),
  );
  if (readFileSync(oddFile, "utf8") !== oddBefore)
    fail("odd.yaml was overwritten");
  if (
    !readFileSync(join(workspace, "Europe", "Madrid", "imp-1.yaml")).equals(
      madridBefore,
    )
  ) {
    fail("imp-1.yaml was overwritten");
  }
  await clickOn(`${dialog} [data-csv=cancel]`);
  await until("the CSV import tab closed", async () => !(await exists(dialog)));
  step(
    "a re-import from a folder's menu, in the one tab, offers no import with nothing new or ticked",
  );

  // A first export's mistake: no row of column names. The first device is
  // read as the columns, and the preview is refused saying what to add
  // (#116); a file that is not there is said plainly.
  const headless = join(userData, "no-header.csv");
  writeFileSync(headless, "r7,10.9.0.7,ISR4321\r\nr8,10.9.0.8,ISR4321\r\n");
  await clickOn("[data-add-menu]");
  await clickOn("[data-add-entries] [data-open-csv-import]");
  await until("the CSV import tab", () => atStep("file"));
  await fillForm({ "csv-file": join(userData, "gone.csv") }, `${dialog} form`);
  await until("the missing file said", async () =>
    (await text(`${dialog} [data-csv-error]`)).includes(
      "there is no such file; check the path",
    ),
  );
  await fillForm({ "csv-file": headless }, `${dialog} form`);
  await until("the column mapping", () => atStep("map"));
  await clickOn(`${dialog} [data-csv=preview]`);
  await until("the missing column names said", async () =>
    (await text(`${dialog} [data-csv-error]`)).includes(
      "the first row looks like a device: the first row must name the columns",
    ),
  );
  await screenshot("csv-no-header");
  await clickOn(`${dialog} [data-csv=cancel]`);
  await until("the CSV import tab closed", async () => !(await exists(dialog)));
  step(
    "a CSV file with no row of column names, and one that is not there, say what to do",
  );
}

/**
 * Nornir import: a simple inventory (hosts, groups and defaults files) into
 * the Europe folder the CSV import made: inherited values, groups as tags,
 * data as custom fields, passwords never written, and a re-import that
 * overwrites nothing.
 */
async function nornirImport({
  evaluate,
  fillForm,
  text,
  mouse,
  centreOf,
  screenshot,
  step,
}) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const dialog = ".slot.view [data-view=nornir-import]";
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
      "spaced:",
      "  hostname: 10.8.0.3 10.8.0.4",
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
  // Opened from the Hosts view's add control (the target then chosen), or
  // from the Europe folder's context menu (`fromMenu`), which targets it.
  const preview = async (fromMenu = false) => {
    if (fromMenu) {
      await mouseClick(
        evaluate,
        'nav button[data-item=folder][data-path="Europe"]',
        { button: "right" },
      );
      await until("the folder's context menu", () =>
        exists("[role=menu] [data-menu=import-nornir]"),
      );
      await clickOn("[role=menu] [data-menu=import-nornir]");
    } else {
      await clickOn("[data-add-menu]");
      await clickOn("[data-add-entries] [data-open-nornir-import]");
    }
    await until("the Nornir import tab", () => atStep("files"));
    if (
      !(await exists(
        ".slot.view[data-view-slot=nornir-import][data-view-active]",
      ))
    )
      fail("the Nornir import tab is not the active pane's shown tab");
    if (!(await exists("nav [data-host]")))
      fail("the tree is not in view beside the Nornir import tab");
    if (fromMenu) {
      const target = await evaluate(
        `document.querySelector("${dialog} select[name=nornir-target]").value`,
      );
      if (target !== "Europe")
        fail(`the Nornir import opened on Europe targets ${json(target)}`);
    } else {
      await evaluate(`(() => {
        const target = document.querySelector("${dialog} select[name=nornir-target]");
        target.value = "Europe";
        target.dispatchEvent(new Event("change", { bubbles: true }));
      })()`);
    }
    await fillForm(files, `${dialog} form`);
    await until("the Nornir preview", () => atStep("preview"));
  };
  const runImport = async () => {
    await clickOn(`${dialog} [data-nornir=import]`);
    await until("the Nornir report", () => atStep("report"));
  };
  const close = async () => {
    await clickOn(`${dialog} [data-nornir=close]`);
    await until(
      "the Nornir import tab closed",
      async () => !(await exists(dialog)),
    );
  };

  // The preview: each host's fate, the passwords left out, nothing written.
  await preview();
  const expected = "nr-1:create nr-2:create broken:fail spaced:fail";
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
  const first = "created=2 updated=0 skipped=3 failed=2";
  await until(`the report ${first}`, async () => (await counts()) === first);
  // A hostname with a space in it, refused as New host refuses it (#131).
  if (
    !(await text(`${dialog} [data-report=failed]`)).includes(
      "the address has a space in it (10.8.0.3 10.8.0.4)",
    )
  )
    fail("the Nornir report does not say the spaced hostname was refused");
  if (existsSync(join(workspace, "Europe", "spaced.yaml")))
    fail("the host with a space in its hostname was imported");
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
  await until("an imported Nornir host in the tree beside the tab", () =>
    exists('nav [data-host="Europe/nr-1.yaml"]'),
  );
  if (!(await atStep("report"))) fail("the Nornir import tab left its report");
  await close();
  step(
    "importing writes inherited values, group tags and data fields, never passwords, the tree updating beside the tab",
  );

  // Again, from the folder's context menu: both hosts exist now, nothing is
  // confirmed, nothing is written.
  writeFileSync(nr2, "address: 10.8.0.2\ndescription: edited by hand\n");
  const nr2Before = readFileSync(nr2, "utf8");
  await preview(true);
  await until(
    "both Nornir hosts existing",
    async () =>
      (await outcomes()) === "nr-1:exists nr-2:exists broken:fail spaced:fail",
  );
  // Nothing new and nothing ticked: "Import 0 hosts" is off (#131).
  const importButton = `document.querySelector("${dialog} [data-nornir=import]")`;
  await until("Import 0 hosts, disabled", () =>
    evaluate(
      `${importButton}.disabled && ${importButton}.textContent.replace(/\\s+/g, " ").includes("Import 0 hosts")`,
    ),
  );
  if (readFileSync(nr1, "utf8") !== expectedNr1)
    fail("nr-1.yaml was overwritten");
  if (readFileSync(nr2, "utf8") !== nr2Before)
    fail("nr-2.yaml was overwritten");
  await clickOn(`${dialog} [data-nornir=cancel]`);
  await until(
    "the Nornir import tab closed",
    async () => !(await exists(dialog)),
  );
  step("a Nornir re-import with nothing new or ticked offers no import");
}

/**
 * A running job's run in a window of its own (#77): its tab's "Open in
 * window" closes the tab and opens a second window (a second DevTools
 * page) showing what already happened, in the same theme and font; the
 * main window still counts the job. Reloaded, the window still shows it
 * (the shell keeps the jobs). Closed, the job goes on; detached again
 * from its reopened tab, it docks back as a tab and the window goes.
 */
async function detachedRunWindow({ evaluate, exists, clickOn, run, step }) {
  const json = (value) => JSON.stringify(value);
  const label = `run-${run}`;
  const tab = `${RUN_TAB} [data-run-details="${run}"]`;
  const inRun = `[data-detached=run][data-detached-run="${run}"]`;
  const host = (state) =>
    `${inRun} [data-run-details="${run}"] [data-run-host="Jobs/jobs-r1.yaml"][data-host-state=${state}]`;
  const showing = async (window, what) => {
    const shown = (selector) =>
      window.evaluate(`!!document.querySelector(${json(selector)})`);
    await until(`the run's window ${what}`, () => shown(host("running")));
    await until(`the output so far, in the window ${what}`, async () =>
      (
        await window.evaluate(
          `document.querySelector(${json(`${inRun} [data-run-output]`)})?.textContent ?? ""`,
        )
      ).includes("lab-r1# hang"),
    );
    // (The window reads the live jobs once it has loaded.)
    for (const selector of [
      `${inRun} [data-run-action=dock]`,
      `${inRun} [data-run-action=cancel]`,
    ]) {
      await until(`the run's window ${what} with ${selector}`, () =>
        shown(selector),
      );
    }
    for (const selector of [
      "[data-rail]",
      ".terminal",
      `${inRun} [data-run-action=detach]`,
      `${inRun} [data-run-action=again]`,
    ]) {
      if (await shown(selector)) fail(`the run's window shows ${selector}`);
    }
  };
  const look = (target) =>
    target(`({
      theme: document.documentElement.dataset.theme,
      font: getComputedStyle(document.documentElement).getPropertyValue("--font-mono").trim(),
    })`);

  await clickOn(`${tab} [data-run-action=detach]`);
  await until("the run's tab closed", async () => !(await exists(tab)));
  let window = await connectWindow(label);
  await showing(window, "opened");
  await checkContrast(window.evaluate, "run window");
  // Its own frame too (#80), with its title in the bar.
  await checkOwnFrame(window.evaluate, "the run's window");
  const titled = await until("the run's window title", () =>
    window.evaluate(`(() => {
      const bar = document.querySelector("[data-window-bar] [data-window-title]")?.textContent;
      return bar && bar === document.title && document.title.endsWith(${json(`run ${run}`)}) && bar;
    })()`),
  );
  step(`the run's window has the app's own frame, titled "${titled}"`);
  {
    const least = await narrowFrame(window.evaluate, "the run's window", 420);
    step(
      `at its narrowest (${least} px) the run's window's three buttons stayed whole inside it at its top right, over nothing, beside its title that drags it, and Maximize and Restore answered`,
    );
  }
  const main = await look(evaluate);
  const detached = await look(window.evaluate);
  if (json(main) !== json(detached) || !main.font.includes("Cascadia Mono"))
    fail(
      `the run's window looks ${json(detached)}, the main one ${json(main)}`,
    );
  if (!(await exists('[data-rail=jobs][data-jobs-running="1"]')))
    fail("the rail no longer counts the detached job");
  await showSideView(evaluate, "jobs");
  await until("the Jobs view listing the detached job running", () =>
    exists(`${JOBS_SIDE} [data-run="${run}"][data-run-state=running]`),
  );
  await showSideView(evaluate, "hosts");
  step(
    "a running job's run opened in a window of its own, showing what already happened, in the app's theme and font; the main window still counts it",
  );

  // A reload forgets nothing: the shell keeps the job.
  await window.devtools.send("Page.reload");
  await sleep(500);
  await showing(window, "reloaded");
  step("the run's window, reloaded, still shows the running job");

  // Closed by its own Close button (#80): the job goes on.
  await mouseClick(window.evaluate, `[data-window-control=close]`, {
    mayClose: true,
  });
  window.devtools.close();
  await until(
    "the run's window closed",
    async () => !(await windowOpen(label)),
  );
  await sleep(500);
  if (!(await exists('[data-rail=jobs][data-jobs-running="1"]')))
    fail("closing the run's window stopped the job");
  step("closing the run's window left the job running");

  // Again from its reopened tab, then docked back.
  await openRun(evaluate, run);
  await clickOn(`${tab} [data-run-action=detach]`);
  await until("the run's tab closed again", async () => !(await exists(tab)));
  window = await connectWindow(label);
  await showing(window, "opened again");
  await mouseClick(window.evaluate, `${inRun} [data-run-action=dock]`, {
    mayClose: true,
  });
  window.devtools.close();
  await until("the run's tab in the main window", () => exists(tab));
  await until("the run's window gone", async () => !(await windowOpen(label)));
  step(
    "Dock back opened the run as a tab in the main window and closed its window",
  );
}

/**
 * Closing the main window closes the detached ones (#77), and so the app:
 * a run window (of a run that is gone: it says so) is opened, then the
 * main window is closed.
 */
async function closingTheMainWindow({ app, evaluate, step }) {
  const label = "run-2000-01-01_00-00-00";
  await evaluate(
    `window.__TAURI_INTERNALS__.invoke("window_detach_run", { run: "2000-01-01_00-00-00" })`,
  );
  const window = await connectWindow(label);
  await until("the gone run's window says so", () =>
    window.evaluate(
      `!!document.querySelector("[data-detached] [data-run-gone]")`,
    ),
  );
  window.devtools.close();
  // A config history window too (#78), for a host the workspace may lack.
  const host = "Jobs/jobs-r1.yaml";
  await evaluate(
    `window.__TAURI_INTERNALS__.invoke("window_detach_configs", { host: ${JSON.stringify(host)} })`,
  );
  const configWindow = await connectWindow(
    `config-${Buffer.from(host, "utf8").toString("hex")}`,
  );
  await until("the config history's window", () =>
    configWindow.evaluate(
      `!!document.querySelector("[data-detached=configs]")`,
    ),
  );
  await checkOwnFrame(configWindow.evaluate, "the config history's window");
  const titled = await configWindow.evaluate(
    `document.querySelector("[data-window-bar] [data-window-title]")?.textContent ?? ""`,
  );
  if (!titled.includes("config history"))
    fail(`the config history's window bar says "${titled}"`);
  const least = await narrowFrame(
    configWindow.evaluate,
    "the config history's window",
    420,
  );
  step(
    `at its narrowest (${least} px) the config history's window's three buttons stayed whole inside it at its top right, over nothing, beside its title that drags it, and Maximize and Restore answered`,
  );
  configWindow.devtools.close();
  // The main window's own Close button (#80) ends the app.
  await mouseClick(evaluate, `[data-window-control=close]`, { mayClose: true });
  await until(
    "the app ended with its main window",
    () => app.exitCode !== null,
    15_000,
  );
  step(
    "the config history's window has the app's own frame; the main window's Close button closed the run and config history windows and ended the app",
  );
}

/**
 * Runs the shell command `command` with `args` in the page `evaluate`
 * reaches, and resolves to its answer (an error fails the smoke).
 */
async function invokeIn(evaluate, command, args = {}) {
  const key = `__smokeInvoke_${command}`;
  await evaluate(`(() => {
    window.${key} = null;
    window.__TAURI_INTERNALS__
      .invoke(${JSON.stringify(command)}, ${JSON.stringify(args)})
      .then((value) => (window.${key} = { value }), (error) => (window.${key} = { error: String(error) }));
  })()`);
  const answer = await until(`the answer to ${command}`, () =>
    evaluate(`window.${key}`),
  );
  if (answer.error !== undefined) fail(`${command}: ${answer.error}`);
  return answer.value;
}

/** The labels of the open session windows. */
async function sessionWindowLabels() {
  const labels = [];
  for (const target of await pageTargets()) {
    const client = await connectTarget(target);
    const label = await evaluateIn(client)(
      "window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label ?? null",
    ).catch(() => null);
    client.close();
    if (label?.startsWith("session-")) labels.push(label);
  }
  return labels;
}

/** The tabs a window shows: id, state and name. */
function tabsIn(evaluate) {
  return evaluate(`[...document.querySelectorAll("[data-tab-bar] [role=tab]")].map((tab) => ({
    id: Number(tab.dataset.tab),
    state: tab.dataset.tabState,
    name: tab.querySelector("[data-tab-name]")?.textContent ?? "",
  }))`);
}

/** Presses `key` (with `modifiers`) through `client`. */
async function pressIn(client, key, code, windowsVirtualKeyCode, modifiers) {
  const event = { key, code, windowsVirtualKeyCode, modifiers };
  await client.send("Input.dispatchKeyEvent", { type: "keyDown", ...event });
  await client.send("Input.dispatchKeyEvent", { type: "keyUp", ...event });
}

/**
 * Moves the connected session `name` of the main window to a new session
 * window, and resolves to that window (`label`, `devtools`, `evaluate`)
 * once it shows the session connected.
 */
async function moveToNewWindow(evaluate, name) {
  const tab = (await tabsIn(evaluate)).find(
    (each) => each.name === name && each.state === "connected",
  );
  if (!tab)
    fail(
      `no connected ${name} to move: ${JSON.stringify(await tabsIn(evaluate))}`,
    );
  await mouseClick(evaluate, `[data-tab-bar] [data-tab="${tab.id}"]`);
  await until(`${name} shown`, () =>
    evaluate(
      `!!document.querySelector('.slot[data-tab="${tab.id}"][data-active]')`,
    ),
  );
  const before = new Set(await sessionWindowLabels());
  await sessionAction(evaluate, "open-in-window", SHOWN_SESSION);
  const label = await until("the new session window", async () =>
    (await sessionWindowLabels()).find((each) => !before.has(each)),
  );
  const window = await connectWindow(label);
  await until(`${name} connected in its window`, async () => {
    const tabs = await tabsIn(window.evaluate);
    if (tabs.some((each) => each.name === name && each.state === "connected"))
      return true;
    // What the window shows instead, for the time-out's message.
    throw new Error(JSON.stringify(tabs));
  });
  return { label, ...window };
}

/**
 * Searches the palette open in `evaluate`'s window for `query` until
 * `selector` is listed (the workspace may still be reloading a file just
 * written), then chooses it.
 */
async function paletteChoose(evaluate, query, selector) {
  const set = (value) =>
    evaluate(`(() => {
      const input = document.querySelector("input[name=palette-query]");
      input.value = ${JSON.stringify(value)};
      input.dispatchEvent(new Event("input", { bubbles: true }));
    })()`);
  await until(`${selector} in the palette`, async () => {
    if (await evaluate(`!!document.querySelector(${JSON.stringify(selector)})`))
      return true;
    await set("");
    await set(query);
    await sleep(200);
    return evaluate(`!!document.querySelector(${JSON.stringify(selector)})`);
  });
  // Clicked: another hit may rank first, and Enter takes that one.
  await mouseClick(evaluate, selector);
}

/** Closes the window `evaluate` reaches with its own Close button. */
function closeWithButton(evaluate) {
  return mouseClick(evaluate, `[data-window-control=close]`, {
    mayClose: true,
  });
}

/**
 * The palette in a session window and closing session windows (#88): after
 * safetyInSessionWindows (lab-r9 open and connected in the main window).
 * Ctrl+K there connects a telnet host into that window, and a snippet
 * found with > goes to that window's active terminal. Closing the window
 * with live sessions asks: Cancel keeps it, Dock back brings both back
 * connected, and Close them (another window) ends its session.
 */
async function closingSessionWindows({ evaluate, screenshot, step }) {
  const CTRL = 2;
  const writeHost = () =>
    writeFileSync(
      join(workspace, "win-telnet.yaml"),
      `address: 127.0.0.1\nport: ${TELNET_PORT}\nprotocol: telnet\n`,
    );
  writeHost();
  const window = await moveToNewWindow(evaluate, "lab-r9");
  const there = window.evaluate;
  const openPalette = async () => {
    await pressIn(window.devtools, "k", "KeyK", 75, CTRL);
    await until("the palette in the session window", () =>
      there(`!!document.querySelector("input[name=palette-query]")`),
    );
  };

  // The palette connects a host into this window.
  await openPalette();
  await paletteChoose(there, "win-telnet", '[data-hit="win-telnet.yaml"]');
  await until("win-telnet connected in the session window", async () =>
    (await tabsIn(there)).some(
      (each) => each.name === "win-telnet" && each.state === "connected",
    ),
  );
  if ((await tabsIn(evaluate)).some((each) => each.name === "win-telnet"))
    fail("the session window's palette opened win-telnet in the main window");
  step(
    "Ctrl+K in a session window found win-telnet and connected it into that window",
  );

  // A snippet found there goes to that window's active terminal.
  const snippetsFile = join(workspace, "_snippets.yaml");
  const savedSnippets = existsSync(snippetsFile)
    ? readFileSync(snippetsFile, "utf8")
    : null;
  writeFileSync(snippetsFile, "Window check: later 0 window-snippet-ok\n");
  try {
    const lab = (await tabsIn(there)).find((each) => each.name === "lab-r9");
    await mouseClick(there, `[data-tab-bar] [data-tab="${lab.id}"]`);
    await until("lab-r9 active in its window", () =>
      there(
        `!!document.querySelector('.slot[data-tab="${lab.id}"][data-active]')`,
      ),
    );
    await openPalette();
    await paletteChoose(
      there,
      ">window check",
      '[data-snippet-hit="Window check"]',
    );
    await until("the snippet ran in lab-r9, in the session window", async () =>
      /(^|\n|\$ )window-snippet-ok\s*(\n|$)/.test(
        await screenOf(there, `.slot[data-tab="${lab.id}"]`),
      ),
    );
  } finally {
    if (savedSnippets === null) rmSync(snippetsFile, { force: true });
    else writeFileSync(snippetsFile, savedSnippets);
  }
  step(
    "a snippet found with > in the session window's palette ran in its active terminal",
  );

  // Closing asks; Cancel keeps the window and its sessions.
  const question = () =>
    there(
      `document.querySelector("[data-confirm-close-window]")?.textContent ?? null`,
    );
  await closeWithButton(there);
  const asked = await until("the session window's close question", question);
  if (!/lab-r9/.test(asked) || !/win-telnet/.test(asked))
    fail(`the close question names: ${asked}`);
  if (screenshot) await screenshot("session-window-close-question");
  await mouseClick(there, `[data-close-window=cancel]`);
  await until(
    "the close question gone",
    async () => (await question()) === null,
  );
  await sleep(500);
  if (!(await windowOpen(window.label)))
    fail("Cancel closed the session window");
  const kept = await tabsIn(there);
  if (
    !["lab-r9", "win-telnet"].every((name) =>
      kept.some((each) => each.name === name && each.state === "connected"),
    )
  )
    fail(`after Cancel the window shows ${JSON.stringify(kept)}`);
  step(
    "closing a session window with live sessions asked; Cancel kept it and both sessions",
  );

  // Dock back: both reach the main window, still connected.
  await closeWithButton(there);
  await until("the close question again", question);
  await mouseClick(there, `[data-close-window=dock]`, { mayClose: true });
  window.devtools.close();
  await until(
    "the session window closed",
    async () => !(await windowOpen(window.label)),
  );
  await until("both sessions docked back, connected", async () => {
    const shown = await tabsIn(evaluate);
    return ["lab-r9", "win-telnet"].every((name) =>
      shown.some((each) => each.name === name && each.state === "connected"),
    );
  });
  step(
    "Dock back closed the window and returned both sessions to the main window, connected",
  );

  // Close them: the window goes and its session ends.
  const second = await moveToNewWindow(evaluate, "win-telnet");
  await closeWithButton(second.evaluate);
  await until("the second window's close question", () =>
    second.evaluate(`!!document.querySelector("[data-confirm-close-window]")`),
  );
  await mouseClick(second.evaluate, `[data-close-window=close]`, {
    mayClose: true,
  });
  second.devtools.close();
  await until(
    "the second session window closed",
    async () => !(await windowOpen(second.label)),
  );
  await sleep(500);
  if ((await tabsIn(evaluate)).some((each) => each.name === "win-telnet"))
    fail("Close them docked win-telnet back");
  const live = await invokeIn(evaluate, "session_windows_live", {
    workspace: null,
  });
  if (live !== 0) fail(`after Close them, ${live} live in session windows`);
  step("Close them closed the session window and ended its session");
}

/**
 * A session window whose page crashes while the window stays open (#89):
 * after closingSessionWindows (lab-r9 connected in the main window).
 * lab-r9 is moved out, and `Page.crash` kills that page's renderer, twice:
 * quiet, then flooding. Each time the shell takes the session to the main
 * window, connected, saying its earlier output was lost (the flood
 * scrolls that line away: checked the first time), and closes the dead
 * window; the flood keeps flowing (flow control released).
 */
async function crashedSessionWindow({ evaluate, devtools, step }) {
  const CTRL = 2;
  const typeIn = async (client, text) => {
    for (const char of text) {
      const key =
        char === "\r"
          ? { key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 }
          : { key: char };
      await client.send("Input.dispatchKeyEvent", {
        type: "keyDown",
        text: char,
        ...key,
      });
      await client.send("Input.dispatchKeyEvent", { type: "keyUp", ...key });
    }
  };
  const focusShown = (target) =>
    target(
      `document.querySelector(${JSON.stringify(`${SHOWN_SESSION} .terminal textarea`)}).focus()`,
    );
  /** Every line of the terminal in `slot` of the main window. */
  const everyLine = (slot) =>
    evaluate(`(() => {
      const buffer = document.querySelector(${JSON.stringify(`${slot} .terminal`)}).xterm.buffer.normal;
      const lines = [];
      for (let y = 0; y < buffer.length; y++)
        lines.push(buffer.getLine(y)?.translateToString(true) ?? "");
      return lines;
    })()`);
  /**
   * Moves lab-r9 out, types `typed` there, waits for its screen to match
   * `shows`, crashes the window's page and resolves to the slot of lab-r9
   * back in the main window, shown, once the dead window has closed.
   */
  const crashWith = async (typed, shows) => {
    const known = new Set((await tabsIn(evaluate)).map((each) => each.id));
    const window = await moveToNewWindow(evaluate, "lab-r9");
    await focusShown(window.evaluate);
    await typeIn(window.devtools, typed);
    await until(`${shows} in the session window`, async () =>
      shows.test(await screenOf(window.evaluate, SHOWN_SESSION)),
    );
    window.devtools.send("Page.crash").catch(() => {});
    await until(
      "the crashed session window closed",
      async () => !(await windowOpen(window.label)),
    );
    window.devtools.close();
    const arrived = await until(
      "lab-r9 in the main window, connected",
      async () =>
        (await tabsIn(evaluate)).find(
          (each) =>
            !known.has(each.id) &&
            each.name === "lab-r9" &&
            each.state === "connected",
        ),
    ).catch(async (error) =>
      fail(
        `${error.message}: the main window shows ${JSON.stringify(await tabsIn(evaluate))}`,
      ),
    );
    const slot = `.slot[data-tab="${arrived.id}"]`;
    await mouseClick(evaluate, `[data-tab-bar] [data-tab="${arrived.id}"]`);
    await until("lab-r9 shown", () =>
      evaluate(`!!document.querySelector('${slot}[data-active]')`),
    );
    if ((await sessionWindowLabels()).length !== 0)
      fail(`a session window is left: ${await sessionWindowLabels()}`);
    if (
      (await invokeIn(evaluate, "session_windows_live", {
        workspace: null,
      })) !== 0
    )
      fail("the crashed window's session is still counted in a session window");
    return slot;
  };

  // A quiet session: its earlier output is lost, and it says so.
  // (Its output, not the command typed: all of it reached the page.)
  let slot = await crashWith(
    "later 0 before-crash\r",
    /(^|\n|\$ )before-crash\s*(\n|$)/,
  );
  const lines = await everyLine(slot);
  if (!lines.some((line) => line.includes("earlier output was lost")))
    fail(
      `the session taken from the crashed window did not say its output was lost: ${JSON.stringify(lines)}`,
    );
  if (lines.some((line) => line.includes("before-crash")))
    fail("the crashed window's output reached the main window");
  await focusShown(evaluate);
  await typeIn(devtools, "later 0 after-crash\r");
  await until("typing after the crash", async () =>
    /(^|\n|\$ )after-crash\s*(\n|$)/.test(await screenOf(evaluate, slot)),
  );
  step(
    "a crashed session window closed, its session reached the main window connected, saying earlier output was lost, and took typing",
  );

  // A flooding one: flow control was released. The flood goes on here,
  // and Ctrl+C's prompt comes after all of it (were flow control still
  // paused, it never would).
  slot = await crashWith("flood\r", /flood flood/);
  await until("the flood flowing in the main window", async () =>
    (await screenOf(evaluate, slot)).includes("flood flood"),
  );
  await focusShown(evaluate);
  await pressIn(devtools, "c", "KeyC", 67, CTRL);
  await until("the prompt after Ctrl+C", async () =>
    /\^C\s*\S*[$#>]\s*$/.test(await screenOf(evaluate, slot)),
  );
  step(
    "the session flooding when its window crashed kept flowing in the main window and stopped with Ctrl+C",
  );
}

/**
 * A session window whose page hangs without crashing (#120), after
 * crashedSessionWindow (lab-r9 in the main window, connected). Its page is
 * stopped on purpose (an endless loop run there);
 * it leaves the shell's question unanswered for 10 s (the core's
 * `PAGE_ANSWER_WAIT`, asked every 2 s), and the main window says so and
 * offers to bring its sessions back. First a page busy
 * for a moment only (2 s), which is not taken for hung; then one hung
 * that recovers (the offer withdrawn once it answers); then one that stays
 * hung, whose session Bring them here takes into the main window,
 * connected, saying earlier output was lost, and taking typing; its window
 * is closed. Nothing is evaluated in a hung page (it would never answer).
 */
/** When a stopped page is said (PAGE_ANSWER_WAIT plus one question's
 * period, PAGE_ASK_EVERY). */
const HUNG_AFTER_MS = 12_000;

async function hungSessionWindow({ evaluate, devtools, step }) {
  const json = (value) => JSON.stringify(value);
  const typeIn = async (client, text) => {
    for (const char of text) {
      const key =
        char === "\r"
          ? { key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 }
          : { key: char };
      await client.send("Input.dispatchKeyEvent", {
        type: "keyDown",
        text: char,
        ...key,
      });
      await client.send("Input.dispatchKeyEvent", { type: "keyUp", ...key });
    }
  };
  const focusShown = (target) =>
    target(
      `document.querySelector(${json(`${SHOWN_SESSION} .terminal textarea`)}).focus()`,
    );
  const notice = () =>
    evaluate(`(() => {
      const notice = document.querySelector("[data-not-answering]");
      return notice ? { window: Number(notice.dataset.notAnswering), text: notice.textContent } : null;
    })()`);
  /** Runs `loop` in the page of `window`, without waiting for it. */
  const stop = (window, loop) => {
    window.devtools
      .send("Runtime.evaluate", {
        expression: `setTimeout(() => { ${loop} }, 0); true`,
      })
      .catch(() => {});
  };
  const listed = () => invokeIn(evaluate, "session_windows_list");

  const known = new Set((await tabsIn(evaluate)).map((each) => each.id));
  const window = await moveToNewWindow(evaluate, "lab-r9");
  const id = Number(window.label.replace("session-", ""));
  await focusShown(window.evaluate);
  await typeIn(window.devtools, "later 0 before-hang\r");
  await until("before-hang in the session window", async () =>
    /(^|\n|\$ )before-hang\s*(\n|$)/.test(
      await screenOf(window.evaluate, SHOWN_SESSION),
    ),
  );

  // Busy for a moment: not taken for hung.
  stop(window, "const end = Date.now() + 2000; while (Date.now() < end) {}");
  await until(
    "the busy page answering again",
    () => window.evaluate("true"),
    20_000,
  );
  await sleep(HUNG_AFTER_MS + 2000);
  if (await notice())
    fail(`a page busy for 2 s was taken for hung: ${json(await notice())}`);
  step(
    "a session window's page busy for 2 s was not taken for one that does not answer",
  );

  // Hung, then recovered: the offer comes, and goes once it answers.
  stop(
    window,
    `const end = Date.now() + ${HUNG_AFTER_MS + 15_000}; while (Date.now() < end) {}`,
  );
  const offered = await until(
    "the main window saying the session window is not answering",
    notice,
    HUNG_AFTER_MS + 15_000,
  );
  if (offered.window !== id || !offered.text.includes("lab-r9"))
    fail(`the main window says ${json(offered)}`);
  await until(
    "the recovered page answering",
    () => window.evaluate("true"),
    60_000,
  );
  await until(
    "the offer withdrawn once the page answers",
    async () => !(await notice()),
  );
  await focusShown(window.evaluate);
  await typeIn(window.devtools, "later 0 recovered\r");
  await until("typing in the recovered window", async () =>
    /(^|\n|\$ )recovered\s*(\n|$)/.test(
      await screenOf(window.evaluate, SHOWN_SESSION),
    ),
  );
  step(
    "a session window's page that stopped answering was said in the main window, with its session's name; once it answered again the offer went and its session took typing there",
  );

  // Hung for good: its session comes back.
  stop(window, "for (;;) {}");
  await until(
    "the main window saying the session window is not answering",
    notice,
    HUNG_AFTER_MS + 15_000,
  );
  await mouseClick(evaluate, "[data-not-answering-action=take-back]");
  await until(
    "the hung window closed",
    async () => (await listed()).length === 0,
  );
  window.devtools.close();
  const arrived = await until(
    "lab-r9 in the main window, connected",
    async () =>
      (await tabsIn(evaluate)).find(
        (each) =>
          !known.has(each.id) &&
          each.name === "lab-r9" &&
          each.state === "connected",
      ),
  ).catch(async (error) =>
    fail(
      `${error.message}: the main window shows ${json(await tabsIn(evaluate))}`,
    ),
  );
  const slot = `.slot[data-tab="${arrived.id}"]`;
  await mouseClick(evaluate, `[data-tab-bar] [data-tab="${arrived.id}"]`);
  await until("lab-r9 shown", () =>
    evaluate(`!!document.querySelector('${slot}[data-active]')`),
  );
  if (await notice()) fail("the offer stayed after the sessions came back");
  const lines = await evaluate(`(() => {
    const buffer = document.querySelector(${json(`${slot} .terminal`)}).xterm.buffer.normal;
    const lines = [];
    for (let y = 0; y < buffer.length; y++)
      lines.push(buffer.getLine(y)?.translateToString(true) ?? "");
    return lines;
  })()`);
  if (!lines.some((line) => line.includes("earlier output was lost")))
    fail(
      `the session taken from the hung window did not say its output was lost: ${json(lines)}`,
    );
  if (
    (await invokeIn(evaluate, "session_windows_live", { workspace: null })) !==
    0
  )
    fail("the hung window's session is still counted in a session window");
  await focusShown(evaluate);
  await typeIn(devtools, "later 0 after-hang\r");
  await until("typing after the hang", async () =>
    /(^|\n|\$ )after-hang\s*(\n|$)/.test(await screenOf(evaluate, slot)),
  );
  step(
    "a session window's page hung for good: Bring them here took its session into the main window, connected, saying earlier output was lost, and taking typing; the hung window closed",
  );
}

/**
 * Closing the main window with a live session in a session window (#88),
 * at the end of the first run: asked once (Cancel keeps everything open),
 * then Close all ends the app and every window. The session is a telnet
 * host found with the palette (it asks for nothing).
 */
async function quittingWithLiveSessions({ app, evaluate, devtools, step }) {
  writeFileSync(
    join(workspace, "win-telnet.yaml"),
    `address: 127.0.0.1\nport: ${TELNET_PORT}\nprotocol: telnet\n`,
  );
  await pressIn(devtools, "k", "KeyK", 75, 2);
  await until("the palette", () =>
    evaluate(`!!document.querySelector("input[name=palette-query]")`),
  );
  await paletteChoose(evaluate, "win-telnet", '[data-hit="win-telnet.yaml"]');
  await until("win-telnet connected", async () =>
    (await tabsIn(evaluate)).some(
      (each) => each.name === "win-telnet" && each.state === "connected",
    ),
  );
  const window = await moveToNewWindow(evaluate, "win-telnet");
  window.devtools.close();
  // #124: a job that runs when the app closes (the jobs step's hang
  // snippet on its device, started over the shell's commands as the job
  // tab does): the question names it, and closing cancels it.
  const json = (value) => JSON.stringify(value);
  const snippetsFile = join(workspace, "_snippets.yaml");
  const savedSnippets = existsSync(snippetsFile)
    ? readFileSync(snippetsFile, "utf8")
    : null;
  writeFileSync(snippetsFile, `${savedSnippets ?? ""}Close check: hang\n`);
  // Planned once the app has read the snippet (a refusal is tried again).
  const planned = await until("the job to close on planned", async () => {
    const plan = await invokeIn(evaluate, "job_plan", {
      target: { kind: "hosts", ids: ["Jobs/jobs-r1.yaml"] },
      step: { kind: "snippet", name: "Close check", answers: {} },
    });
    return plan.hosts.length === 1 && plan;
  });
  // Start names its plan and the production hosts confirmed for it (#121).
  const started = await invokeIn(evaluate, "job_start", {
    plan: planned.id,
    confirmedProduction: planned.production,
    label: "jobs-r1",
  });
  const hangRunning = async () => {
    const live = (await invokeIn(evaluate, "jobs_live")).find(
      (job) => job.run === started.run,
    );
    if (live?.states?.["Jobs/jobs-r1.yaml"]?.kind === "running") return true;
    // Not running (yet): what it is, for the time-out's message.
    throw new Error(json(live));
  };
  await until("the job closing will cancel, running", hangRunning);
  const question = () =>
    evaluate(
      `document.querySelector("[data-confirm-quit]")?.textContent ?? null`,
    );
  await closeWithButton(evaluate);
  const asked = await until("the quit question", question);
  if (!/session window/.test(asked))
    fail(`the quit question does not count the session window's: ${asked}`);
  if (
    !/1 job is still running/.test(asked) ||
    !/cancels the job/.test(asked) ||
    !(await evaluate(`!!document.querySelector("[data-quit-jobs='1']")`))
  )
    fail(`the quit question does not name the running job: ${asked}`);
  await mouseClick(evaluate, `[data-quit=cancel]`);
  await until(
    "the quit question gone",
    async () => (await question()) === null,
  );
  await sleep(500);
  if (app.exitCode !== null) fail("Cancel ended the app");
  if (!(await windowOpen(window.label)))
    fail("Cancel closed the session window");
  await hangRunning().catch((error) =>
    fail(`Cancel on the quit question ended the job: ${error.message}`),
  );
  step(
    "closing the main window with a live session in a session window and a running job asked once, naming both; Cancel kept every window and the job",
  );

  await closeWithButton(evaluate);
  await until("the quit question again", question);
  await mouseClick(evaluate, `[data-quit=close]`, { mayClose: true });
  await until(
    "the app ended with every window",
    () => app.exitCode !== null,
    15_000,
  );
  step("Close all ended the app, the session window with it");
  if (savedSnippets === null) rmSync(snippetsFile, { force: true });
  else writeFileSync(snippetsFile, savedSnippets);
  return started.run;
}

/**
 * After the restart (#124): the run of the job the app was closed on
 * (`quittingWithLiveSessions`) is kept as ended, its host cancelled, and
 * its tab says the app was closed while it ran.
 */
async function jobCutByClosing({ evaluate, run, step }) {
  const json = (value) => JSON.stringify(value);
  await showSideView(evaluate, "jobs");
  const row = `${JOBS_SIDE} [data-run="${run}"]`;
  await until("the run cut by closing, listed", () =>
    evaluate(`!!document.querySelector(${json(row)})`),
  );
  const state = await evaluate(
    `document.querySelector(${json(row)}).dataset.runState`,
  );
  if (state !== "ended") fail(`the run cut by closing is ${state}`);
  await mouseClick(evaluate, row);
  const reason = await until("its reason shown", () =>
    evaluate(
      `document.querySelector(${json(`${RUN_TAB} [data-run-details="${run}"] [data-run-cancel-reason]`)})?.textContent.trim() ?? null`,
    ),
  );
  if (reason !== "Cancelled: the app was closed while the job ran.")
    fail(`the run cut by closing says: ${reason}`);
  if (
    !(await evaluate(
      `!!document.querySelector(${json(`${RUN_TAB} [data-run-host="Jobs/jobs-r1.yaml"][data-host-state=cancelled]`)})`,
    ))
  )
    fail("the run cut by closing does not show its host cancelled");
  step(
    "after the restart, the job the app was closed on is kept as cancelled because the app was closed",
  );
  // Left as the steps after it expect: no run tab, the Hosts view.
  await closeTabOf(evaluate, `${RUN_TAB} [data-run-details="${run}"]`);
  await showSideView(evaluate, "hosts");
}

/**
 * Jobs (#42): a snippet run on a folder whose platform is set there,
 * against the example server's device-mode shell. Pre-flight lists the
 * skipped hosts with their reasons and asks once for the production host;
 * an untrusted host key fails the host (a job never trusts a key); once the
 * key is trusted in a terminal, the job runs and shows the device's plain
 * output. The second run starts from the host's side panel, one host at a
 * time (#43: the frr definition turns paging off first). A third run of a
 * command that never returns is cancelled, keeping its output.
 */
async function jobs({
  evaluate,
  mouse,
  centreOf,
  click,
  fillForm,
  doubleClick,
  status,
  screen,
  text,
  type,
  screenshot,
  step,
}) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const dialog = JOB_TAB;
  const atStep = (name) => exists(`${dialog}[data-step=${name}]`);
  const profilesDialog = CREDENTIALS;
  const folder = join(workspace, "Jobs");
  const backupHostFile = join(folder, "jobs-b1.yaml");
  const renamedHostFile = join(folder, "jobs-b2.yaml");
  const snippetsFile = join(workspace, "_snippets.yaml");
  const savedSnippets = existsSync(snippetsFile)
    ? readFileSync(snippetsFile, "utf8")
    : null;

  // A profile for the job's hosts.
  await newProfile(evaluate);
  await fillForm(
    {
      "profile-name": "jobs-login",
      "profile-username": USERNAME,
      "profile-password": PASSWORD,
    },
    `${profilesDialog} form`,
  );
  await until("the jobs profile listed", () =>
    exists(`${profilesDialog}[data-mode=list] [data-profile="jobs-login"]`),
  );
  await showSideView(evaluate, "hosts");

  // The folder sets the platform (frr: its prompt matches `lab-r1#`), the
  // profile and the device's port for its hosts.
  mkdirSync(folder, { recursive: true });
  writeFileSync(
    join(folder, "_folder.yaml"),
    `platform: frr\ncredential_profile: jobs-login\nport: ${DEVICE_PORT}\n`,
  );
  writeFileSync(
    join(folder, "jobs-r1.yaml"),
    "address: 127.0.0.1\nenvironment: production\n",
  );
  writeFileSync(
    join(folder, "jobs-telnet.yaml"),
    "address: 127.0.0.1\nprotocol: telnet\n",
  );
  writeFileSync(
    join(folder, "jobs-odd.yaml"),
    "address: 127.0.0.1\nplatform: toaster\n",
  );
  writeFileSync(
    snippetsFile,
    `${savedSnippets ?? ""}Job check: "show hostname\\necho site {{site}}\\n"\nJob hang: hang\nJob configure: configure interface Loopback9\n`,
  );
  const folderRow = 'nav button[data-item=folder][data-path="Jobs"]';
  await until("the Jobs folder in the tree", () => exists(folderRow));
  await showSideView(evaluate, "snippets");
  await until("the job snippet loaded", () =>
    exists('[data-snippet="Job check"]'),
  );
  await showSideView(evaluate, "hosts");

  const runFromFolder = async () => {
    await clickOn(folderRow);
    await showDetails(evaluate);
    await until("the Jobs folder's side panel", () =>
      exists(".side [data-folder-run-snippet]"),
    );
    await clickOn(".side [data-folder-run-snippet]");
    await until("the run-snippet tab", () => atStep("snippet"));
  };
  const pickSnippet = (name) =>
    evaluate(`(() => {
      const select = document.querySelector("${dialog} [name=job-snippet]");
      select.value = ${json(name)};
      select.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
  /** Sets how many hosts run at once. */
  const setParallel = (value) =>
    evaluate(`(() => {
      const input = document.querySelector("${dialog} [name=job-parallel]");
      input.value = ${json(String(value))};
      input.dispatchEvent(new Event("input", { bubbles: true }));
    })()`);
  /**
   * Picks the snippet, answers the variable, and checks pre-flight: the
   * skipped hosts, and whether jobs-r1 is warned that its host key is not
   * trusted yet (#129).
   */
  const preflight = async (expectSkipped, expectUntrusted) => {
    await pickSnippet("Job check");
    await until("the snippet's text shown", async () =>
      (await text(`${dialog} [data-job-snippet-text]`)).includes(
        "echo site {{site}}",
      ),
    );
    await clickOn(`${dialog} [data-job=plan]`);
    await until("the variables step", () => atStep("variables"));
    const asked = await evaluate(
      `[...document.querySelectorAll("${dialog} [data-variable]")].map((i) => i.dataset.variable).join(",")`,
    );
    if (asked !== "site") fail(`the job asked for ${asked}, not site`);
    await fillForm({ "job-variable-0": "lab-site" }, `${dialog} form`);
    await until("the pre-flight list", () => atStep("preflight"));
    const plan = await evaluate(`(() => {
      const d = document.querySelector("${dialog}");
      return {
        runs: [...d.querySelectorAll("[data-job-host]")].map((r) => r.dataset.jobHost),
        skipped: Object.fromEntries([...d.querySelectorAll("[data-job-skipped]")].map(
          (r) => [r.dataset.jobSkipped, r.dataset.skipReason])),
        production: d.querySelector("[data-job-production-names]")?.textContent ?? "",
        startDisabled: d.querySelector("[data-job=start]").disabled,
        keyWarning: d.querySelector('[data-job-host="Jobs/jobs-r1.yaml"] [data-job-host-key]')?.dataset.jobHostKey ?? null,
        keyMessage: d.querySelector('[data-job-host="Jobs/jobs-r1.yaml"] [data-job-host-key]')?.textContent ?? "",
        untrusted: d.querySelector("[data-job-summary] [data-job-untrusted]")?.dataset.jobUntrusted ?? null,
      };
    })()`);
    if (json(plan.runs) !== json(["Jobs/jobs-r1.yaml"])) {
      fail(`the job would run on ${json(plan.runs)}`);
    }
    if (expectUntrusted) {
      if (
        plan.keyWarning !== "device" ||
        !plan.keyMessage.includes(`127.0.0.1 port ${DEVICE_PORT}`) ||
        !plan.keyMessage.includes("not trusted yet") ||
        !plan.keyMessage.includes("connecting to it once in a terminal") ||
        plan.untrusted !== "1"
      ) {
        fail(`pre-flight did not warn of the untrusted key: ${json(plan)}`);
      }
    } else if (plan.keyWarning !== null || plan.untrusted !== null) {
      fail(`pre-flight warns of a trusted key: ${json(plan)}`);
    }
    if (json(plan.skipped) !== json(expectSkipped)) {
      fail(`the job skips ${json(plan.skipped)}`);
    }
    if (plan.production !== "jobs-r1" || !plan.startDisabled) {
      fail(`the production host must be confirmed first: ${json(plan)}`);
    }
    const parallel = await evaluate(
      `document.querySelector("${dialog} [name=job-parallel]").value`,
    );
    if (parallel !== "10") fail(`hosts at once starts at ${parallel}, not 10`);
    await clickOn(`${dialog} [name=job-confirm-production]`);
    await until("the start button enabled", () =>
      evaluate(
        `!document.querySelector("${dialog} [data-job=start]").disabled`,
      ),
    );
  };
  const startDisabled = () =>
    evaluate(`document.querySelector("${dialog} [data-job=start]").disabled`);
  const finished = async () => {
    await until("the job finished", () =>
      exists(`${dialog} [data-job-progress][data-finished]`),
    );
    return evaluate(`(() => {
      const row = document.querySelector('${dialog} [data-job-result="Jobs/jobs-r1.yaml"]');
      return {
        state: row.dataset.hostState,
        reason: row.querySelector("[data-job-reason]")?.textContent ?? "",
        output: row.querySelector("[data-job-output]").textContent,
      };
    })()`);
  };

  try {
    await runFromFolder();
    await preflight(
      {
        "Jobs/jobs-odd.yaml": "noPlatform",
        "Jobs/jobs-telnet.yaml": "notSsh",
      },
      true,
    );
    await screenshot("job-preflight");
    step(
      "pre-flight lists the host that runs, the skipped ones with their reasons, and asks for production",
    );
    step(
      "pre-flight warns that the device's host key is not trusted yet and counts it in the summary (#129)",
    );

    // Its host key was never trusted: the job refuses it.
    await clickOn(`${dialog} [data-job=start]`);
    const refused = await finished();
    if (refused.state !== "failed" || !refused.reason.includes("not trusted")) {
      fail(`an untrusted key did not fail the host: ${json(refused)}`);
    }
    step("a host whose key is not trusted yet fails the job, naming the key");
    await clickOn(`${dialog} [data-job=close]`);
    await until("the job tab closed", async () => !(await exists(dialog)));

    // Trusted once in a terminal, the key lets the job in.
    await expandFolder(evaluate, folderRow);
    const hostRow = 'nav [data-host="Jobs/jobs-r1.yaml"]';
    await until("jobs-r1 in the tree", () => exists(hostRow));
    await doubleClick(hostRow);
    await until("the device's host-key question", () =>
      exists("[data-host-key=first-use] [data-decision=trustAndRemember]"),
    );
    await clickOn("[data-decision=trustAndRemember]");
    await until("the device's prompt", async () =>
      (await screen()).includes("lab-r1#"),
    );
    await sessionAction(evaluate, "disconnect");
    await until(
      "the device session closed",
      async () => (await status()) !== "Connected",
    );

    await clickOn(hostRow);
    await showDetails(evaluate);
    await until("jobs-r1's side panel", () =>
      exists(".side [data-host-run-snippet]"),
    );
    await clickOn(".side [data-host-run-snippet]");
    await until("the run-snippet tab for the host", () => atStep("snippet"));
    // Trusted now: no warning.
    await preflight({}, false);
    // Hosts at once: 1 to 50.
    for (const refused of ["0", "51"]) {
      await setParallel(refused);
      await until(`${refused} hosts at once refused`, startDisabled);
    }
    await setParallel(1);
    await until(
      "one host at once accepted",
      async () => !(await startDisabled()),
    );
    await clickOn(`${dialog} [data-job=start]`);
    const ran = await finished();
    const expected = [
      "lab-r1# terminal length 0",
      "lab-r1# show hostname",
      "lab-r1",
      "lab-r1# echo site lab-site",
      "site lab-site",
      "lab-r1#",
    ].join("\n");
    if (ran.state !== "done" || !ran.output.includes(expected)) {
      fail(`the job's result: ${json(ran)}`);
    }
    if (ran.output.includes("\u001b")) fail("the job's output has escapes");
    await screenshot("job-done");
    step(
      "the job turned paging off, logged in by itself and showed the device's plain output",
    );
    await clickOn(`${dialog} [data-job=close]`);
    await until("the job tab closed", async () => !(await exists(dialog)));

    // A command that never returns: cancelled, its output kept.
    await clickOn(".side [data-host-run-snippet]");
    await until("the run-snippet tab again", () => atStep("snippet"));
    await pickSnippet("Job hang");
    await until("the hang snippet shown", async () =>
      (await text(`${dialog} [data-job-snippet-text]`)).includes("hang"),
    );
    await clickOn(`${dialog} [data-job=plan]`);
    await until("the pre-flight list", () => atStep("preflight"));
    await clickOn(`${dialog} [name=job-confirm-production]`);
    await until(
      "the start button enabled",
      async () => !(await startDisabled()),
    );
    await clickOn(`${dialog} [data-job=start]`);
    const hostOutput = `${dialog} [data-job-output="Jobs/jobs-r1.yaml"]`;
    await until("the hang command sent", async () =>
      (await text(hostOutput)).includes("lab-r1# hang"),
    );
    // Its tab closed, the job goes on: a notice says so, the rail counts it.
    await clickOn(`${dialog} [data-job=close]`);
    await until("the job tab closed", async () => !(await exists(dialog)));
    await until("the notice that the job goes on", () =>
      exists("[data-job-still-running]"),
    );
    await until("the rail shows one job running", () =>
      exists('[data-rail=jobs][data-jobs-running="1"]'),
    );
    await screenshot("job-running-indicator");
    step(
      "closing a running job's tab leaves it running, says so, and the rail counts it",
    );
    await clickOn("[data-job-notice=dismiss]");

    // The Jobs side view: every run, the running one at the top.
    await showSideView(evaluate, "jobs");
    await until("the runs listed", () => exists(`${JOBS_SIDE} [data-runs]`));
    const listed = () =>
      evaluate(`[...document.querySelectorAll("${JOBS_SIDE} [data-run]")].map((r) => ({
        id: r.dataset.run,
        state: r.dataset.runState,
        title: r.querySelector(".title").textContent,
      }))`);
    await until("three runs listed", async () => (await listed()).length === 3);
    const runs = await listed();
    if (
      json(runs.map((r) => [r.title, r.state])) !==
      json([
        ["Job hang", "running"],
        ["Job check", "ended"],
        ["Job check", "ended"],
      ])
    ) {
      fail(`the runs listed: ${json(runs)}`);
    }
    const [hangRun, doneRun, refusedRun] = runs.map((r) => r.id);
    const view = RUN_TAB;
    const runRow = (id) => `${JOBS_SIDE} [data-run="${id}"]`;
    const hostState = (state) =>
      `${view} [data-run-host="Jobs/jobs-r1.yaml"][data-host-state=${state}]`;
    const runOutputText = () => text(`${view} [data-run-output]`);

    // Reopened from there, in its run's tab, still running.
    await clickOn(runRow(hangRun));
    await until("the running run's tab", () =>
      exists(`${view} [data-run-details="${hangRun}"]`),
    );
    await until("its host still running", () => exists(hostState("running")));

    // #77: the run in a window of its own, and back.
    await detachedRunWindow({ evaluate, exists, clickOn, run: hangRun, step });
    await until("the run's tab docked back", () =>
      exists(`${view} [data-run-details="${hangRun}"]`),
    );
    await until("its host still running, docked back", () =>
      exists(hostState("running")),
    );
    const runTab = await evaluate(
      `document.querySelector('${view} [data-run-details="${hangRun}"]').closest(".slot").dataset.tab`,
    );

    // A terminal to the device, and the run's tab in a pane of its own
    // beside it: the job's progress shows there while the user types.
    await showSideView(evaluate, "hosts");
    // The new session's tab, not one shown before it (an earlier terminal
    // to the same device also shows its prompt: taking that one, which
    // the new connection then replaced, failed the run now and then).
    const tabsBefore = await evaluate(
      `[...document.querySelectorAll("[data-tab-bar] [data-tab]")].map((t) => t.dataset.tab)`,
    );
    await doubleClick(hostRow);
    const sessionTab = await until("a new terminal on jobs-r1", async () => {
      const shown = await evaluate(`(() => {
        const slot = document.querySelector("${SHOWN_SESSION}");
        return slot && document.querySelector('[data-tab-bar] [data-tab="' + slot.dataset.tab + '"]')
          ? slot.dataset.tab : null;
      })()`);
      return shown !== null &&
        !tabsBefore.includes(shown) &&
        (await screen()).includes("lab-r1#")
        ? shown
        : null;
    });
    const tabBarOf = (tab) =>
      `document.querySelector('[data-tab-bar] [data-tab="${tab}"]').closest("[data-tab-bar]")`;
    await until(
      `the run's tab ${runTab} in a tab bar (tabs: ${await evaluate(`[...document.querySelectorAll("[data-tab-bar] [data-tab]")].map((t) => t.dataset.tab).join(",")`)})`,
      () => exists(`[data-tab-bar] [data-tab="${runTab}"]`),
    );
    await mouseClick(evaluate, `[data-tab-bar] [data-tab="${runTab}"]`);
    await until("the run's tab shown again", () =>
      exists(`${view} [data-run-details="${hangRun}"]`),
    );
    await paneTool(
      evaluate,
      tabBarOf(runTab),
      "split-right",
      "the run tab's pane",
    );
    await until("the run's tab in a pane of its own", () =>
      evaluate(
        `${tabBarOf(runTab)}.querySelectorAll("[data-tab]").length === 1`,
      ),
    );
    await mouseClick(evaluate, `[data-tab-bar] [data-tab="${sessionTab}"]`);
    await until("the terminal shown beside the run's tab", () =>
      evaluate(`(() => {
        const run = document.querySelector('.slot[data-tab="${runTab}"]');
        const session = document.querySelector('.slot[data-tab="${sessionTab}"]');
        return !run.hidden && session.matches("[data-active]") &&
          run.dataset.pane !== session.dataset.pane;
      })()`),
    );
    await type("echo beside\r");
    await until("the terminal answering beside the run's tab", async () =>
      /echo beside\s+beside\s+lab-r1#/.test(await screen()),
    );
    // Cancelled from the run's own window (opened beside the tab, #77),
    // the tab beside and that window show it at once; the terminal still
    // takes the keys.
    await showSideView(evaluate, "jobs");
    // Its runs are listed a moment after the view shows (in Windows Sandbox,
    // #139, the check made at once failed).
    await until("the Jobs view's cancel for the running job", () =>
      exists(`${JOBS_SIDE} [data-run-cancel="${hangRun}"]`),
    ).catch(() => fail("the Jobs view offers no cancel for the running job"));
    await evaluate(
      `window.__TAURI_INTERNALS__.invoke("window_detach_run", { run: ${json(hangRun)} })`,
    );
    const other = await connectWindow(`run-${hangRun}`);
    const inWindow = (selector) =>
      other.evaluate(`!!document.querySelector(${json(selector)})`);
    const windowHost = (state) =>
      `[data-detached-run="${hangRun}"] [data-run-host="Jobs/jobs-r1.yaml"][data-host-state=${state}]`;
    await until("the run's window, running", () =>
      inWindow(windowHost("running")),
    );
    // The host reads "running" from the kept run at once; Cancel is offered
    // only once the window has the app's record of live jobs, a moment later.
    await until("the run's window offers Cancel", () =>
      inWindow("[data-run-action=cancel]"),
    );
    await mouseClick(other.evaluate, `[data-run-action=cancel]`);
    await until("the cancelled host, in its window", () =>
      inWindow(windowHost("cancelled")),
    );
    await until("the cancelled host, in the tab beside", () =>
      exists(hostState("cancelled")),
    );
    await other
      .evaluate(
        `window.__TAURI_INTERNALS__.invoke("plugin:window|close", { label: ${json(`run-${hangRun}`)} })`,
      )
      .catch(() => {});
    other.devtools.close();
    await until(
      "the run's window closed",
      async () => !(await windowOpen(`run-${hangRun}`)),
    );
    await until("the hang run ended", () =>
      exists(`${runRow(hangRun)}[data-run-state=ended]`),
    );
    await until("the cancelled host's output kept", async () =>
      (await runOutputText()).includes("lab-r1# hang"),
    );
    await until("the rail shows no job running", () =>
      exists('[data-rail=jobs][data-jobs-running="0"]'),
    );
    await type("echo still\r");
    await until("the terminal still answering", async () =>
      /echo still\s+still\s+lab-r1#/.test(await screen()),
    );
    await screenshot("job-run-beside-terminal");
    step(
      "the Jobs view lists the runs, running first; a run's tab in its own pane kept updating beside a terminal in use, and the job was cancelled from the run's own window, keeping its output",
    );
    await sessionAction(evaluate, "disconnect");
    await until(
      "the terminal beside closed",
      async () => (await status()) !== "Connected",
    );
    await closeTabOf(evaluate, `.slot[data-tab="${runTab}"]`);

    // The finished run: its output, and search across it.
    await clickOn(runRow(doneRun));
    await until("the done run's host", () => exists(hostState("done")));
    await until("the done run's output", async () =>
      (await runOutputText()).includes("site lab-site"),
    );
    await fillForm({ "run-search": "SITE LAB" }, `${view} [data-run-search]`);
    await until("the search hit", () =>
      exists(`${view} [data-search-hit="Jobs/jobs-r1.yaml"][data-count="2"]`),
    );
    await fillForm(
      { "run-search": "nowhere at all" },
      `${view} [data-run-search]`,
    );
    await until("no search hit", () => exists(`${view} [data-search-none]`));
    await screenshot("job-results");
    step("a kept run shows each host's output, and search finds the host");

    // The same host in two runs, compared line by line.
    await evaluate(`(() => {
      const select = document.querySelector("${view} [name=compare-new]");
      select.value = ${json(`${refusedRun}|Jobs/jobs-r1.yaml`)};
      select.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
    await until(
      "the compare button enabled",
      async () =>
        !(await evaluate(
          `document.querySelector("${view} [data-run-action=compare]").disabled`,
        )),
    );
    await clickOn(`${view} [data-run-action=compare]`);
    await until("the diff", () => exists(`${view} [data-diff]`));
    const diff = await evaluate(`(() => {
      const lines = [...document.querySelectorAll("${view} [data-diff-line]")];
      return {
        removed: lines.filter((l) => l.dataset.diffLine === "removed").map((l) => l.querySelector(".text").textContent),
        added: lines.filter((l) => l.dataset.diffLine === "added").length,
      };
    })()`);
    if (!diff.removed.includes("site lab-site") || diff.added !== 0) {
      fail(`the diff with the refused run: ${json(diff)}`);
    }
    step("one host's output from two runs compared as a line diff");

    // Saved to a folder: the description and one text file per host.
    const saveTo = mkdtempSync(join(tmpdir(), "netterm-job-export-"));
    try {
      await fillForm(
        { "run-export-folder": saveTo },
        `${view} [data-run-export]`,
      );
      await until("the run saved", () => exists(`${view} [data-run-notice]`));
      const saved = join(saveTo, doneRun);
      const hostFile = join(saved, "output", "Jobs", "jobs-r1.txt");
      if (
        !existsSync(join(saved, "run.yaml")) ||
        !existsSync(hostFile) ||
        !readFileSync(hostFile, "utf8").includes("site lab-site")
      ) {
        fail(`the saved run in ${saved} is not complete`);
      }
    } finally {
      rmSync(saveTo, { recursive: true, force: true });
    }
    step("Save to… copied the run to the chosen folder");

    // Deleted: gone from the list and from disk.
    const keptRuns = join(appData, "jobs");
    const runFolder = (id) =>
      readdirSync(keptRuns)
        .map((workspaceId) => join(keptRuns, workspaceId, id))
        .find((folder) => existsSync(folder));
    if (!runFolder(refusedRun)) fail("the refused run is not on disk");
    await clickOn(runRow(refusedRun));
    await until("the refused run selected", () =>
      exists(`${view} [data-run-details="${refusedRun}"]`),
    );
    await clickOn(`${view} [data-run-action=delete]`);
    await clickOn(`${view} [data-run-action=confirm-delete]`);
    await until(
      "the refused run gone from the list",
      async () => !(await exists(runRow(refusedRun))),
    );
    if (runFolder(refusedRun)) fail("the deleted run is still on disk");
    step("delete removed the run from the list and from disk");

    // Run again: the same snippet and answers on the same host, planned
    // anew, at pre-flight.
    await clickOn(runRow(doneRun));
    await until("the done run selected", () =>
      exists(`${view} [data-run-details="${doneRun}"]`),
    );
    await clickOn(`${view} [data-run-action=again]`);
    await until("the run-again pre-flight", () => atStep("preflight"));
    const again = await evaluate(
      `[...document.querySelectorAll("${dialog} [data-job-host]")].map((r) => r.dataset.jobHost)`,
    );
    if (json(again) !== json(["Jobs/jobs-r1.yaml"])) {
      fail(`run again would run on ${json(again)}`);
    }
    if (!(await exists(`${dialog} [data-job-production]`))) {
      fail("run again did not ask about the production host");
    }
    step("Run again planned the same step and target and showed pre-flight");
    await clickOn(`${dialog} [data-job=dismiss]`);
    await until("the job tab closed", async () => !(await exists(dialog)));
    await closeTabOf(evaluate, `.slot[data-tab] [data-run-tab="${doneRun}"]`);
    await showSideView(evaluate, "hosts");

    // Config backup: a host driven by eos (which ignores the device's
    // changing `! Time:` line), backed up twice for one version, then
    // again after its configuration changed for a second one and a diff.
    writeFileSync(backupHostFile, "address: 127.0.0.1\nplatform: eos\n");
    const backupHost = "Jobs/jobs-b1.yaml";
    const backupRow = `nav [data-host="${backupHost}"]`;
    await until("jobs-b1 in the tree", () => exists(backupRow));
    await clickOn(backupRow);
    await showDetails(evaluate);
    await until("jobs-b1's side panel", () =>
      exists(".side [data-host-back-up]"),
    );
    const backUp = async () => {
      await clickOn(".side [data-host-back-up]");
      await until("the backup pre-flight", async () =>
        exists(
          `${dialog}[data-step=preflight] [data-job-host="${backupHost}"]`,
        ),
      );
      if (!(await exists(`${dialog} [data-job-backup-note]`))) {
        fail("the backup pre-flight does not say how sensitive backups are");
      }
      await until(
        "the backup's start button enabled",
        async () => !(await startDisabled()),
      );
      await clickOn(`${dialog} [data-job=start]`);
      await until("the backup finished", () =>
        exists(`${dialog} [data-job-progress][data-finished]`),
      );
      const result = await evaluate(`(() => {
        const row = document.querySelector('${dialog} [data-job-result="${backupHost}"]');
        const backup = row.querySelector("[data-job-backup]");
        return {
          state: row.dataset.hostState,
          reason: row.querySelector("[data-job-reason]")?.textContent ?? "",
          backup: backup?.dataset.jobBackup ?? null,
          version: backup?.dataset.version ?? null,
        };
      })()`);
      await clickOn(`${dialog} [data-job=close]`);
      await until("the job tab closed", async () => !(await exists(dialog)));
      if (result.state !== "done") fail(`the backup failed: ${json(result)}`);
      return result;
    };
    // The Config backups side view (#70): what each backed-up host's last
    // backup did; the Hosts view and the host's details come back after.
    const backupsView = "[data-side-view=backups] [data-backups-view]";
    const backupsEntry = async (expected) => {
      await showSideView(evaluate, "backups");
      const entry = await until(
        `jobs-b1 in the backups view (${expected})`,
        () =>
          evaluate(`(() => {
          const row = document.querySelector('${backupsView} [data-backup-host="${backupHost}"]');
          return row?.dataset.last === ${json(expected)} &&
            { last: row.dataset.last, versions: Number(row.dataset.versions), newest: row.dataset.newest };
        })()`),
      );
      return entry;
    };
    const backToHost = async () => {
      await showSideView(evaluate, "hosts");
      await clickOn(backupRow);
      await showDetails(evaluate);
      await until("jobs-b1's side panel", () =>
        exists(".side [data-host-back-up]"),
      );
    };
    if (!existsSync(join(appData, "configs"))) {
      await showSideView(evaluate, "backups");
      await until("the empty backups view", () =>
        exists(`${backupsView} [data-backups-empty]`),
      );
      if (!(await exists(`${backupsView} [data-backups-back-up]`))) {
        fail("the empty backups view offers no backup of a folder");
      }
      // It offers a folder's backup: the Jobs folder, at its pre-flight.
      await evaluate(`(() => {
        const select = document.querySelector("${backupsView} [name=backups-folder]");
        select.value = "Jobs";
        select.dispatchEvent(new Event("change", { bubbles: true }));
      })()`);
      await clickOn(`${backupsView} [data-backups-back-up]`);
      await until("the folder backup's pre-flight", () =>
        exists(
          `${dialog}[data-step=preflight] [data-job-host="${backupHost}"]`,
        ),
      );
      await clickOn(`${dialog} [data-job=dismiss]`);
      await until("the job tab closed", async () => !(await exists(dialog)));
      step("the empty backups view says how backups are made and offers one");
      await backToHost();
    }
    const first = await backUp();
    if (first.backup !== "saved") fail(`the first backup: ${json(first)}`);
    const firstEntry = await backupsEntry("first");
    if (firstEntry.versions !== 1 || firstEntry.newest !== first.version) {
      fail(`the backups view after the first backup: ${json(firstEntry)}`);
    }
    await backToHost();
    const second = await backUp();
    if (second.backup !== "unchanged" || second.version !== first.version) {
      fail(`a second backup of an unchanged device: ${json(second)}`);
    }
    // The view refreshes: the last backup changed nothing.
    const secondEntry = await backupsEntry("unchanged");
    if (secondEntry.versions !== 1) {
      fail(`the backups view after an unchanged backup: ${json(secondEntry)}`);
    }
    await screenshot("backups-view");
    await backToHost();
    step("the backups view lists the host with what its last backup did");
    // The versions kept (not the view's note of an unchanged backup).
    const kept = () => {
      const root = join(appData, "configs");
      if (!existsSync(root)) return [];
      return readdirSync(root).flatMap((id) => {
        const folder = join(root, id, "Jobs", "jobs-b1");
        return existsSync(folder)
          ? readdirSync(folder).filter((name) => name.endsWith(".txt"))
          : [];
      });
    };
    if (kept().length !== 1) fail(`versions on disk: ${json(kept())}`);
    await screenshot("config-backup");
    step(
      "two backups of an unchanged device kept one version, outside the workspace",
    );

    // The device's configuration changes (a snippet job), and the next
    // backup keeps a second version.
    await clickOn(".side [data-host-run-snippet]");
    await until("the run-snippet tab on jobs-b1", () => atStep("snippet"));
    await pickSnippet("Job configure");
    await until("the configure snippet shown", async () =>
      (await text(`${dialog} [data-job-snippet-text]`)).includes(
        "configure interface",
      ),
    );
    await clickOn(`${dialog} [data-job=plan]`);
    await until("the configure pre-flight", () => atStep("preflight"));
    await clickOn(`${dialog} [data-job=start]`);
    await until("the configure job finished", () =>
      exists(`${dialog} [data-job-progress][data-finished]`),
    );
    if (
      !(await exists(
        `${dialog} [data-job-result="${backupHost}"][data-host-state=done]`,
      ))
    ) {
      fail("the configure snippet did not run");
    }
    await clickOn(`${dialog} [data-job=close]`);
    await until("the job tab closed", async () => !(await exists(dialog)));
    const third = await backUp();
    if (third.backup !== "saved" || third.version === first.version) {
      fail(`a backup of a changed device: ${json(third)}`);
    }
    if (kept().length !== 2) fail(`versions on disk: ${json(kept())}`);
    step("a changed configuration was kept as a second version");

    // The history, in its tab from the backups view (now "changed"): both
    // versions, the text without the prompt, a diff, and Save to….
    const thirdEntry = await backupsEntry("changed");
    if (thirdEntry.versions !== 2 || thirdEntry.newest !== third.version) {
      fail(`the backups view after a changed backup: ${json(thirdEntry)}`);
    }
    // (The host is checked apart: selectors here go inside quoted strings.)
    const history = ".slot.view:not([hidden]) [data-view=config-history]";
    await clickOn(`${backupsView} [data-backup-host="${backupHost}"]`);
    await until("the config history tab", () =>
      exists(`${history} [data-config-version]`),
    );
    if (await exists("[data-dialog=config-history]")) {
      fail("the config history is still a dialog");
    }
    if (
      (await evaluate(
        `document.querySelector(${json(history)}).dataset.host`,
      )) !== backupHost
    ) {
      fail("the config history tab shows another host");
    }
    if (!(await exists(`${history} [data-config-sensitive]`))) {
      fail("the config history does not say how sensitive backups are");
    }
    const versions = await evaluate(
      `[...document.querySelectorAll("${history} [data-config-version]")].map((b) => b.dataset.configVersion)`,
    );
    if (json(versions) !== json([third.version, first.version])) {
      fail(`the versions listed: ${json(versions)}`);
    }
    await until("the newest version's text", async () =>
      (await text(`${history} [data-config-text]`)).includes(
        "interface Loopback9",
      ),
    );
    const shown = await text(`${history} [data-config-text]`);
    // No prompt, so no echo line either (`lab-r1# show running-config`).
    if (shown.includes("lab-r1#") || !shown.startsWith("! Command:")) {
      fail(`the version holds more than the configuration: ${json(shown)}`);
    }
    await clickOn(`${history} [data-config-action=diff]`);
    await until("the versions' diff", () => exists(`${history} [data-diff]`));
    const added = await evaluate(
      `[...document.querySelectorAll('${history} [data-diff-line="added"] .text')].map((l) => l.textContent)`,
    );
    if (!added.includes("interface Loopback9")) {
      fail(`the diff of the two versions adds ${json(added)}`);
    }
    await screenshot("config-history");
    step("the host's config history lists both versions and diffs them");

    const saveConfigTo = mkdtempSync(join(tmpdir(), "netterm-config-export-"));
    try {
      await fillForm(
        { "config-export-folder": saveConfigTo },
        `${history} [data-config-export]`,
      );
      await until("the version saved", () =>
        exists(`${history} [data-config-notice]`),
      );
      const savedFile = join(saveConfigTo, `jobs-b1_${third.version}.txt`);
      if (
        !existsSync(savedFile) ||
        !readFileSync(savedFile, "utf8").includes("interface Loopback9")
      ) {
        fail(`the saved version ${savedFile} is not there`);
      }
    } finally {
      rmSync(saveConfigTo, { recursive: true, force: true });
    }
    step("Save to… wrote the version's text to the chosen folder");
    // From the details and the context menu: the same tab, never a second.
    const historyTabs = () =>
      evaluate(
        `document.querySelectorAll('.slot.view [data-view=config-history][data-host="${backupHost}"]').length`,
      );
    await backToHost();
    await clickOn(".side [data-open-config-history]");
    await until("the history tab from the details", () => exists(history));
    await mouseClick(evaluate, backupRow, { button: "right" });
    await until("the host's context menu", () =>
      exists("[role=menu] [data-menu=config-history]"),
    );
    await clickOn("[role=menu] [data-menu=config-history]");
    await until("the history tab from the context menu", () => exists(history));
    if ((await historyTabs()) !== 1) {
      fail(`the host has ${await historyTabs()} config history tabs`);
    }
    step("the host's history opens in one tab from the view, details and menu");

    // In a window of its own (#78): the tab closes; the window has the
    // versions, the text, a diff, copy, save and back up now, and stays
    // open while a backup on login keeps a new version below.
    const configLabel = `config-${Buffer.from(backupHost, "utf8").toString("hex")}`;
    const inConfigWindow = "[data-detached=configs] [data-view=config-history]";
    await clickOn(`${history} [data-config-action=detach]`);
    await until(
      "the history's tab closed",
      async () => !(await exists(history)),
    );
    let configWindow = await connectWindow(configLabel);
    const inside = (selector) =>
      configWindow.evaluate(
        `!!document.querySelector(${json(`${inConfigWindow} ${selector}`)})`,
      );
    const windowVersions = () =>
      configWindow.evaluate(
        `[...document.querySelectorAll(${json(`${inConfigWindow} [data-config-version]`)})].map((b) => b.dataset.configVersion)`,
      );
    await until("the history in its window", async () =>
      (
        await configWindow.evaluate(
          `document.querySelector(${json(`${inConfigWindow} [data-config-text]`)})?.textContent ?? ""`,
        )
      ).includes("interface Loopback9"),
    );
    if (json(await windowVersions()) !== json(versions)) {
      fail(`the history's window lists ${json(await windowVersions())}`);
    }
    await checkContrast(configWindow.evaluate, "config history window");
    if (
      (await configWindow.evaluate(
        `document.querySelector("[data-detached=configs]").dataset.detachedHost`,
      )) !== backupHost
    ) {
      fail("the history's window shows another host");
    }
    for (const selector of [
      "[data-config-action=copy]:not([disabled])",
      "[data-config-export]",
      "[data-config-action=back-up]",
      "[data-config-action=dock]",
      "[data-config-sensitive]",
    ]) {
      if (!(await inside(selector)))
        fail(`the history's window lacks ${selector}`);
    }
    for (const selector of [
      "[data-rail]",
      ".terminal",
      "[data-config-action=detach]",
    ]) {
      if (
        await configWindow.evaluate(
          `!!document.querySelector(${json(selector)})`,
        )
      )
        fail(`the history's window shows ${selector}`);
    }
    await mouseClick(
      configWindow.evaluate,
      `${inConfigWindow} [data-config-action=diff]`,
    );
    await until("the diff in the history's window", async () =>
      (
        await configWindow.evaluate(
          `[...document.querySelectorAll('${inConfigWindow} [data-diff-line="added"] .text')].map((l) => l.textContent)`,
        )
      ).includes("interface Loopback9"),
    );
    await screenshot("config-history-window");
    step(
      "the config history opened in a window of its own (its tab closed) with versions, text, diff, copy, save and back up now",
    );
    // Back up now there opens the backup's tab in the main window.
    await mouseClick(
      configWindow.evaluate,
      `${inConfigWindow} [data-config-action=back-up]`,
    );
    await until("the backup's pre-flight in the main window", () =>
      exists(`${dialog}[data-step=preflight] [data-job-host="${backupHost}"]`),
    );
    await clickOn(`${dialog} [data-job=dismiss]`);
    await until("the job tab closed", async () => !(await exists(dialog)));
    step(
      "Back up now in the history's window opened the backup in the main window",
    );
    await clickOn(backupRow);
    await showDetails(evaluate);

    // Back up on login: on for the host, each connection checks the
    // configuration over a second channel, showing nothing in the terminal.
    await evaluate(`(() => {
      const select = document.querySelector(".side [name=setting-backup_on_login]");
      select.value = "on";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
    await until("backup on login set on the host", () =>
      readFileSync(backupHostFile, "utf8").includes("backup_on_login: true"),
    );
    if (!(await exists('.side [data-setting-note="backup_on_login"]'))) {
      fail("the setting does not say where it does not apply");
    }
    const loginBackup = `${SHOWN_SESSION} [data-login-backup]`;
    const backupMarker = () =>
      evaluate(`(() => {
        const marker = document.querySelector(${json(loginBackup)});
        return marker
          ? { kind: marker.dataset.loginBackup, version: marker.dataset.version ?? null, title: marker.title }
          : null;
      })()`);
    await doubleClick(backupRow);
    await until("jobs-b1's prompt in a terminal", async () =>
      (await screen()).includes("lab-r1#"),
    );
    await until(
      "the backup on login",
      async () => (await backupMarker()) !== null,
    );
    const unchanged = await backupMarker();
    if (unchanged.kind !== "unchanged" || unchanged.version !== third.version) {
      fail(`the backup on login of an unchanged device: ${json(unchanged)}`);
    }
    await checkHeaderLine(evaluate, SHOWN_SESSION, [
      "[data-login-backup=unchanged]",
    ]);
    if ((await screen()).includes("running-config")) {
      fail("the backup on login showed in the terminal");
    }
    // Changed in the terminal; the next connection keeps a new version.
    await type("configure interface Loopback10\r");
    await until("the configure line answered", async () =>
      (await screen()).match(/Loopback10\s+lab-r1#/),
    );
    await type("exit\r");
    await until("the session ended", () =>
      exists(`${SHOWN_SESSION} [data-session-ended]`),
    );
    await type("\r");
    await until("a new backup on login after reconnecting", async () => {
      const marker = await backupMarker();
      return marker?.kind === "saved" && marker.version !== third.version;
    });
    if (kept().length !== 3) fail(`versions on disk: ${json(kept())}`);
    if ((await screen()).includes("running-config")) {
      fail("the backup on login showed in the terminal");
    }
    if (await exists(`${SHOWN_SESSION} [data-login-backup-failed]`)) {
      fail("a successful backup on login showed a failure notice");
    }
    await screenshot("backup-on-login");
    step(
      "back up on login: unchanged on connecting, a new version after a change and a reconnect, nothing in the terminal",
    );
    // The history's window refreshed: the new version, shown first.
    const loginVersion = (await backupMarker()).version;
    await until("the history's window listing the new version", async () => {
      const listed = await windowVersions();
      return listed.length === 3 && listed[0] === loginVersion;
    });
    await until("the new version's text in the window", () =>
      inside(`[data-config-text="${loginVersion}"]`),
    );
    step(
      "the history's window refreshed when the backup on login kept a version",
    );
    // A backup on login that fails says why in a quiet bar (#127: untested
    // until then): the host's history folder is a file for a moment.
    const historyFolder = readdirSync(join(appData, "configs"))
      .map((id) => join(appData, "configs", id, "Jobs", "jobs-b1"))
      .find((folder) => existsSync(folder));
    if (!historyFolder) fail("jobs-b1 has no history folder");
    const failedBar = `${SHOWN_SESSION} [data-login-backup-failed]`;
    renameSync(historyFolder, `${historyFolder}.aside`);
    writeFileSync(historyFolder, "not a folder\n");
    try {
      await type("exit\r");
      await until("the session ended", () =>
        exists(`${SHOWN_SESSION} [data-session-ended]`),
      );
      await type("\r");
      await until(
        "the failed backup on login",
        async () => (await backupMarker())?.kind === "failed",
      );
      await until("the failure's bar", () => exists(failedBar));
      const said = await evaluate(
        `document.querySelector(${json(`${failedBar} .reason`)})?.textContent ?? ""`,
      );
      if (!said.startsWith("The backup on login failed: ") || said.length < 40)
        fail(`the failure's bar says ${json(said)}`);
      await screenshot("backup-on-login-failed");
      await clickOn(`${failedBar} [data-login-backup-dismiss]`);
      await until(
        "the failure's bar dismissed",
        async () => !(await exists(failedBar)),
      );
    } finally {
      rmSync(historyFolder, { force: true });
      renameSync(`${historyFolder}.aside`, historyFolder);
    }
    step(
      "a backup on login that failed said why in a bar, which Dismiss closed",
    );
    await sessionAction(evaluate, "disconnect");
    await until(
      "the backed-up session closed",
      async () => (await status()) !== "Connected",
    );
    // Closing the history's window, then opening it again and docking it
    // back: the history is a tab in the main window again.
    await configWindow
      .evaluate(
        `window.__TAURI_INTERNALS__.invoke("plugin:window|close", { label: ${json(configLabel)} })`,
      )
      .catch(() => {});
    configWindow.devtools.close();
    await until(
      "the history's window closed",
      async () => !(await windowOpen(configLabel)),
    );
    if (await exists(history)) fail("closing the window opened a tab");
    await evaluate(
      `window.__TAURI_INTERNALS__.invoke("window_detach_configs", { host: ${json(backupHost)} })`,
    );
    configWindow = await connectWindow(configLabel);
    await until("the history's window again", () =>
      inside(`[data-config-version="${loginVersion}"]`),
    );
    await mouseClick(
      configWindow.evaluate,
      `${inConfigWindow} [data-config-action=dock]`,
      { mayClose: true },
    );
    configWindow.devtools.close();
    await until("the history docked back as a tab", () =>
      exists(`${history} [data-config-version="${loginVersion}"]`),
    );
    await until(
      "the history's window gone",
      async () => !(await windowOpen(configLabel)),
    );
    step(
      "closing the history's window closed only it; Dock back opened the history as a tab again",
    );

    // Renamed (#99): its session logs and its config versions follow it
    // to the new path, and its open history tab shows them under it.
    const logsOf = (name) =>
      join(appData, "logs", "workspaces", basename(workspace), "Jobs", name);
    const logsBefore = existsSync(logsOf("jobs-b1"))
      ? readdirSync(logsOf("jobs-b1"))
      : [];
    if (logsBefore.length === 0) fail("jobs-b1 has no session log");
    const versionsOf = (name) => {
      const root = join(appData, "configs");
      return readdirSync(root).flatMap((id) => {
        const folder = join(root, id, "Jobs", name);
        return existsSync(folder)
          ? readdirSync(folder).filter((file) => file.endsWith(".txt"))
          : [];
      });
    };
    const versionsBefore = versionsOf("jobs-b1");
    await backToHost();
    await evaluate(`(() => {
      const input = document.querySelector(".side [name=name]");
      input.value = "jobs-b2";
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
    await until(
      "the renamed host file",
      () => existsSync(renamedHostFile) && !existsSync(backupHostFile),
    );
    await until("the history tab following the host", () =>
      exists(
        `${history}[data-host="Jobs/jobs-b2.yaml"] [data-config-version="${loginVersion}"]`,
      ),
    );
    if (json(versionsOf("jobs-b2")) !== json(versionsBefore)) {
      fail(`versions after the rename: ${json(versionsOf("jobs-b2"))}`);
    }
    if (versionsOf("jobs-b1").length !== 0) {
      fail("versions were left under the old name");
    }
    const logsAfter = existsSync(logsOf("jobs-b2"))
      ? readdirSync(logsOf("jobs-b2"))
      : [];
    if (json(logsAfter) !== json(logsBefore) || existsSync(logsOf("jobs-b1"))) {
      fail(`logs after the rename: ${json(logsAfter)}`);
    }
    if (await exists("[data-move-report]")) {
      fail("the rename reported history left behind");
    }
    await showSideView(evaluate, "backups");
    await until("the renamed host in the backups view", () =>
      exists(
        `${backupsView} [data-backup-host="Jobs/jobs-b2.yaml"][data-versions="${versionsBefore.length}"]`,
      ),
    );
    await screenshot("renamed-host-history");
    step(
      "renaming a host kept its session logs and config versions, and its history tab followed it",
    );
    await closeTabOf(evaluate, history);
  } finally {
    rmSync(backupHostFile, { force: true });
    rmSync(renamedHostFile, { force: true });
    if (savedSnippets === null) rmSync(snippetsFile, { force: true });
    else writeFileSync(snippetsFile, savedSnippets);
  }
}

/**
 * The Files tab (#103) against the test SSH server's SFTP folder: a typed
 * login and an unknown host key asked over the tab (mouse and keyboard,
 * Cancel saying why), a stored profile listing the platform's file folder
 * without a question, one tab per host from the details sheet, the tree's
 * context menu, a session's menu and its tab's menu, and from a session
 * window; not offered for telnet, serial or quick connections; the path
 * box, a folder clicked, Up and Refresh; rename, delete, new folder and a
 * rename onto an existing name; production's colour, mark and questions
 * (Cancel focused); a cut connection with its reason and Reconnect; closing
 * the tab closing its connection (the server counts them), the terminal to
 * the same host unaffected.
 */
async function filesTab({
  evaluate,
  devtools,
  fillForm,
  doubleClick,
  screen,
  type,
  screenshot,
  step,
}) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const press = async (key, code, windowsVirtualKeyCode) => {
    const event = { key, code, windowsVirtualKeyCode };
    await devtools.send("Input.dispatchKeyEvent", {
      type: "rawKeyDown",
      ...event,
    });
    await devtools.send("Input.dispatchKeyEvent", { type: "keyUp", ...event });
  };
  const folder = join(workspace, "Files");
  const platformsFile = join(workspace, "_platforms.yaml");
  const typedFile = join(workspace, "files-typed.yaml");
  const knownHosts = join(appData, "known_hosts");
  const savedKnownHosts = existsSync(knownHosts)
    ? readFileSync(knownHosts, "utf8")
    : null;
  const savedPlatforms = existsSync(platformsFile)
    ? readFileSync(platformsFile, "utf8")
    : null;
  const r1 = "Files/files-r1.yaml";
  const prod = "Files/files-prod.yaml";
  const typed = "files-typed.yaml";
  const tabOf = (host) => `.slot.view [data-view=files][data-host="${host}"]`;
  const row = (host) => `nav [data-host="${host}"]`;
  const local = (path) => join(sftpFolder, path);
  const filesState = (host) =>
    evaluate(
      `document.querySelector(${json(tabOf(host))})?.dataset.filesState ?? null`,
    );
  const shownFolder = (host) =>
    evaluate(
      `document.querySelector(${json(`${tabOf(host)} [data-files-folder]`)})?.dataset.filesFolder ?? null`,
    );
  const names = (host) =>
    evaluate(
      `[...document.querySelectorAll(${json(`${tabOf(host)} [data-files-entry]`)})].map((r) => r.dataset.filesEntry)`,
    );
  const reason = (host) =>
    evaluate(
      `document.querySelector(${json(`${tabOf(host)} [data-files-reason]`)})?.textContent ?? ""`,
    );
  const filesTabs = (host) =>
    evaluate(`document.querySelectorAll(${json(tabOf(host))}).length`);
  const listed = async (host, where) => {
    try {
      await until(
        `${host}'s files listed in ${where}`,
        async () =>
          (await filesState(host)) === "connected" &&
          (await shownFolder(host)) === where,
      );
    } catch (error) {
      const seen = await evaluate(`(() => {
        const tab = document.querySelector(${json(tabOf(host))});
        return tab && {
          state: tab.dataset.filesState,
          folder: tab.querySelector("[data-files-folder]")?.dataset.filesFolder,
          text: tab.textContent.slice(0, 300),
          focused: document.activeElement?.outerHTML.slice(0, 120),
        };
      })()`);
      fail(`${error.message}; the tab shows ${json(seen)}`);
    }
  };
  const contextMenu = async (host) => {
    await mouseClick(evaluate, row(host), { button: "right" });
    await until(`${host}'s context menu`, () =>
      exists("[role=menu] [data-menu=details]"),
    );
  };
  const closeMenu = async () => {
    await press("Escape", "Escape", 27);
    await until(
      "the menu closed",
      async () =>
        !(await exists("[role=menu]")) &&
        !(await exists("[data-session-actions]")),
    );
  };
  // Types the login over the tab with the keyboard: the username field
  // has the focus, Tab goes to the password, Enter sends.
  const typeLogin = async (host) => {
    const form = `${tabOf(host)} [data-login-prompt]`;
    await until("the login asked over the tab", async () =>
      evaluate(
        `document.activeElement === document.querySelector(${json(`${form} input[name=answer-0]`)})`,
      ),
    );
    await type(USERNAME);
    await press("Tab", "Tab", 9);
    await until("the password field focused", () =>
      evaluate(
        `document.activeElement === document.querySelector(${json(`${form} input[name=answer-1]`)})`,
      ),
    );
    await type(`${PASSWORD}\r`);
  };
  const closeFilesTab = (host) => closeTabOf(evaluate, tabOf(host));

  rmSync(sftpFolder, { recursive: true, force: true });
  mkdirSync(local("flash"), { recursive: true });
  mkdirSync(local("logs"), { recursive: true });
  mkdirSync(local("Zeta"), { recursive: true });
  mkdirSync(local("empty"), { recursive: true });
  writeFileSync(local("flash/boot.bin"), "boot");
  writeFileSync(local("logs/old.log"), "old");
  writeFileSync(local("notes.txt"), "hostname r1\n");
  writeFileSync(local(".hidden"), "x");
  writeFileSync(local("b.cfg"), "b");

  try {
    await newProfile(evaluate);
    await fillForm(
      {
        "profile-name": "files-login",
        "profile-username": USERNAME,
        "profile-password": PASSWORD,
      },
      `${CREDENTIALS} form`,
    );
    await until("the files profile listed", () =>
      exists(`${CREDENTIALS}[data-mode=list] [data-profile="files-login"]`),
    );
    await showSideView(evaluate, "hosts");
    // The platform puts files in /flash; a typed login at the top.
    writeFileSync(
      platformsFile,
      `${savedPlatforms ?? ""}smoke-files:\n  prompt: 'test-ssh\\$ '\n  file_folder: /flash\n`,
    );
    mkdirSync(folder, { recursive: true });
    writeFileSync(
      join(folder, "_folder.yaml"),
      `credential_profile: files-login\nport: ${SSH_PORT}\n`,
    );
    writeFileSync(
      join(folder, "files-r1.yaml"),
      "address: 127.0.0.1\nplatform: smoke-files\n",
    );
    writeFileSync(
      join(folder, "files-prod.yaml"),
      "address: 127.0.0.1\nenvironment: production\n",
    );
    writeFileSync(
      join(folder, "files-telnet.yaml"),
      `address: 127.0.0.1\nport: ${TELNET_PORT}\nprotocol: telnet\n`,
    );
    writeFileSync(
      join(folder, "files-serial.yaml"),
      "address: 192.0.2.51\nprotocol: serial\nserial:\n  port: COM250\n",
    );
    writeFileSync(typedFile, `address: 127.0.0.1\nport: ${SSH_PORT}\n`);
    const folderRow = 'nav button[data-item=folder][data-path="Files"]';
    await until("the Files folder in the tree", () => exists(folderRow));
    await expandFolder(evaluate, folderRow);
    await until(
      "the files hosts in the tree",
      async () => (await exists(row(r1))) && (await exists(row(typed))),
    );

    // A typed login and an unknown host key, over the tab. The key is
    // unknown: the trusted keys start empty (and are put back after).
    writeFileSync(knownHosts, "");
    await contextMenu(typed);
    await until("Files in the context menu", () =>
      exists("[role=menu] [data-menu=files]"),
    );
    await clickOn("[role=menu] [data-menu=files]");
    await until("the login asked over the Files tab", () =>
      exists(`${tabOf(typed)} [data-files-login] [data-login-prompt]`),
    );
    await clickOn(
      `${tabOf(typed)} [data-login-prompt] .actions button[type=button]`,
    );
    await until(
      "the cancelled login said",
      async () =>
        (await filesState(typed)) === "failed" &&
        (await reason(typed)).includes("no login was given"),
    );
    await clickOn(`${tabOf(typed)} [data-files-action=reconnect]`);
    await typeLogin(typed);
    await until("the unknown key asked over the tab", () =>
      exists(
        `${tabOf(typed)} [data-host-key=first-use] [data-decision=cancel]:not(:disabled)`,
      ),
    );
    await clickOn(`${tabOf(typed)} [data-decision=cancel]`);
    await until(
      "the refused key said",
      async () =>
        (await filesState(typed)) === "failed" &&
        (await reason(typed)).includes("host key"),
    );
    await screenshot("files-not-connected");
    await clickOn(`${tabOf(typed)} [data-files-action=reconnect]`);
    await typeLogin(typed);
    // Trust and remember has the focus: Enter takes it.
    await until("the key asked again, focused", () =>
      evaluate(
        `document.activeElement === document.querySelector(${json(`${tabOf(typed)} [data-decision=trustAndRemember]`)})`,
      ),
    );
    // A key with its character, as typing sends it: a button acts on that.
    await type(String.fromCharCode(13));
    await listed(typed, "/");
    const expected = [
      "empty",
      "flash",
      "logs",
      "Zeta",
      ".hidden",
      "b.cfg",
      "notes.txt",
    ];
    if (json(await names(typed)) !== json(expected))
      fail(`the listing is ${json(await names(typed))}`);
    const cells = await evaluate(
      `[...document.querySelectorAll(${json(`${tabOf(typed)} [data-files-entry="notes.txt"] td`)})].map((cell) => cell.textContent.trim())`,
    );
    // The first cell holds the file's Download tick box (#106).
    if (cells[2] !== "12 B" || !cells[3] || !/^[rwx-]{9}$/.test(cells[4] ?? ""))
      fail(`notes.txt shows ${json(cells)}`);
    if (
      (await evaluate(
        `document.querySelector("[data-tab-bar] [data-tab-view=files] [data-tab-name]")?.textContent`,
      )) !== "Files files-typed"
    )
      fail("the Files tab is not named after its host");
    await screenshot("files");
    step(
      "a typed login and an unknown host key are asked over the Files tab (mouse and keyboard; Cancel says why); folders first, hidden files, size, time and permissions",
    );
    await closeFilesTab(typed);

    // A stored profile: no question; the platform's file folder.
    await clickOn(row(r1));
    await showDetails(evaluate);
    await until("Files in the details", () =>
      exists(".side [data-open-files]"),
    );
    await clickOn(".side [data-open-files]");
    await listed(r1, "/flash");
    if (json(await names(r1)) !== json(["boot.bin"]))
      fail(`/flash lists ${json(await names(r1))}`);
    if (
      await exists(
        `${tabOf(r1)} [data-host-key], ${tabOf(r1)} [data-login-prompt]`,
      )
    )
      fail("a stored profile asked a question");
    await contextMenu(r1);
    await until("Files in the context menu", () =>
      exists("[role=menu] [data-menu=files]"),
    );
    await clickOn("[role=menu] [data-menu=files]");
    await until("the same tab shown", () =>
      exists(
        `.slot.view[data-view-active] [data-view=files][data-host="${r1}"]`,
      ),
    );
    if ((await filesTabs(r1)) !== 1)
      fail(`files-r1 has ${await filesTabs(r1)} Files tabs`);
    step(
      "with a stored profile the Files tab lists the platform's file folder without a question; the details and the context menu show one tab per host",
    );

    // The path box, a folder clicked, Up and Refresh; a missing path.
    const bar = `${tabOf(r1)} form`;
    await fillForm({ "files-path": "/logs" }, bar);
    await listed(r1, "/logs");
    if (json(await names(r1)) !== json(["old.log"]))
      fail(`/logs lists ${json(await names(r1))}`);
    await clickOn(`${tabOf(r1)} [data-files-action=up]`);
    await listed(r1, "/");
    await clickOn(`${tabOf(r1)} [data-files-open="logs"]`);
    await listed(r1, "/logs");
    await clickOn(`${tabOf(r1)} [data-files-action=up]`);
    await listed(r1, "/");
    writeFileSync(local("fresh.txt"), "fresh");
    await clickOn(`${tabOf(r1)} [data-files-action=refresh]`);
    await until("fresh.txt after Refresh", async () =>
      (await names(r1)).includes("fresh.txt"),
    );
    await fillForm({ "files-path": "/missing" }, bar);
    await until("the missing path said", async () =>
      (
        await evaluate(
          `document.querySelector(${json(`${tabOf(r1)} [data-files-error]`)})?.textContent ?? ""`,
        )
      ).includes("no such file"),
    );
    if (
      (await shownFolder(r1)) !== "/" ||
      !(await names(r1)).includes("fresh.txt")
    )
      fail("a missing path moved the listing");
    step(
      "the path box, a folder, Up and Refresh list; a missing path says so and the listing stays",
    );

    // Rename, a rename onto an existing name, new folder, delete.
    const entryAction = (host, name, action) =>
      `${tabOf(host)} [data-files-entry="${name}"] [data-files-entry-action=${action}]`;
    const nameForm = (host) => `${tabOf(host)} [data-files-name-form]`;
    await clickOn(entryAction(r1, "notes.txt", "rename"));
    await until("the rename form", () => exists(nameForm(r1)));
    await fillForm({ "files-name": "renamed.txt" }, nameForm(r1));
    await until("renamed.txt listed", async () =>
      (await names(r1)).includes("renamed.txt"),
    );
    if (!existsSync(local("renamed.txt")) || existsSync(local("notes.txt")))
      fail("the rename did not reach the device");
    await clickOn(entryAction(r1, "renamed.txt", "rename"));
    await until("the rename form", () => exists(nameForm(r1)));
    await fillForm({ "files-name": "b.cfg" }, nameForm(r1));
    await until("the existing name said", async () =>
      (
        await evaluate(
          `document.querySelector(${json(`${tabOf(r1)} [data-files-error]`)})?.textContent ?? ""`,
        )
      ).includes("exists already"),
    );
    if (
      !existsSync(local("renamed.txt")) ||
      readFileSync(local("b.cfg"), "utf8") !== "b"
    )
      fail("a rename onto an existing name changed something");
    await clickOn(`${tabOf(r1)} [data-files-action=new-folder]`);
    await until("the new folder form", () => exists(nameForm(r1)));
    await fillForm({ "files-name": "made" }, nameForm(r1));
    await until("made listed", async () => (await names(r1)).includes("made"));
    await clickOn(entryAction(r1, "fresh.txt", "delete"));
    await until(
      "fresh.txt deleted",
      async () => !(await names(r1)).includes("fresh.txt"),
    );
    await clickOn(entryAction(r1, "made", "delete"));
    await until(
      "made deleted",
      async () => !(await names(r1)).includes("made"),
    );
    if (existsSync(local("fresh.txt")) || existsSync(local("made")))
      fail("the deletes did not reach the device");
    step(
      "rename, new folder and delete work on the device; a rename onto an existing name shows the core's message",
    );

    // Production: its colour and mark, and each change asks first, Cancel
    // focused.
    await contextMenu(prod);
    await until("Files in the context menu", () =>
      exists("[role=menu] [data-menu=files]"),
    );
    await clickOn("[role=menu] [data-menu=files]");
    await listed(prod, "/");
    const prodTab = await evaluate(
      `document.querySelector(${json(tabOf(prod))}).closest(".slot").dataset.tab`,
    );
    if (
      !(await exists(
        `[data-tab-bar] [data-tab="${prodTab}"][data-environment=production] [data-tab-production]`,
      ))
    )
      fail("the production host's Files tab has no production colour and mark");
    const confirm = `${tabOf(prod)} [data-files-confirm]`;
    const cancelFocused = () =>
      until("Cancel focused", () =>
        evaluate(
          `document.activeElement === document.querySelector(${json(`${confirm} [data-files-confirm-action=cancel]`)})`,
        ),
      );
    await clickOn(entryAction(prod, "b.cfg", "delete"));
    await cancelFocused();
    await screenshot("files-production");
    await clickOn(`${confirm} [data-files-confirm-action=cancel]`);
    await until("the question gone", async () => !(await exists(confirm)));
    if (!existsSync(local("b.cfg"))) fail("Cancel deleted b.cfg");
    await clickOn(`${tabOf(prod)} [data-files-action=new-folder]`);
    await until("the new folder form", () => exists(nameForm(prod)));
    await fillForm({ "files-name": "prod-made" }, nameForm(prod));
    await cancelFocused();
    await clickOn(`${confirm} [data-files-confirm-action=cancel]`);
    await clickOn(entryAction(prod, "renamed.txt", "rename"));
    await until("the rename form", () => exists(nameForm(prod)));
    await fillForm({ "files-name": "prod.txt" }, nameForm(prod));
    await cancelFocused();
    await clickOn(`${confirm} [data-files-confirm-action=confirm]`);
    await until("prod.txt listed", async () =>
      (await names(prod)).includes("prod.txt"),
    );
    await clickOn(entryAction(prod, "b.cfg", "delete"));
    await cancelFocused();
    await clickOn(`${confirm} [data-files-confirm-action=confirm]`);
    await until(
      "b.cfg deleted",
      async () => !(await names(prod)).includes("b.cfg"),
    );
    if (
      existsSync(local("prod-made")) ||
      existsSync(local("b.cfg")) ||
      !existsSync(local("prod.txt"))
    )
      fail("production's answers were not followed");
    step(
      "a production host's Files tab has its colour and mark; rename, delete and new folder ask first with Cancel focused",
    );

    // A session's menu and its tab's menu; a cut connection; closing.
    await closeFilesTab(r1);
    await doubleClick(row(r1));
    await until("files-r1's terminal", async () =>
      (await screen()).includes("test-ssh$"),
    );
    const sessionTab = await evaluate(
      `document.querySelector("${SHOWN_SESSION}").dataset.tab`,
    );
    await sessionAction(evaluate, "files");
    await listed(r1, "/flash");
    await mouseClick(evaluate, `[data-tab-bar] [data-tab="${sessionTab}"]`, {
      button: "right",
    });
    await until("the tab's menu", () => exists("[data-session-actions]"));
    if (!(await menuEntries(evaluate)).includes("files"))
      fail("the session tab's menu offers no Files");
    await clickOn("[data-session-actions] [data-session-action=files]");
    await until(
      "the menu closed",
      async () => !(await exists("[data-session-actions]")),
    );
    if ((await filesTabs(r1)) !== 1)
      fail(`files-r1 has ${await filesTabs(r1)} Files tabs`);
    step("a session's menu and its tab's menu open its host's one Files tab");

    const terminal = async (command, expect) => {
      await clickOn(`[data-tab-bar] [data-tab="${sessionTab}"]`);
      await until("the terminal shown", () =>
        exists(`.slot[data-tab="${sessionTab}"][data-active]`),
      );
      await type(`${command}\r`);
      await until(`${expect} in the terminal`, async () =>
        (await screen()).includes(expect),
      );
    };
    await terminal("show sftp", "sftp connections: 2");
    await terminal("cut sftp", "cut 2 sftp connections");
    for (const host of [r1, prod]) {
      await until(
        `${host}'s lost connection said`,
        async () =>
          (await filesState(host)) === "lost" &&
          (await reason(host)).includes("going down"),
      );
    }
    if (
      !(await exists(
        `[data-tab-bar] [data-tab="${sessionTab}"][data-tab-state=connected]`,
      ))
    )
      fail("cutting the files connections cut the terminal");
    const r1Tab = await evaluate(
      `document.querySelector(${json(tabOf(r1))}).closest(".slot").dataset.tab`,
    );
    await clickOn(`[data-tab-bar] [data-tab="${r1Tab}"]`);
    // The tab is shown before its button is clicked.
    await until("files-r1's tab shown", () =>
      exists(`.slot[data-tab="${r1Tab}"]:not([hidden])`),
    );
    await clickOn(`${tabOf(r1)} [data-files-action=reconnect]`);
    await listed(r1, "/flash");
    step(
      "a cut connection says so with its reason; Reconnect lists again; the terminal stays connected",
    );

    await terminal("show sftp", "sftp connections: 1");
    await closeFilesTab(prod);
    await closeFilesTab(r1);
    await terminal("show sftp", "sftp connections: 0");
    if (
      !(await exists(
        `[data-tab-bar] [data-tab="${sessionTab}"][data-tab-state=connected]`,
      ))
    )
      fail("closing the Files tabs touched the terminal");
    step(
      "closing a Files tab closes its connection (the server sees it go); the terminal is unaffected",
    );

    // From a session window: the tab opens in the main window.
    const window = await moveToNewWindow(evaluate, "files-r1");
    await sessionAction(window.evaluate, "files", SHOWN_SESSION);
    await listed(r1, "/flash");
    if (await window.evaluate(`!!document.querySelector("[data-view=files]")`))
      fail("the Files tab opened in the session window");
    await sessionAction(window.evaluate, "dock", SHOWN_SESSION);
    window.devtools.close();
    await until(
      `${window.label} closed`,
      async () => !(await windowOpen(window.label)),
    );
    await closeFilesTab(r1);
    step("from a session window the Files tab opens in the main window");

    // Not offered: telnet and serial hosts, quick connections.
    for (const host of ["Files/files-telnet.yaml", "Files/files-serial.yaml"]) {
      await contextMenu(host);
      // The host's settings are resolved meanwhile.
      await sleep(500);
      if (await exists("[role=menu] [data-menu=files]"))
        fail(`${host}'s context menu offers Files`);
      await closeMenu();
      await clickOn(row(host));
      await showDetails(evaluate);
      await sleep(500);
      if (await exists(".side [data-open-files]"))
        fail(`${host}'s details offer Files`);
    }
    // The active pane's new-tab button (folded into "…" when narrow).
    await paneTool(
      evaluate,
      `document.querySelector('[data-tab-bar] [data-tab="' + document.querySelector("${SHOWN_SESSION}").dataset.tab + '"]').closest("[data-tab-bar]")`,
      "new-tab",
      "the terminal's pane",
    );
    // Its own form: the details sheet left open has forms too.
    await until("the quick-connect form", () =>
      exists("form input[name=host]"),
    );
    await fillForm(
      {
        host: "127.0.0.1",
        port: String(SSH_PORT),
        username: USERNAME,
        password: PASSWORD,
      },
      "form:has(input[name=host])",
    );
    await until("the quick connection", async () =>
      (await screen()).includes("test-ssh$"),
    );
    await mouseClick(evaluate, `${SHOWN_SESSION} [data-session-menu]`);
    await until("the session's menu", () => exists("[data-session-actions]"));
    if ((await menuEntries(evaluate)).includes("files"))
      fail("a quick connection offers Files");
    await closeMenu();
    const quickTab = await evaluate(
      `document.querySelector("${SHOWN_SESSION}").dataset.tab`,
    );
    // files-r1's terminal came back from its window in a new tab.
    const docked = (await tabsIn(evaluate)).find(
      (each) => each.name === "files-r1",
    );
    if (!docked) fail("files-r1's terminal did not come back");
    for (const tab of [quickTab, docked.id]) {
      await clickOn(`[data-tab-bar] [data-tab="${tab}"] [data-tab-close]`);
      await until("the close question", () => exists("[data-confirm=close]"));
      await clickOn("[data-confirm=close]");
      await until(
        "the session's tab closed",
        async () => !(await exists(`.slot[data-tab="${tab}"]`)),
      );
    }
    step(
      "Files is not offered for telnet or serial hosts, nor for a quick connection",
    );
  } finally {
    if (savedKnownHosts === null) rmSync(knownHosts, { force: true });
    else writeFileSync(knownHosts, savedKnownHosts);
    if (savedPlatforms === null) rmSync(platformsFile, { force: true });
    else writeFileSync(platformsFile, savedPlatforms);
    rmSync(folder, { recursive: true, force: true });
    rmSync(typedFile, { force: true });
  }
}

/**
 * The Files tab's transfers (#106), after `filesTab` (its `files-login`
 * profile stays): uploads by the Upload button and by files dropped on the
 * tab (sent as the event Tauri delivers; a drop elsewhere does nothing)
 * into the folder shown, the listing refreshed; the question over files of
 * the same name (one for several), Cancel and Replace; downloads of ticked
 * files into a folder picked, remembered for the next, asking over a local
 * file; a production host's question with Cancel focused; the transfer
 * list (progress, speed, waiting order, Remove, Cancel with the partial
 * file's fate, MD5 and SHA-256 copied); a transfer going on while the
 * terminal is shown; closing the tab and the main window while one runs
 * asking first, Cancel keeping everything; and a device with SCP only: the
 * form, sent to and fetched from typed paths, a drop filling the file, the
 * line that an existing file is replaced, a refusal with its hint, and a
 * typed login whose one-time code each send asks again over the form
 * (#109), Cancel failing that send alone.
 *
 * The system's file and folder dialogs are answered in the page, on the
 * IPC protocol (DevTools cannot click them); the buttons that open them
 * are clicked.
 * The test shell's `hold sftp` keeps a transfer running.
 */
async function fileTransfers({
  evaluate,
  fillForm,
  doubleClick,
  screen,
  type,
  screenshot,
  step,
}) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const folder = join(workspace, "Transfers");
  const platformsFile = join(workspace, "_platforms.yaml");
  const knownHosts = join(appData, "known_hosts");
  const savedKnownHosts = existsSync(knownHosts)
    ? readFileSync(knownHosts, "utf8")
    : null;
  const savedPlatforms = existsSync(platformsFile)
    ? readFileSync(platformsFile, "utf8")
    : null;
  const r1 = "Transfers/xfer-r1.yaml";
  const prod = "Transfers/xfer-prod.yaml";
  const scp = "Transfers/xfer-scp.yaml";
  // At the top, out of the folder's profile: a typed login.
  const otp = "xfer-otp.yaml";
  const otpFile = join(workspace, otp);
  const tabOf = (host) => `.slot.view [data-view=files][data-host="${host}"]`;
  const row = (host) => `nav [data-host="${host}"]`;
  const localFolder = join(userData, "transfers-local");
  const into = join(userData, "transfers-downloads");
  const fetched = join(userData, "transfers-fetched");
  const localFile = (name) => join(localFolder, name);
  const remote = (path) => join(sftpFolder, path);
  const md5 = (path) =>
    createHash("md5").update(readFileSync(path)).digest("hex");
  const sha256 = (path) =>
    createHash("sha256").update(readFileSync(path)).digest("hex");
  const filesState = (host) =>
    evaluate(
      `document.querySelector(${json(tabOf(host))})?.dataset.filesState ?? null`,
    );
  const names = (host) =>
    evaluate(
      `[...document.querySelectorAll(${json(`${tabOf(host)} [data-files-entry]`)})].map((r) => r.dataset.filesEntry)`,
    );
  /** The transfer rows of `host`'s tab, in order. */
  const transfers = (host) =>
    evaluate(`[...document.querySelectorAll(${json(`${tabOf(host)} [data-transfer]`)})].map((r) => ({
      id: r.dataset.transfer,
      name: r.dataset.transferName,
      direction: r.dataset.direction,
      state: r.dataset.transferState,
      text: r.textContent.replace(/\\s+/g, " ").trim(),
    }))`);
  const transferAt = (host, index) => async () =>
    (await transfers(host))[index] ?? null;
  const ended = async (host, index, state) => {
    const seen = await until(
      `transfer ${index} of ${host} ${state}`,
      async () => {
        const each = await transferAt(host, index)();
        return each &&
          !["waiting", "connecting", "running"].includes(each.state)
          ? each
          : null;
      },
    );
    if (seen.state !== state)
      fail(`transfer ${index} of ${host} ended ${seen.state}: ${seen.text}`);
    return seen;
  };
  /** The selector of `host`'s transfer at `index`, and `inner` in it. */
  const transferSelector = async (host, index, inner = "") => {
    const each = (await transfers(host))[index];
    if (!each) fail(`${host} has no transfer ${index}`);
    return `${tabOf(host)} [data-transfer="${each.id}"] ${inner}`.trim();
  };
  /** Answers the next system file dialogs with `answers`, in order, and
   * keeps what each was asked with. Tauri's own `invoke` cannot be
   * replaced, so its request on the IPC protocol is answered (the page's
   * `fetch`, looked up at each call). */
  const answerDialogs = (answers) =>
    evaluate(`(() => {
      if (!window.__smokeFetch) {
        window.__smokeFetch = window.fetch;
        window.fetch = async (url, init) => {
          if (String(url).includes(encodeURIComponent("plugin:dialog|open"))) {
            try {
              window.__smokeDialogs.push(JSON.parse(init.body).options ?? null);
            } catch {
              window.__smokeDialogs.push(null);
            }
            return new Response(JSON.stringify(window.__smokeAnswers.shift() ?? null), {
              headers: { "Tauri-Response": "ok", "Content-Type": "application/json" },
            });
          }
          return window.__smokeFetch(url, init);
        };
      }
      window.__smokeAnswers = ${json(answers)};
      window.__smokeDialogs = [];
      return window.fetch !== window.__smokeFetch;
    })()`);
  const dialogsAsked = () => evaluate(`window.__smokeDialogs`);
  /** Files dropped from Explorer on the middle of `selector`, as Tauri
   * delivers it: drag-enter, drag-over and drag-drop, in physical pixels. */
  const drop = async (selector, paths) => {
    const position = await evaluate(`(() => {
      const box = document.querySelector(${json(selector)}).getBoundingClientRect();
      const ratio = window.devicePixelRatio || 1;
      return { x: (box.left + box.width / 2) * ratio, y: (box.top + box.height / 2) * ratio };
    })()`);
    for (const [event, payload] of [
      ["tauri://drag-enter", { paths, position }],
      ["tauri://drag-over", { position }],
      ["tauri://drag-drop", { paths, position }],
    ])
      await evaluate(
        `window.__TAURI_INTERNALS__.invoke("plugin:event|emit_to", { target: { kind: "Webview", label: "main" }, event: ${json(event)}, payload: ${json(payload)} })`,
      );
  };
  const question = (host) => `${tabOf(host)} [data-files-transfer-confirm]`;
  const focused = (selector) =>
    evaluate(
      `document.activeElement === document.querySelector(${json(selector)})`,
    );
  const openFiles = async (host) => {
    await mouseClick(evaluate, row(host), { button: "right" });
    await until("Files in the context menu", () =>
      exists("[role=menu] [data-menu=files]"),
    );
    await clickOn("[role=menu] [data-menu=files]");
  };
  const listed = (host, where) =>
    until(
      `${host}'s files listed in ${where}`,
      async () =>
        (await filesState(host)) === "connected" &&
        (await evaluate(
          `document.querySelector(${json(`${tabOf(host)} [data-files-folder]`)})?.dataset.filesFolder`,
        )) === where,
    );
  const tabIdOf = (selector) =>
    evaluate(
      `document.querySelector(${json(selector)})?.closest(".slot")?.dataset.tab ?? null`,
    );
  const showTab = async (tab) => {
    await clickOn(`[data-tab-bar] [data-tab="${tab}"]`);
    await until("the tab shown", () =>
      exists(`.slot[data-tab="${tab}"]:not([hidden])`),
    );
  };

  rmSync(localFolder, { recursive: true, force: true });
  rmSync(into, { recursive: true, force: true });
  rmSync(fetched, { recursive: true, force: true });
  mkdirSync(localFolder, { recursive: true });
  mkdirSync(into, { recursive: true });
  mkdirSync(fetched, { recursive: true });
  rmSync(remote("xfer"), { recursive: true, force: true });
  mkdirSync(remote("xfer"), { recursive: true });
  mkdirSync(join(scpFolder, "scp"), { recursive: true });
  writeFileSync(localFile("up1.cfg"), "hostname up1\n");
  writeFileSync(localFile("up2.cfg"), "hostname up2\n");
  writeFileSync(localFile("up3.cfg"), "hostname up3\n");
  writeFileSync(localFile("wait.txt"), "waited\n");
  // Big enough to be held part way (`hold sftp`).
  const big = Buffer.alloc(8 * 1024 * 1024);
  for (let i = 0; i < big.length; i += 4096) big[i] = i % 251;
  writeFileSync(localFile("big.bin"), big);

  let terminalTab = null;
  const terminal = async (command, expect) => {
    await showTab(terminalTab);
    await type(`${command}\r`);
    await until(`${expect} in the terminal`, async () =>
      (await screen()).includes(expect),
    );
  };

  try {
    writeFileSync(
      platformsFile,
      `${savedPlatforms ?? ""}smoke-xfer:\n  prompt: 'test-ssh\\$ '\n  file_folder: /xfer\nsmoke-scp:\n  prompt: 'test-ssh\\$ '\n  file_folder: /scp\n`,
    );
    mkdirSync(folder, { recursive: true });
    writeFileSync(
      join(folder, "_folder.yaml"),
      `credential_profile: files-login\nport: ${SSH_PORT}\nplatform: smoke-xfer\n`,
    );
    writeFileSync(join(folder, "xfer-r1.yaml"), "address: 127.0.0.1\n");
    writeFileSync(
      join(folder, "xfer-prod.yaml"),
      "address: 127.0.0.1\nenvironment: production\n",
    );
    writeFileSync(
      join(folder, "xfer-scp.yaml"),
      `address: 127.0.0.1\nport: ${SCP_PORT}\nplatform: smoke-scp\n`,
    );
    const folderRow = 'nav button[data-item=folder][data-path="Transfers"]';
    await until("the Transfers folder in the tree", () => exists(folderRow));
    await expandFolder(evaluate, folderRow);
    await until("the transfer hosts in the tree", () => exists(row(scp)));

    // A terminal to the same server, for its test shell's commands.
    await doubleClick(row(r1));
    await until("xfer-r1's terminal", async () =>
      (await screen()).includes("test-ssh$"),
    );
    terminalTab = await tabIdOf(SHOWN_SESSION);

    // Upload by the button: the dialog's two files go into /xfer.
    await openFiles(r1);
    await listed(r1, "/xfer");
    if (!(await answerDialogs([[localFile("up1.cfg"), localFile("up2.cfg")]])))
      fail("the file dialog could not be answered");
    await clickOn(`${tabOf(r1)} [data-files-action=upload]`);
    await ended(r1, 0, "done");
    await ended(r1, 1, "done");
    if (await exists(question(r1)))
      fail("an upload of new files on a lab host asked");
    await until(
      "the uploads listed",
      async () => json(await names(r1)) === json(["up1.cfg", "up2.cfg"]),
    );
    if (readFileSync(remote("xfer/up1.cfg"), "utf8") !== "hostname up1\n")
      fail("up1.cfg did not reach the device");
    const asked = (await dialogsAsked())[0];
    if (!asked?.multiple) fail(`the upload dialog was ${json(asked)}`);
    const hashOf = (kind) =>
      transferSelector(r1, 0, `[data-transfer-hash=${kind}]`).then((selector) =>
        evaluate(
          `document.querySelector(${json(selector)})?.querySelector("code")?.textContent ?? ""`,
        ),
      );
    if ((await hashOf("md5")) !== md5(localFile("up1.cfg")))
      fail("the upload's MD5 is not the file's");
    if ((await hashOf("sha256")) !== sha256(localFile("up1.cfg")))
      fail("the upload's SHA-256 is not the file's");
    for (const kind of ["md5", "sha256"]) {
      await clickOn(
        await transferSelector(r1, 0, `[data-transfer-hash=${kind}] button`),
      );
      const want =
        kind === "md5"
          ? md5(localFile("up1.cfg"))
          : sha256(localFile("up1.cfg"));
      await until(
        `the ${kind} on the clipboard`,
        () => readClipboard() === want,
      );
    }
    await screenshot("files-uploaded");
    step(
      "Upload puts the files chosen in the folder shown, the listing shows them; the list gives each upload's MD5 and SHA-256, copied with a click",
    );

    // Drops: elsewhere does nothing; on the tab over an existing name asks
    // once for several, Cancel keeps the device's files, Replace sends.
    const before = (await transfers(r1)).length;
    await drop("nav", [localFile("up3.cfg")]);
    await sleep(500);
    if ((await transfers(r1)).length !== before || (await exists(question(r1))))
      fail("a drop on the tree started an upload");
    writeFileSync(localFile("up1.cfg"), "hostname up1 changed\n");
    await drop(`${tabOf(r1)} [data-files-folder]`, [
      localFile("up1.cfg"),
      localFile("up2.cfg"),
    ]);
    await until("the replace question", () => exists(question(r1)));
    const listedNames = await evaluate(
      `[...document.querySelectorAll(${json(`${question(r1)} [data-files-replace-name]`)})].map((e) => e.dataset.filesReplaceName)`,
    );
    if (json(listedNames) !== json(["up1.cfg", "up2.cfg"]))
      fail(`the replace question lists ${json(listedNames)}`);
    if (await exists(`${question(r1)}[data-environment=production]`))
      fail("a lab host's upload asked as production");
    await screenshot("files-replace-question");
    await clickOn(`${question(r1)} [data-files-transfer-action=cancel]`);
    await until("the question gone", async () => !(await exists(question(r1))));
    if ((await transfers(r1)).length !== before)
      fail("Cancel started an upload");
    if (readFileSync(remote("xfer/up1.cfg"), "utf8") !== "hostname up1\n")
      fail("Cancel replaced up1.cfg");
    await drop(`${tabOf(r1)} [data-files-folder]`, [localFile("up1.cfg")]);
    await until("the replace question again", () => exists(question(r1)));
    await clickOn(`${question(r1)} [data-files-transfer-action=confirm]`);
    await ended(r1, before, "done");
    if (
      readFileSync(remote("xfer/up1.cfg"), "utf8") !== "hostname up1 changed\n"
    )
      fail("Replace did not send up1.cfg");
    step(
      "a drop on the tree does nothing; a drop on the tab over existing names asks once listing them, Cancel keeps the device's files, Replace sends",
    );

    // Download the ticked files into a folder picked; the next download
    // starts there and asks over a local file of that name.
    await clickOn(`${tabOf(r1)} [data-files-select="up1.cfg"]`);
    await clickOn(`${tabOf(r1)} [data-files-select="up2.cfg"]`);
    await answerDialogs([into]);
    let count = (await transfers(r1)).length;
    await clickOn(`${tabOf(r1)} [data-files-action=download]`);
    await ended(r1, count, "done");
    await ended(r1, count + 1, "done");
    if (
      readFileSync(join(into, "up1.cfg"), "utf8") !==
        "hostname up1 changed\n" ||
      readFileSync(join(into, "up2.cfg"), "utf8") !== "hostname up2\n"
    )
      fail("the downloads are not the device's files");
    writeFileSync(join(into, "up2.cfg"), "local edit\n");
    await clickOn(`${tabOf(r1)} [data-files-select="up1.cfg"]`);
    await answerDialogs([into]);
    count = (await transfers(r1)).length;
    await clickOn(`${tabOf(r1)} [data-files-action=download]`);
    await until("the local replace question", () =>
      exists(`${tabOf(r1)} [data-files-transfer-confirm=download]`),
    );
    const folderAsked = (await dialogsAsked())[0];
    if (folderAsked?.defaultPath !== into || !folderAsked?.directory)
      fail(`the second download's folder dialog was ${json(folderAsked)}`);
    if (!(await exists(`${question(r1)} [data-files-replace-name="up2.cfg"]`)))
      fail("the local replace question does not name up2.cfg");
    await clickOn(`${question(r1)} [data-files-transfer-action=cancel]`);
    if (readFileSync(join(into, "up2.cfg"), "utf8") !== "local edit\n")
      fail("Cancel replaced the local file");
    await answerDialogs([into]);
    await clickOn(`${tabOf(r1)} [data-files-action=download]`);
    await until("the local replace question again", () => exists(question(r1)));
    await clickOn(`${question(r1)} [data-files-transfer-action=confirm]`);
    await ended(r1, count, "done");
    if (readFileSync(join(into, "up2.cfg"), "utf8") !== "hostname up2\n")
      fail("Replace did not download up2.cfg");
    step(
      "Download puts the ticked files in the folder picked; the next starts there and asks before replacing a local file",
    );

    // Production: an upload asks first, Cancel focused.
    await openFiles(prod);
    await listed(prod, "/xfer");
    await answerDialogs([[localFile("up3.cfg")]]);
    await clickOn(`${tabOf(prod)} [data-files-action=upload]`);
    await until("the production question", () =>
      exists(`${question(prod)}[data-environment=production]`),
    );
    await until("Cancel focused", () =>
      focused(`${question(prod)} [data-files-transfer-action=cancel]`),
    );
    await screenshot("files-production-upload");
    await clickOn(`${question(prod)} [data-files-transfer-action=cancel]`);
    await sleep(300);
    if (existsSync(remote("xfer/up3.cfg"))) fail("Cancel uploaded up3.cfg");
    await answerDialogs([[localFile("up3.cfg")]]);
    await clickOn(`${tabOf(prod)} [data-files-action=upload]`);
    await until("the production question again", () => exists(question(prod)));
    await clickOn(`${question(prod)} [data-files-transfer-action=confirm]`);
    await ended(prod, 0, "done");
    if (!existsSync(remote("xfer/up3.cfg"))) fail("up3.cfg was not uploaded");
    await closeTabOf(evaluate, tabOf(prod));
    step(
      "an upload to a production host asks first with Cancel focused; Cancel sends nothing, the answer sends",
    );

    // A held transfer: progress and speed, the next waiting in order and
    // removed; going on while the terminal is shown.
    await terminal("hold sftp 1048576", "sftp held from 1048576");
    const r1Tab = await tabIdOf(tabOf(r1));
    await showTab(r1Tab);
    count = (await transfers(r1)).length;
    await answerDialogs([[localFile("big.bin"), localFile("wait.txt")]]);
    await clickOn(`${tabOf(r1)} [data-files-action=upload]`);
    // Held part way: the client has up to 2 MiB in flight, so where its
    // count stops varies; it stays put.
    const heldAt = async () => {
      const each = await transferAt(r1, count)();
      return each?.state === "running" && / of 8\.0 MiB/.test(each.text)
        ? each.text
        : null;
    };
    await until("big.bin held part way", async () => {
      const first = await heldAt();
      if (!first) return false;
      await sleep(1000);
      return (await heldAt()) === first;
    });
    await until("its speed shown", async () =>
      /\/s/.test(
        await evaluate(
          `document.querySelector(${json(await transferSelector(r1, count, "[data-transfer-speed]"))})?.textContent ?? ""`,
        ),
      ),
    );
    const waiting = await transferAt(r1, count + 1)();
    if (waiting?.state !== "waiting" || !/after 1/.test(waiting.text))
      fail(`wait.txt is not waiting behind big.bin: ${json(waiting)}`);
    await screenshot("files-transfers");
    await clickOn(
      await transferSelector(r1, count + 1, "[data-transfer-action=cancel]"),
    );
    await until(
      "wait.txt taken off the list",
      async () => (await transfers(r1)).length === count + 1,
    );
    if (!(await exists(`${tabOf(r1)} [data-transfer-name="big.bin"]`)))
      fail("Remove took the running transfer off the list");
    await terminal("release sftp", "sftp released");
    await ended(r1, count, "done");
    if (readFileSync(remote("xfer/big.bin")).length !== big.length)
      fail("big.bin did not arrive whole while the terminal was shown");
    step(
      "a running transfer shows progress and speed, the next waits in order and Remove takes it off the list; it goes on to the end while another tab is shown",
    );

    // Cancel a running upload: its partial file is removed.
    rmSync(remote("xfer/big.bin"));
    await terminal("hold sftp 1048576", "sftp held from 1048576");
    await showTab(r1Tab);
    await clickOn(`${tabOf(r1)} [data-files-action=refresh]`);
    count = (await transfers(r1)).length;
    await answerDialogs([[localFile("big.bin")]]);
    await clickOn(`${tabOf(r1)} [data-files-action=upload]`);
    await until(
      "big.bin running",
      async () => (await transferAt(r1, count)())?.state === "running",
    );
    await clickOn(
      await transferSelector(r1, count, "[data-transfer-action=cancel]"),
    );
    await terminal("release sftp", "sftp released");
    await showTab(r1Tab);
    const cancelled = await ended(r1, count, "cancelled");
    if (
      !cancelled.text.includes("The partial file was removed from the device")
    )
      fail(`the cancelled upload says ${cancelled.text}`);
    if (existsSync(remote("xfer/big.bin")))
      fail("the cancelled upload's partial file remains");
    step(
      "a cancelled upload says its partial file was removed, and it is gone",
    );

    // Closing the tab and the main window while a transfer runs asks
    // first; Cancel keeps everything.
    await terminal("hold sftp 1048576", "sftp held from 1048576");
    await showTab(r1Tab);
    count = (await transfers(r1)).length;
    await answerDialogs([[localFile("big.bin")]]);
    await clickOn(`${tabOf(r1)} [data-files-action=upload]`);
    await until(
      "big.bin running",
      async () => (await transferAt(r1, count)())?.state === "running",
    );
    await clickOn(`[data-tab-bar] [data-tab="${r1Tab}"] [data-tab-close]`);
    await until("the close question", () =>
      exists("[data-confirm-close][data-confirm-transfers='1']"),
    );
    await until("Cancel focused", () => focused("[data-confirm=cancel]"));
    await clickOn("[data-confirm=cancel]");
    await until(
      "the close question gone",
      async () => !(await exists("[data-confirm-close]")),
    );
    if ((await transferAt(r1, count)())?.state !== "running")
      fail("Cancel on the close question stopped the transfer");
    await closeWithButton(evaluate);
    const quit = await until("the quit question", () =>
      evaluate(
        `document.querySelector("[data-confirm-quit]")?.textContent ?? null`,
      ),
    );
    if (!/1 transfer is running or waiting/.test(quit))
      fail(`the quit question does not name the transfer: ${quit}`);
    await mouseClick(evaluate, "[data-quit=cancel]");
    await until(
      "the quit question gone",
      async () => !(await exists("[data-confirm-quit]")),
    );
    if ((await transferAt(r1, count)())?.state !== "running")
      fail("Cancel on the quit question stopped the transfer");
    await clickOn(`[data-tab-bar] [data-tab="${r1Tab}"] [data-tab-close]`);
    await until("the close question again", () =>
      exists("[data-confirm-close][data-confirm-transfers]"),
    );
    await clickOn("[data-confirm=close]");
    await until("the Files tab closed", async () => !(await exists(tabOf(r1))));
    await terminal("release sftp", "sftp released");
    await until(
      "the cancelled upload's file removed",
      async () => !existsSync(remote("xfer/big.bin")),
    );
    // Its connection goes once the file is removed.
    await sleep(1000);
    await terminal("show sftp", "sftp connections: 0");
    step(
      "closing a Files tab or the main window while a transfer runs asks first (the app's question names it); Cancel keeps it running, Close tab cancels it",
    );

    // SCP only: the form in place of the listing.
    await openFiles(scp);
    await until("the SCP host's key asked", () =>
      exists(`${tabOf(scp)} [data-decision=trustAndRemember]:not(:disabled)`),
    );
    await clickOn(`${tabOf(scp)} [data-decision=trustAndRemember]`);
    await until(
      "the SCP form",
      async () => (await filesState(scp)) === "transfer-only",
    );
    const status = await evaluate(
      `document.querySelector(${json(`${tabOf(scp)} [data-files-status]`)})?.textContent ?? ""`,
    );
    if (!/No connection held/.test(status))
      fail(`the SCP tab's status says ${status}`);
    if (!(await exists(`${tabOf(scp)} [data-files-replaced]`)))
      fail("the SCP form does not say an existing file is replaced");
    if (await exists(`${tabOf(scp)} [data-files-folder]`))
      fail("the SCP tab shows a listing");
    const field = (name) =>
      evaluate(
        `document.querySelector(${json(`${tabOf(scp)} [name=${name}]`)})?.value ?? null`,
      );
    // Before a file is chosen the path is the platform's file folder.
    if ((await field("files-send-remote")) !== "/scp/")
      fail(
        `the send's path starts as ${json(await field("files-send-remote"))}, not the platform's file folder`,
      );
    if (
      !(await evaluate(
        `document.querySelector(${json(`${tabOf(scp)} [data-files-action=send]`)}).disabled`,
      ))
    )
      fail("Send is offered before a file is chosen");
    await answerDialogs([localFile("up1.cfg")]);
    await clickOn(`${tabOf(scp)} [data-files-action=choose-send]`);
    await until(
      "the path filled in",
      async () => (await field("files-send-remote")) === "/scp/up1.cfg",
    );
    await screenshot("files-scp");
    await clickOn(`${tabOf(scp)} [data-files-action=send]`);
    const sent = await ended(scp, 0, "done");
    if (!sent.text.includes("could not be checked"))
      fail(`the SCP upload says ${sent.text}`);
    if (
      readFileSync(join(scpFolder, "scp/up1.cfg"), "utf8") !==
      "hostname up1 changed\n"
    )
      fail("the SCP send did not reach the device");
    await drop(`${tabOf(scp)} [data-files-send]`, [localFile("up2.cfg")]);
    await until(
      "the dropped file in the form",
      async () => (await field("files-send-local")) === localFile("up2.cfg"),
    );
    if ((await field("files-send-remote")) !== "/scp/up2.cfg")
      fail("the dropped file's path was not filled in");
    await fillForm(
      { "files-fetch-remote": "/scp/up1.cfg", "files-fetch-local": fetched },
      `${tabOf(scp)} [data-files-fetch]`,
    );
    await ended(scp, 1, "done");
    if (
      readFileSync(join(fetched, "up1.cfg"), "utf8") !==
      "hostname up1 changed\n"
    )
      fail("the SCP fetch did not arrive");
    step(
      "a device with SCP only shows the form, says no connection is held and that an existing file is replaced; it sends a chosen or dropped file to the path filled in and fetches a typed path",
    );

    // A refusal in the device's words, the hint under it.
    await doubleClick(row(scp));
    await until("xfer-scp's terminal", async () =>
      (await screen()).includes("test-ssh$"),
    );
    const scpTerminal = await tabIdOf(SHOWN_SESSION);
    await type("scp off\r");
    await until("scp off", async () => (await screen()).includes("scp off"));
    await showTab(await tabIdOf(tabOf(scp)));
    await clickOn(`${tabOf(scp)} [data-files-action=send]`);
    await ended(scp, 2, "failed");
    const reasonText = await evaluate(
      `document.querySelector(${json(await transferSelector(scp, 2, "[data-transfer-reason]"))})?.textContent ?? ""`,
    );
    const hintText = await evaluate(
      `document.querySelector(${json(await transferSelector(scp, 2, "[data-transfer-hint]"))})?.textContent ?? ""`,
    );
    if (!reasonText.includes("Administratively disabled"))
      fail(`the refusal says ${reasonText}`);
    if (!hintText.includes("ip scp server enable"))
      fail(`the refusal's hint says ${hintText}`);
    await screenshot("files-scp-refused");
    await showTab(scpTerminal);
    await type("scp on\r");
    await until("scp on", async () => (await screen()).includes("scp on"));
    step("a device's refusal shows in its own words with the hint under it");

    // SCP only, with a login that asks a one-time code each time (#109):
    // each transfer's own login asks it over the SCP form; the password
    // typed for the tab is not asked again; Cancel fails that transfer
    // alone and the next asks for itself. The device's key was remembered
    // above, so only the code is asked.
    writeFileSync(
      otpFile,
      `address: 127.0.0.1\nport: ${SCP_PORT}\nplatform: smoke-scp\n`,
    );
    await until("the one-time-code host in the tree", () => exists(row(otp)));
    await openFiles(otp);
    const typedLogin = `${tabOf(otp)} [data-files-login] [data-login-prompt]`;
    await until("the login asked over the tab", () => exists(typedLogin));
    await evaluate(`(() => {
      const form = document.querySelector(${json(typedLogin)});
      for (const [name, value] of [["answer-0", ${json(OTP_USERNAME)}], ["answer-1", ${json(PASSWORD)}]]) {
        const input = form.querySelector("[name=" + name + "]");
        input.value = value;
        input.dispatchEvent(new Event("input", { bubbles: true }));
      }
    })()`);
    await clickOn(`${typedLogin} button[type=submit]`);
    const codePrompt = `${tabOf(otp)} [data-login-prompt]`;
    /** Waits for the code's question over the tab: one field, the code. */
    const codeAsked = async (what) => {
      await until(`the code asked for ${what}`, async () =>
        (
          await evaluate(
            `document.querySelector(${json(codePrompt)})?.textContent ?? ""`,
          )
        ).includes("Verification code:"),
      );
      if (await exists(`${codePrompt} [name=answer-1]`))
        fail(`${what} asks more than the code`);
    };
    const answerCode = async () => {
      await until("the code's field focused", () =>
        evaluate(
          `document.activeElement === document.querySelector(${json(`${codePrompt} input[name=answer-0]`)})`,
        ),
      );
      await type(ONE_TIME_CODE);
      await clickOn(`${codePrompt} button[type=submit]`);
      await until(
        "the code's question gone",
        async () => !(await exists(codePrompt)),
      );
    };
    await codeAsked("the tab's login");
    await answerCode();
    await until(
      "the one-time-code host's SCP form",
      async () => (await filesState(otp)) === "transfer-only",
    );
    const otpField = (name) =>
      evaluate(
        `document.querySelector(${json(`${tabOf(otp)} [name=${name}]`)})?.value ?? null`,
      );
    rmSync(join(scpFolder, "scp/up3.cfg"), { force: true });
    await answerDialogs([localFile("up3.cfg")]);
    await clickOn(`${tabOf(otp)} [data-files-action=choose-send]`);
    await until(
      "the one-time-code host's path filled in",
      async () => (await otpField("files-send-remote")) === "/scp/up3.cfg",
    );
    await clickOn(`${tabOf(otp)} [data-files-action=send]`);
    await codeAsked("the send's login");
    // The row follows the transfer at most every 200 ms: it may still
    // read Waiting when the question appears.
    await until(
      "the send asking its code shown as connecting",
      async () => (await transferAt(otp, 0)())?.state === "connecting",
    );
    if ((await filesState(otp)) !== "transfer-only")
      fail("the send's question is not over the SCP form");
    await screenshot("files-scp-code");
    await clickOn(`${codePrompt} .actions button[type=button]`);
    const declined = await ended(otp, 0, "failed");
    if (!declined.text.includes("the login was cancelled"))
      fail(`the cancelled send says ${declined.text}`);
    if (await exists(codePrompt)) fail("the cancelled question stayed");
    if (existsSync(join(scpFolder, "scp/up3.cfg")))
      fail("a send whose login was cancelled reached the device");
    await clickOn(`${tabOf(otp)} [data-files-action=send]`);
    await codeAsked("the next send's login");
    await answerCode();
    await ended(otp, 1, "done");
    if (
      readFileSync(join(scpFolder, "scp/up3.cfg"), "utf8") !== "hostname up3\n"
    )
      fail("the send whose code was answered did not reach the device");
    step(
      "on a device with SCP only, each send's login asks its one-time code over the form (the typed password is not asked again); Cancel fails that send alone, the next asks again and goes through once answered",
    );
    await closeTabOf(evaluate, tabOf(otp));

    await closeTabOf(evaluate, tabOf(scp));
    for (const tab of [scpTerminal, terminalTab]) {
      await clickOn(`[data-tab-bar] [data-tab="${tab}"] [data-tab-close]`);
      await until("the close question", () => exists("[data-confirm=close]"));
      await clickOn("[data-confirm=close]");
      await until(
        "the session's tab closed",
        async () => !(await exists(`.slot[data-tab="${tab}"]`)),
      );
    }
  } finally {
    // The dialogs answer for themselves again.
    await evaluate(`(() => {
      if (window.__smokeFetch) {
        window.fetch = window.__smokeFetch;
        delete window.__smokeFetch;
      }
    })()`).catch(() => {});
    if (savedKnownHosts === null) rmSync(knownHosts, { force: true });
    else writeFileSync(knownHosts, savedKnownHosts);
    if (savedPlatforms === null) rmSync(platformsFile, { force: true });
    else writeFileSync(platformsFile, savedPlatforms);
    rmSync(folder, { recursive: true, force: true });
    rmSync(otpFile, { force: true });
  }
}

/**
 * The file job (#107), after `fileTransfers` (the `files-login` profile,
 * the example server's key trusted): a file sent from a folder's panel to
 * its two hosts over SFTP (a telnet one skipped, the production one
 * confirmed), checked on disk; its kept run opened with the file's name,
 * size and hashes, refused as a saved job; run again, both hosts already
 * there.
 */
async function fileJob({ evaluate, fillForm, screenshot, step }) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const folder = join(workspace, "Staging");
  const platformsFile = join(workspace, "_platforms.yaml");
  const savedPlatforms = existsSync(platformsFile)
    ? readFileSync(platformsFile, "utf8")
    : null;
  const localFolder = join(userData, "stage-local");
  const localFile = join(localFolder, "stage.bin");
  // Each host has a folder of its own on the one example server: two
  // hosts sending to one path would be two devices sharing a file, and
  // the second would find the first's half-written file there.
  const remote = (name, folder = "stage") => join(sftpFolder, folder, name);
  const hash = (kind) =>
    createHash(kind).update(readFileSync(localFile)).digest("hex");
  const atStep = (name) => exists(`${JOB_TAB}[data-step=${name}]`);
  const r1 = "Staging/stage-r1.yaml";
  const prod = "Staging/stage-prod.yaml";
  const telnet = "Staging/stage-telnet.yaml";
  const results = () =>
    evaluate(`Object.fromEntries([...document.querySelectorAll(${json(`${JOB_TAB} [data-job-result]`)})].map(
      (r) => [r.dataset.jobResult, { state: r.dataset.hostState, reason: r.querySelector("[data-job-reason]")?.textContent ?? "" }]))`);
  const finished = () =>
    until("the file job finished", () =>
      exists(`${JOB_TAB} [data-job-progress][data-finished]`),
    );
  const confirmAndStart = async () => {
    await clickOn(`${JOB_TAB} [name=job-confirm-production]`);
    await until("the start button enabled", () =>
      evaluate(
        `!document.querySelector(${json(`${JOB_TAB} [data-job=start]`)}).disabled`,
      ),
    );
    await clickOn(`${JOB_TAB} [data-job=start]`);
  };

  rmSync(localFolder, { recursive: true, force: true });
  mkdirSync(localFolder, { recursive: true });
  const bytes = Buffer.alloc(3 * 1024 * 1024);
  for (let i = 0; i < bytes.length; i += 1024) bytes[i] = i % 253;
  writeFileSync(localFile, bytes);
  for (const each of ["stage", "stage-prod"]) {
    rmSync(join(sftpFolder, each), { recursive: true, force: true });
    mkdirSync(join(sftpFolder, each), { recursive: true });
  }

  try {
    writeFileSync(
      platformsFile,
      `${savedPlatforms ?? ""}smoke-stage:\n  prompt: 'test-ssh\\$ '\n  file_folder: /stage\nsmoke-stage-prod:\n  prompt: 'test-ssh\\$ '\n  file_folder: /stage-prod\n`,
    );
    mkdirSync(folder, { recursive: true });
    writeFileSync(
      join(folder, "_folder.yaml"),
      `credential_profile: files-login\nport: ${SSH_PORT}\nplatform: smoke-stage\n`,
    );
    writeFileSync(join(folder, "stage-r1.yaml"), "address: 127.0.0.1\n");
    writeFileSync(
      join(folder, "stage-prod.yaml"),
      "address: 127.0.0.1\nenvironment: production\nplatform: smoke-stage-prod\n",
    );
    writeFileSync(
      join(folder, "stage-telnet.yaml"),
      "address: 127.0.0.1\nprotocol: telnet\n",
    );
    const folderRow = 'nav button[data-item=folder][data-path="Staging"]';
    await until("the Staging folder in the tree", () => exists(folderRow));

    // From the folder's panel, a third kind beside snippet and script.
    await clickOn(folderRow);
    await showDetails(evaluate);
    await until("the Staging folder's side panel", () =>
      exists(".side [data-folder-send-file]"),
    );
    await clickOn(".side [data-folder-send-file]");
    await until("the send-file tab", () => atStep("file"));
    await fillForm({ "job-file": localFile }, `${JOB_TAB} form`);
    await until("the pre-flight list", () => atStep("preflight"));
    const plan = await evaluate(`(() => {
      const d = document.querySelector(${json(JOB_TAB)});
      return {
        name: d.querySelector("[data-job-file-name]")?.textContent ?? "",
        size: d.querySelector("[data-job-file-size]")?.dataset.jobFileSize ?? "",
        runs: [...d.querySelectorAll("[data-job-host]")].map((r) => r.dataset.jobHost),
        destinations: [...d.querySelectorAll("[data-job-host] [data-job-destination]")].map((c) => c.dataset.jobDestination),
        skipped: Object.fromEntries([...d.querySelectorAll("[data-job-skipped]")].map(
          (r) => [r.dataset.jobSkipped, r.dataset.skipReason])),
        production: d.querySelector("[data-job-production-names]")?.textContent ?? "",
        parallel: d.querySelector("[name=job-parallel]")?.value ?? "",
        startDisabled: d.querySelector("[data-job=start]").disabled,
      };
    })()`);
    if (plan.name !== "stage.bin" || plan.size !== String(bytes.length))
      fail(`pre-flight shows the file as ${plan.name} of ${plan.size} bytes`);
    if (json(plan.runs) !== json([prod, r1]))
      fail(`the file job would send to ${json(plan.runs)}`);
    if (
      json(plan.destinations) !==
      json(["/stage-prod/stage.bin", "/stage/stage.bin"])
    )
      fail(`the destinations are ${json(plan.destinations)}`);
    if (json(plan.skipped) !== json({ [telnet]: "notSsh" }))
      fail(`the file job skips ${json(plan.skipped)}`);
    if (plan.production !== "stage-prod" || !plan.startDisabled)
      fail(`the production host must be confirmed first: ${json(plan)}`);
    if (plan.parallel !== "3")
      fail(`hosts at once starts at ${plan.parallel}, not 3`);
    if (await exists(`${JOB_TAB} [data-save-job=open]:not(:disabled)`))
      fail("a file job can be saved as a job");
    await screenshot("file-job-preflight");
    step(
      "Send file… on a folder plans the file's name and size, each host's platform file folder, skips the telnet host, asks for production once and starts at 3 hosts at once",
    );

    await confirmAndStart();
    await finished();
    const sent = await results();
    for (const host of [r1, prod]) {
      if (sent[host]?.state !== "done")
        fail(`${host} ended ${json(sent[host])}`);
    }
    for (const each of ["stage", "stage-prod"]) {
      if (!readFileSync(remote("stage.bin", each)).equals(bytes))
        fail(`the file did not reach /${each} byte for byte`);
    }
    await screenshot("file-job-sent");
    step("the file job sends the file to each host, each ends Sent");

    // The kept run: the file, its hashes, the destination; not saveable.
    await clickOn(`${JOB_TAB} [data-job=view]`);
    await until("the file run's tab", () =>
      exists(`${RUN_TAB} [data-run-file-md5]`),
    );
    const kept = await evaluate(`(() => {
      const d = document.querySelector(${json(RUN_TAB)});
      return {
        file: d.querySelector("[data-run-file]")?.dataset.runFile ?? "",
        md5: d.querySelector("[data-run-file-md5]")?.textContent.trim() ?? "",
        sha256: d.querySelector("[data-run-file-sha256]")?.textContent.trim() ?? "",
        destination: d.querySelector("[data-run-file-destination]")?.textContent.trim() ?? "",
        overwrite: d.querySelector("[data-run-file-overwrite]")?.dataset.runFileOverwrite ?? "",
        hosts: Object.fromEntries([...d.querySelectorAll("[data-run-host]")].map(
          (r) => [r.dataset.runHost, { state: r.dataset.hostState, bytes: r.querySelector("[data-run-bytes]")?.dataset.runBytes ?? "" }])),
        saveable: !!d.querySelector("[data-save-job=open]:not(:disabled)"),
        saveTitle: d.querySelector("[data-save-job=open]")?.title ?? "",
      };
    })()`);
    if (kept.file !== "stage.bin") fail(`the run keeps the file ${kept.file}`);
    if (kept.md5 !== hash("md5") || kept.sha256 !== hash("sha256"))
      fail(`the run's hashes are not the file's: ${json(kept)}`);
    if (
      !kept.destination.includes("platform file folder") ||
      kept.overwrite !== "no"
    )
      fail(`the run keeps the destination as ${json(kept)}`);
    for (const host of [r1, prod]) {
      const entry = kept.hosts[host];
      if (entry?.state !== "done" || entry.bytes !== String(bytes.length))
        fail(`the run keeps ${host} as ${json(entry)}`);
    }
    if (kept.saveable || !kept.saveTitle.includes("cannot be saved"))
      fail(`the file run can be saved as a job: ${json(kept)}`);
    await screenshot("file-job-run");
    step(
      "its kept run shows the file's name, size, MD5 and SHA-256, the destination and each host's bytes; saving it as a job is refused with the reason",
    );

    // Opened later from the Jobs view, read back from disk.
    const runId = await evaluate(
      `document.querySelector(${json(RUN_TAB)})?.dataset.runTab ?? ""`,
    );
    await closeTabOf(evaluate, RUN_TAB);
    await until("the run tab closed", async () => !(await exists(RUN_TAB)));
    await openRun(evaluate, runId);
    await until("the reopened run's hashes", () =>
      exists(`${RUN_TAB} [data-run-file-md5]`),
    );
    const reopened = await evaluate(
      `document.querySelector(${json(`${RUN_TAB} [data-run-file-sha256]`)})?.textContent.trim() ?? ""`,
    );
    if (reopened !== hash("sha256"))
      fail(`the reopened run's SHA-256 is ${reopened}`);
    step("the kept run opened later from the Jobs view shows the same file");

    // Run again without overwrite: the file is already there.
    await clickOn(`${RUN_TAB} [data-run-action=again]`);
    await until("run again's pre-flight", () => atStep("preflight"));
    await confirmAndStart();
    await finished();
    const again = await results();
    for (const host of [r1, prod]) {
      if (again[host]?.state !== "alreadyThere")
        fail(`run again left ${host} ${json(again[host])}`);
    }
    step("run again without overwrite counts each host as already there");
    await clickOn(`${JOB_TAB} [data-job=close]`);
    await until("the job tab closed", async () => !(await exists(JOB_TAB)));
    await closeTabOf(evaluate, RUN_TAB);

    // Two hosts selected with Ctrl held (#110): Send file… from the menu
    // reaches pre-flight with exactly those two, in the tree's order.
    const hostRow = (id) => `nav button[data-host=${json(id)}]`;
    await expandFolder(evaluate, folderRow);
    await clickOn(hostRow(r1));
    await mouseClick(evaluate, hostRow(prod), { modifiers: CTRL });
    await until("two Staging hosts selected", () =>
      exists("[data-selection='2']"),
    );
    await mouseClick(evaluate, hostRow(r1), { button: "right" });
    await until("the selection's menu", () =>
      exists("[role=menu][data-menu-hosts='2'] [data-menu=send-file]"),
    );
    await clickOn("[role=menu] [data-menu=send-file]");
    await until("the send-file tab on the selection", () => atStep("file"));
    const tabs = await evaluate(
      `[...document.querySelectorAll("[data-tab-view=job]")].map((t) => t.textContent.trim()).join(" | ")`,
    );
    if (!tabs.includes("Send file to 2 hosts"))
      fail(`the job tab is called ${json(tabs)}`);
    await fillForm({ "job-file": localFile }, `${JOB_TAB} form`);
    await until("the selection's pre-flight", () => atStep("preflight"));
    const chosen = await evaluate(`(() => {
      const d = document.querySelector(${json(JOB_TAB)});
      return {
        runs: [...d.querySelectorAll("[data-job-host]")].map((r) => r.dataset.jobHost),
        skipped: [...d.querySelectorAll("[data-job-skipped]")].map((r) => r.dataset.jobSkipped),
      };
    })()`);
    if (json(chosen) !== json({ runs: [prod, r1], skipped: [] }))
      fail(`the file job on the selection plans ${json(chosen)}`);
    await screenshot("file-job-selection");
    step(
      "Send file… on two hosts selected with Ctrl+click reached pre-flight with exactly those two, in the tree's order",
    );
    await clickOn(`${JOB_TAB} [data-job=dismiss]`);
    await until("the job tab closed", async () => !(await exists(JOB_TAB)));
    await clickOn("[data-selection-clear]");
  } finally {
    if (savedPlatforms === null) rmSync(platformsFile, { force: true });
    else writeFileSync(platformsFile, savedPlatforms);
    rmSync(folder, { recursive: true, force: true });
    rmSync(localFolder, { recursive: true, force: true });
  }
}

/** The keys held during a click, as DevTools counts them. */
const CTRL = 2;
const SHIFT = 8;

/**
 * A selection of several hosts (#110), in the Jobs folder `jobs` leaves
 * (jobs-r1 trusted with its profile, jobs-odd of an unknown platform,
 * jobs-telnet): Ctrl+click and Shift+click with the mouse, the count and
 * its clear button, Escape, the details sheet closing, the context menu's
 * five job entries and nothing else, a snippet job on exactly the
 * selected hosts that runs (and is not saveable), a right-click outside
 * the selection, Ctrl on a folder, a drag, Shift+click under the filter
 * field, and selected hosts deleted on disk leaving the selection.
 */
async function hostSelection({
  evaluate,
  devtools,
  fillForm,
  screenshot,
  step,
}) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector, options) =>
    mouseClick(evaluate, selector, options);
  const dialog = JOB_TAB;
  const atStep = (name) => exists(`${dialog}[data-step=${name}]`);
  const r1 = "Jobs/jobs-r1.yaml";
  const odd = "Jobs/jobs-odd.yaml";
  const telnet = "Jobs/jobs-telnet.yaml";
  const fileOf = (id) => join(workspace, ...id.split("/"));
  const originals = Object.fromEntries(
    [odd, telnet].map((id) => [id, readFileSync(fileOf(id), "utf8")]),
  );
  const row = (id) => `nav button[data-host=${json(id)}]`;
  const folderRow = 'nav button[data-item=folder][data-path="Jobs"]';
  const menu = "[role=menu]";
  const snippetsFile = join(workspace, "_snippets.yaml");
  const savedSnippets = existsSync(snippetsFile)
    ? readFileSync(snippetsFile, "utf8")
    : null;
  /** The hosts marked selected, in the order the tree shows them. */
  const chosen = () =>
    evaluate(
      `[...document.querySelectorAll("nav button[data-host][aria-pressed=true]")].map((b) => b.dataset.host)`,
    );
  /** The count the Hosts view shows, or null when it shows none. */
  const count = () =>
    evaluate(
      `document.querySelector("[data-selection]")?.dataset.selection ?? null`,
    );
  const selectionIs = async (ids, shown, what) => {
    await until(
      what,
      async () =>
        json(await chosen()) === json(ids) && (await count()) === shown,
    );
  };
  const entries = () =>
    evaluate(
      `[...document.querySelectorAll(${json(`${menu} [data-menu]`)})].map((b) => b.dataset.menu)`,
    );
  const key = async (spec) => {
    await devtools.send("Input.dispatchKeyEvent", { type: "keyDown", ...spec });
    await devtools.send("Input.dispatchKeyEvent", { type: "keyUp", ...spec });
  };
  const escape = { key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 };
  const filterWords = (words) =>
    evaluate(`(() => {
      const input = document.querySelector("input[name=host-filter]");
      input.value = ${json(words)};
      input.dispatchEvent(new Event("input", { bubbles: true }));
    })()`);
  /** Selects `first` alone, then adds `more` with Ctrl held. */
  const pick = async (first, ...more) => {
    await clickOn(row(first));
    for (const id of more) await clickOn(row(id), { modifiers: CTRL });
  };

  // Words for the filter field to find jobs-odd and jobs-telnet only, and
  // the snippet; written first, so that no reload of half-written files
  // takes a row away under the mouse later.
  for (const id of [odd, telnet])
    writeFileSync(fileOf(id), `${originals[id]}custom:\n  pick: zebrafish\n`);
  writeFileSync(
    snippetsFile,
    `${savedSnippets ?? ""}Selection check: show hostname\n`,
  );
  try {
    await showSideView(evaluate, "snippets");
    await until("the selection's snippet", () =>
      exists('[data-snippet="Selection check"]'),
    );
    await showSideView(evaluate, "hosts");
    await until("the Jobs folder in the tree", () => exists(folderRow));
    await expandFolder(evaluate, folderRow);
    await until("the Jobs hosts", () => exists(row(telnet)));
    await filterWords("zebrafish");
    await until(
      "the filter field showing jobs-odd and jobs-telnet only",
      async () =>
        (await exists(row(odd))) &&
        (await exists(row(telnet))) &&
        !(await exists(row(r1))),
    );
    await filterWords("");
    await until("every host back", () => exists(row(r1)));
    const order = await evaluate(
      `[...document.querySelectorAll('nav button[data-host^="Jobs/"]')].map((b) => b.dataset.host)`,
    );
    if (json(order) !== json([odd, r1, telnet]))
      fail(`the Jobs folder shows ${json(order)}`);

    // Ctrl+click adds and takes out; the count shows; the details sheet,
    // which follows one item, closes.
    await clickOn(row(r1));
    await showDetails(evaluate);
    await clickOn(row(odd), { modifiers: CTRL });
    await selectionIs([odd, r1], "2", "jobs-odd and jobs-r1 selected");
    await until(
      "the details sheet closed",
      async () => !(await exists("[data-details-sheet]")),
    );
    await clickOn(row(telnet), { modifiers: CTRL });
    await selectionIs([odd, r1, telnet], "3", "three hosts selected");
    await clickOn(row(telnet), { modifiers: CTRL });
    await selectionIs([odd, r1], "2", "jobs-telnet taken out again");
    await screenshot("host-selection");
    step(
      "Ctrl+click added hosts to the selection and took one out; the Hosts view counted them and the details sheet closed",
    );

    // The menu on a selected row: the five job entries, nothing else.
    await clickOn(row(r1), { button: "right" });
    await until("the selection's menu", () =>
      exists(`${menu}[data-menu-hosts="2"]`),
    );
    const offered = await entries();
    if (
      json(offered) !==
      json(["run-snippet", "run-script", "send-file", "back-up", "enrich"])
    )
      fail(`the menu on two hosts offers ${json(offered)}`);
    if (json(await chosen()) !== json([odd, r1]))
      fail("the right-click changed the selection");
    await screenshot("host-selection-menu");
    step(
      "the context menu on a selected row offered only Run snippet, Run script, Send file, Back up configs and Fill in from devices",
    );

    // Run snippet… on exactly those two: one runs, the other is skipped.
    await clickOn(`${menu} [data-menu=run-snippet]`);
    await until("the run-snippet tab", () => atStep("snippet"));
    const title = await evaluate(
      `[...document.querySelectorAll("[data-tab-view=job]")].map((t) => t.textContent.trim()).join(" | ")`,
    );
    if (!title.includes("Snippet on 2 hosts"))
      fail(`the job tab is called ${json(title)}`);
    await evaluate(`(() => {
      const select = document.querySelector("${dialog} [name=job-snippet]");
      select.value = "Selection check";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
    await clickOn(`${dialog} [data-job=plan]`);
    await until("the pre-flight list", () => atStep("preflight"));
    const plan = await evaluate(`(() => {
      const d = document.querySelector("${dialog}");
      return {
        runs: [...d.querySelectorAll("[data-job-host]")].map((r) => r.dataset.jobHost),
        skipped: Object.fromEntries([...d.querySelectorAll("[data-job-skipped]")].map(
          (r) => [r.dataset.jobSkipped, r.dataset.skipReason])),
        saveable: !!d.querySelector("[data-save-job=open]:not(:disabled)"),
      };
    })()`);
    if (json(plan.runs) !== json([r1]))
      fail(`the job on the selection would run on ${json(plan.runs)}`);
    if (json(plan.skipped) !== json({ [odd]: "noPlatform" }))
      fail(`the job on the selection skips ${json(plan.skipped)}`);
    if (plan.saveable) fail("a job on two hosts can be saved as a job");
    await clickOn(`${dialog} [name=job-confirm-production]`);
    await until("the start button enabled", () =>
      evaluate(
        `!document.querySelector("${dialog} [data-job=start]").disabled`,
      ),
    );
    await clickOn(`${dialog} [data-job=start]`);
    await until("the job on the selection finished", () =>
      exists(`${dialog} [data-job-progress][data-finished]`),
    );
    const result = await evaluate(`(() => {
      const row = document.querySelector('${dialog} [data-job-result="${r1}"]');
      return { state: row?.dataset.hostState, output: row?.querySelector("[data-job-output]")?.textContent ?? "" };
    })()`);
    if (
      result.state !== "done" ||
      !result.output.includes("lab-r1# show hostname")
    )
      fail(`the job on the selection ended ${json(result)}`);
    await clickOn(`${dialog} [data-job=close]`);
    await until("the job tab closed", async () => !(await exists(dialog)));
    if ((await count()) !== "2") fail("the job ended the selection");
    step(
      "Run snippet… on the two selected hosts planned exactly those (one skipped for its platform), ran on the other and cannot be saved as a job",
    );

    // The clear button, then a Shift+click range and Escape.
    await clickOn("[data-selection-clear]");
    await selectionIs([], null, "the selection cleared");
    await clickOn(row(odd));
    await clickOn(row(telnet), { modifiers: SHIFT });
    await selectionIs([odd, r1, telnet], "3", "the range selected");
    await key(escape);
    await selectionIs([], null, "the selection cleared by Escape");
    step(
      "the clear button ended the selection; Shift+click selected the hosts between; Escape in the tree cleared them",
    );

    // A right-click outside the selection selects that row alone, with
    // its own menu.
    await pick(odd, r1);
    await clickOn(row(telnet), { button: "right" });
    await selectionIs([telnet], null, "jobs-telnet selected alone");
    const own = await entries();
    if (!own.includes("connect") || (await exists(`${menu}[data-menu-hosts]`)))
      fail(`a row outside the selection got the menu ${json(own)}`);
    await key(escape);
    await until("the menu closed", async () => !(await exists(menu)));
    step("a right-click outside the selection selected that row alone");

    // Ctrl on a folder is a plain click; a drag ends the selection and
    // moves nothing (dropped back on its own folder).
    await pick(odd, r1);
    await clickOn(folderRow, { modifiers: CTRL });
    await selectionIs([], null, "the folder selected instead");
    if (
      !(await evaluate(
        `document.querySelector(${json(folderRow)}).classList.contains("selected")`,
      ))
    )
      fail("Ctrl+click on a folder did not select it");
    await expandFolder(evaluate, folderRow);
    await pick(odd, r1);
    const from = await reachable(evaluate, row(r1));
    const send = (type, at, extra = {}) =>
      devtools.send("Input.dispatchMouseEvent", {
        type,
        x: at.x,
        y: at.y,
        button: "left",
        ...extra,
      });
    await send("mousePressed", from, { clickCount: 1 });
    for (let i = 1; i <= 5; i++) {
      await send(
        "mouseMoved",
        { x: from.x + i * 8, y: from.y },
        { buttons: 1 },
      );
    }
    await send(
      "mouseReleased",
      { x: from.x + 40, y: from.y },
      { clickCount: 1 },
    );
    await selectionIs([], null, "the drag ended the selection");
    if (!existsSync(fileOf(r1))) fail("the drag moved jobs-r1");
    step(
      "Ctrl+click on a folder selected the folder alone; a drag ended the selection",
    );

    // Under the filter field, Shift+click selects only the hosts shown.
    await filterWords("zebrafish");
    await until(
      "the filter showing jobs-odd and jobs-telnet only",
      async () =>
        (await exists(row(odd))) &&
        (await exists(row(telnet))) &&
        !(await exists(row(r1))),
    );
    await clickOn(row(odd));
    await clickOn(row(telnet), { modifiers: SHIFT });
    await selectionIs([odd, telnet], "2", "the shown range selected");
    await filterWords("");
    await until("every host back", () => exists(row(r1)));
    await selectionIs([odd, telnet], "2", "the selection kept");
    step(
      "under the filter field, Shift+click selected only the hosts it shows",
    );

    // Selected hosts deleted on disk leave the selection.
    await clickOn(row(odd));
    await clickOn(row(telnet), { modifiers: SHIFT });
    await selectionIs([odd, r1, telnet], "3", "all three selected");
    rmSync(fileOf(telnet));
    await selectionIs([odd, r1], "2", "jobs-telnet gone from the selection");
    if (await exists(row(telnet))) fail("the deleted host is still shown");
    rmSync(fileOf(odd));
    await selectionIs([r1], null, "jobs-r1 left selected alone");
    await screenshot("host-selection-reload");
    step(
      "selected hosts deleted on disk left the selection (the last one stays selected alone)",
    );
  } finally {
    for (const [id, text] of Object.entries(originals))
      writeFileSync(fileOf(id), text);
    if (savedSnippets === null) rmSync(snippetsFile, { force: true });
    else writeFileSync(snippetsFile, savedSnippets);
  }
  await until(
    "jobs-odd and jobs-telnet back",
    async () => (await exists(row(odd))) && (await exists(row(telnet))),
  );
}

/**
 * Saved jobs (after `jobs`, which leaves the Jobs folder with jobs-r1
 * trusted and its profile): saved from pre-flight, run in one click from
 * the panel, saved from a past run, renamed and deleted; an entry whose
 * filter is gone says so instead of running, and a bad one is a problem.
 */
/**
 * Two job tabs at pre-flight at once (#121, after `jobs` and `savedJobs`:
 * the device's key is trusted, the profile `jobs-login` exists): one on
 * the production host `jobs-r1`, planned first, one on a lab host of a
 * folder of its own, planned last. Each Start runs its own tab's plan (the
 * production one still asks first), the shell refuses to start the
 * production plan without the confirmation, and a plan that started cannot
 * start again.
 */
async function twoPreflights({ evaluate, screenshot, step }) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const dialog = JOB_TAB;
  const folder = join(workspace, "TwoJobs");
  const snippetsFile = join(workspace, "_snippets.yaml");
  const savedSnippets = existsSync(snippetsFile)
    ? readFileSync(snippetsFile, "utf8")
    : null;
  const prodHost = "Jobs/jobs-r1.yaml";
  const labHost = "TwoJobs/two-lab.yaml";
  /** The tab id and plan id of the job tab shown. */
  const shown = () =>
    evaluate(`(() => {
      const view = document.querySelector(${json(dialog)});
      return view ? { tab: view.closest(".slot").dataset.tab, plan: view.dataset.plan ?? null } : null;
    })()`);
  /** Brings the tab `tab` to the front by its header. */
  const showTab = async (tab) => {
    await clickOn(`[data-tab-bar] [data-tab="${tab}"]`);
    await until(`the job tab ${tab} in front`, async () => {
      const now = await shown();
      return now?.tab === tab;
    });
  };
  /** Opens a folder's run-snippet tab and plans `snippet` to pre-flight. */
  const plan = async (path, snippet) => {
    await showSideView(evaluate, "hosts");
    await clickOn(`nav button[data-item=folder][data-path=${json(path)}]`);
    await showDetails(evaluate);
    await until(`the ${path} folder's side panel`, () =>
      exists(".side [data-folder-run-snippet]"),
    );
    const before = (await shown())?.tab ?? null;
    await clickOn(".side [data-folder-run-snippet]");
    await until(`a new run-snippet tab for ${path}`, async () => {
      const now = await shown();
      return (
        now !== null &&
        now.tab !== before &&
        (await exists(`${dialog}[data-step=snippet]`))
      );
    });
    await evaluate(`(() => {
      const select = document.querySelector("${dialog} [name=job-snippet]");
      select.value = ${json(snippet)};
      select.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
    await clickOn(`${dialog} [data-job=plan]`);
    await until(`the pre-flight of ${snippet}`, () =>
      exists(`${dialog}[data-step=preflight][data-plan]`),
    );
    return shown();
  };
  /** What the shell answers a start of `plan` sent straight to it. */
  const startDirectly = async (plan, confirmedProduction) => {
    await evaluate(`(() => {
      window.__smokeStart = null;
      window.__TAURI_INTERNALS__
        .invoke("job_start", ${json({ plan: Number(plan), confirmedProduction, parallel: 1, label: "smoke" })})
        .then((value) => (window.__smokeStart = { value }), (error) => (window.__smokeStart = { error: String(error) }));
    })()`);
    return until("the shell's answer to the start", () =>
      evaluate("window.__smokeStart"),
    );
  };
  const result = async (host) => {
    await until("the job finished", () =>
      exists(`${dialog} [data-job-progress][data-finished]`),
    );
    return evaluate(`(() => {
      const rows = [...document.querySelectorAll("${dialog} [data-job-result]")];
      const row = rows.find((r) => r.dataset.jobResult === ${json(host)});
      return {
        hosts: rows.map((r) => r.dataset.jobResult),
        state: row?.dataset.hostState ?? null,
        output: row?.querySelector("[data-job-output]")?.textContent ?? "",
      };
    })()`);
  };

  mkdirSync(folder, { recursive: true });
  writeFileSync(
    join(folder, "_folder.yaml"),
    `platform: frr\ncredential_profile: jobs-login\nport: ${DEVICE_PORT}\n`,
  );
  writeFileSync(
    join(folder, "two-lab.yaml"),
    "address: 127.0.0.1\nenvironment: lab\n",
  );
  writeFileSync(
    snippetsFile,
    `${savedSnippets ?? ""}Two prod: echo prod-plan\nTwo lab: echo lab-plan\n`,
  );
  try {
    await until("the TwoJobs folder in the tree", () =>
      exists('nav button[data-item=folder][data-path="TwoJobs"]'),
    );
    await showSideView(evaluate, "snippets");
    await until("the two snippets loaded", () =>
      exists('[data-snippet="Two lab"]'),
    );

    // The production tab plans first, the lab tab last.
    const prod = await plan("Jobs", "Two prod");
    if (!(await exists(`${dialog} [data-job-production]`))) {
      fail("the production pre-flight did not ask about jobs-r1");
    }
    const lab = await plan("TwoJobs", "Two lab");
    if (await exists(`${dialog} [data-job-production]`)) {
      fail("the lab pre-flight asked about production");
    }
    if (prod.plan === lab.plan) fail(`one plan id for both tabs: ${prod.plan}`);
    await screenshot("two-preflights");

    // Back in the production tab: it still asks, and the shell refuses a
    // start of its plan that does not say production was confirmed.
    await showTab(prod.tab);
    const startDisabled = await evaluate(
      `document.querySelector("${dialog} [data-job=start]").disabled`,
    );
    if (!startDisabled) fail("the production tab's Start did not wait");
    const unconfirmed = await startDirectly(prod.plan, []);
    if (!unconfirmed.error?.includes("jobs-r1 was not confirmed")) {
      fail(`the shell's answer to an unconfirmed start: ${json(unconfirmed)}`);
    }
    // Confirming the lab tab's (no) production confirms nothing here.
    const labConfirmation = await startDirectly(prod.plan, [labHost]);
    if (!labConfirmation.error?.includes("not confirmed")) {
      fail(`another plan's confirmation counted: ${json(labConfirmation)}`);
    }
    await clickOn(`${dialog} [name=job-confirm-production]`);
    await until(
      "the production tab's Start enabled",
      async () =>
        !(await evaluate(
          `document.querySelector("${dialog} [data-job=start]").disabled`,
        )),
    );
    await clickOn(`${dialog} [data-job=start]`);
    const ranProd = await result(prodHost);
    if (
      json(ranProd.hosts) !== json([prodHost]) ||
      ranProd.state !== "done" ||
      !ranProd.output.includes("lab-r1# echo prod-plan")
    ) {
      fail(`the production tab ran: ${json(ranProd)}`);
    }
    step(
      "with two job tabs at pre-flight, Start in the one planned first ran its own plan, after its production question",
    );

    // The lab tab runs its own plan too.
    await showTab(lab.tab);
    await clickOn(`${dialog} [data-job=start]`);
    const ranLab = await result(labHost);
    if (
      json(ranLab.hosts) !== json([labHost]) ||
      ranLab.state !== "done" ||
      !ranLab.output.includes("lab-r1# echo lab-plan")
    ) {
      fail(`the lab tab ran: ${json(ranLab)}`);
    }
    // A plan that started is used up.
    const again = await startDirectly(prod.plan, [prodHost]);
    if (!again.error?.includes("plan the job again")) {
      fail(`a started plan started again: ${json(again)}`);
    }
    step(
      "the other tab ran its own plan, and the shell refuses an unconfirmed production plan and a plan already started",
    );
    await clickOn(`${dialog} [data-job=close]`);
    await showTab(prod.tab);
    await clickOn(`${dialog} [data-job=close]`);
    await until("both job tabs closed", async () => !(await exists(dialog)));
  } finally {
    rmSync(folder, { recursive: true, force: true });
    if (savedSnippets === null) rmSync(snippetsFile, { force: true });
    else writeFileSync(snippetsFile, savedSnippets);
  }
}

async function savedJobs({ evaluate, fillForm, text, screenshot, step }) {
  const json = (value) => JSON.stringify(value);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const dialog = JOB_TAB;
  const atStep = (name) => exists(`${dialog}[data-step=${name}]`);
  const view = RUN_TAB;
  const jobsFile = join(workspace, "_jobs.yaml");
  const jobsText = () =>
    existsSync(jobsFile) ? readFileSync(jobsFile, "utf8") : "";
  const snippetsFile = join(workspace, "_snippets.yaml");
  const savedSnippets = existsSync(snippetsFile)
    ? readFileSync(snippetsFile, "utf8")
    : null;
  const savedJob = (name) => `[data-saved-job=${json(name)}]`;
  const dialogButton = (label) =>
    `[...document.querySelectorAll("[role=dialog] button")].find((b) => b.textContent.trim() === ${json(label)})`;

  if (existsSync(jobsFile)) fail("the workspace already has a jobs file");
  writeFileSync(
    snippetsFile,
    `${savedSnippets ?? ""}Saved check: show hostname\nSaved secret: "echo vlan {{vlan}} {{enable_password}}"\n`,
  );
  try {
    // The saved jobs are in the Jobs side view, the snippets in theirs.
    await showSideView(evaluate, "jobs");
    await until("the saved jobs panel, empty", () =>
      exists("[data-saved-jobs] .hint"),
    );
    await showSideView(evaluate, "snippets");
    await until("the snippet for saved jobs", () =>
      exists('[data-snippet="Saved check"]'),
    );
    await showSideView(evaluate, "hosts");

    // Saved from pre-flight: a snippet on the Jobs folder.
    await clickOn('nav button[data-item=folder][data-path="Jobs"]');
    await showDetails(evaluate);
    await until("the Jobs folder's side panel", () =>
      exists(".side [data-folder-run-snippet]"),
    );
    await clickOn(".side [data-folder-run-snippet]");
    await until("the run-snippet tab", () => atStep("snippet"));
    await evaluate(`(() => {
      const select = document.querySelector("${dialog} [name=job-snippet]");
      select.value = "Saved check";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
    await clickOn(`${dialog} [data-job=plan]`);
    await until("the pre-flight list", () => atStep("preflight"));
    await clickOn(`${dialog} [data-save-job=open]`);
    await until("the save-as-job form", () =>
      exists(`${dialog} [data-save-job-form]`),
    );
    const offered = await evaluate(
      `document.querySelector("${dialog} [name=save-job-name]").value`,
    );
    if (offered !== "Saved check on Jobs") {
      fail(`the name offered is ${json(offered)}`);
    }
    await fillForm(
      { "save-job-name": "Jobs check" },
      `${dialog} [data-save-job-form]`,
    );
    await until("the job saved", () =>
      exists(`${dialog} [data-save-job-saved="Jobs check"]`),
    );
    if (
      jobsText() !== "Jobs check:\n  snippet: Saved check\n  folder: Jobs\n"
    ) {
      fail(`the jobs file holds ${json(jobsText())}`);
    }
    await clickOn(`${dialog} [data-job=dismiss]`);
    await until("the job tab closed", async () => !(await exists(dialog)));
    step("a job was saved from its pre-flight list into the workspace");

    // A variable whose name looks like a secret (#126): the save form says
    // it is asked at each run, the file keeps only the other answer, and
    // the saved job asks it again.
    await clickOn('nav button[data-item=folder][data-path="Jobs"]');
    await showDetails(evaluate);
    await until("the Jobs folder's side panel", () =>
      exists(".side [data-folder-run-snippet]"),
    );
    await clickOn(".side [data-folder-run-snippet]");
    await until("the run-snippet tab", () => atStep("snippet"));
    await evaluate(`(() => {
      const select = document.querySelector("${dialog} [name=job-snippet]");
      select.value = "Saved secret";
      select.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
    await until("the secret snippet's text shown", async () =>
      (await text(`${dialog} [data-job-snippet-text]`)).includes(
        "{{enable_password}}",
      ),
    );
    await clickOn(`${dialog} [data-job=plan]`);
    await until("the variables step", () => atStep("variables"));
    const variables = await evaluate(
      `[...document.querySelectorAll("${dialog} [data-variable]")].map((i) => [i.name, i.dataset.variable])`,
    );
    const typed = { vlan: "42", enable_password: "smoke-never-kept" };
    if (
      json(variables.map(([, v]) => v).sort()) !==
      json(["enable_password", "vlan"])
    ) {
      fail(`the secret snippet asked ${json(variables)}`);
    }
    await fillForm(
      Object.fromEntries(variables.map(([input, v]) => [input, typed[v]])),
      `${dialog} form`,
    );
    await until("the pre-flight list", () => atStep("preflight"));
    await clickOn(`${dialog} [data-save-job=open]`);
    await until("the save-as-job form", () =>
      exists(`${dialog} [data-save-job-form]`),
    );
    const asked = await evaluate(
      `document.querySelector("${dialog} [data-save-job-asked]")?.dataset.saveJobAsked ?? null`,
    );
    if (asked !== "enable_password") {
      fail(
        `the save form does not say enable_password is asked: ${json(asked)}`,
      );
    }
    await fillForm(
      { "save-job-name": "Secret check" },
      `${dialog} [data-save-job-form]`,
    );
    await until("the secret job saved", () =>
      exists(`${dialog} [data-save-job-saved="Secret check"]`),
    );
    if (
      !jobsText().includes(
        "Secret check:\n  snippet: Saved secret\n  answers:\n    vlan: '42'\n  folder: Jobs\n",
      ) ||
      jobsText().includes("enable_password") ||
      jobsText().includes(typed.enable_password)
    ) {
      fail(`the jobs file holds ${json(jobsText())}`);
    }
    await clickOn(`${dialog} [data-job=dismiss]`);
    await until("the job tab closed", async () => !(await exists(dialog)));
    await showSideView(evaluate, "jobs");
    await until("the secret job listed", () =>
      exists(savedJob("Secret check")),
    );
    await clickOn(savedJob("Secret check"));
    await until("the secret job asks its variable", () => atStep("variables"));
    const askedAgain = await evaluate(
      `[...document.querySelectorAll("${dialog} [data-variable]")].map((i) => i.dataset.variable)`,
    );
    if (json(askedAgain) !== json(["enable_password"])) {
      fail(`the saved secret job asks ${json(askedAgain)}`);
    }
    await closeTabOf(evaluate, dialog);
    await showSideView(evaluate, "hosts");
    step(
      "a secret-looking variable's answer was not saved: the form said so and the saved job asks it at each run",
    );

    // One click in the panel: pre-flight first, then it runs.
    await showSideView(evaluate, "jobs");
    await until("the saved job listed", () => exists(savedJob("Jobs check")));
    await clickOn(savedJob("Jobs check"));
    await until("the saved job's pre-flight", () => atStep("preflight"));
    const runs = await evaluate(
      `[...document.querySelectorAll("${dialog} [data-job-host]")].map((r) => r.dataset.jobHost)`,
    );
    if (json(runs) !== json(["Jobs/jobs-r1.yaml"])) {
      fail(`the saved job would run on ${json(runs)}`);
    }
    if (!(await exists(`${dialog} [data-job-production]`))) {
      fail("the saved job did not ask about the production host");
    }
    await screenshot("saved-job-preflight");
    await clickOn(`${dialog} [name=job-confirm-production]`);
    await until(
      "the start button enabled",
      async () =>
        !(await evaluate(
          `document.querySelector("${dialog} [data-job=start]").disabled`,
        )),
    );
    await clickOn(`${dialog} [data-job=start]`);
    await until("the saved job finished", () =>
      exists(`${dialog} [data-job-progress][data-finished]`),
    );
    const result = await evaluate(`(() => {
      const row = document.querySelector('${dialog} [data-job-result="Jobs/jobs-r1.yaml"]');
      return { state: row.dataset.hostState, output: row.querySelector("[data-job-output]").textContent };
    })()`);
    if (
      result.state !== "done" ||
      !result.output.includes("lab-r1# show hostname")
    ) {
      fail(`the saved job's result: ${json(result)}`);
    }
    step("a saved job started from the panel showed pre-flight, then ran");

    // Saved from the past run, in its tab.
    await clickOn(`${dialog} [data-job=view]`);
    await until("the run in its tab", () =>
      exists(`${view} [data-run-details] [data-save-job=open]`),
    );
    await clickOn(`${view} [data-save-job=open]`);
    await until("the run tab's save form", () =>
      exists(`${view} [data-save-job-form]`),
    );
    await fillForm(
      { "save-job-name": "From a run" },
      `${view} [data-save-job-form]`,
    );
    await until("the run saved as a job", () =>
      exists(`${view} [data-save-job-saved="From a run"]`),
    );
    await closeTabOf(evaluate, ".slot[data-view-slot=run]");
    await until("the job saved from a run listed", () =>
      exists(savedJob("From a run")),
    );
    if (
      !jobsText().includes(
        "From a run:\n  snippet: Saved check\n  folder: Jobs\n",
      )
    ) {
      fail(`the jobs file holds ${json(jobsText())}`);
    }
    step("a past run was saved as a job from its tab");

    // Renamed and deleted from the panel.
    await clickOn('[data-saved-job-rename="From a run"]');
    await until("the rename dialog", () =>
      exists("[role=dialog] form [name=name]"),
    );
    await fillForm({ name: "Renamed job" }, "[role=dialog] form");
    await until("the renamed job", () => exists(savedJob("Renamed job")));
    if (
      !jobsText().includes("Renamed job:") ||
      jobsText().includes("From a run:")
    ) {
      fail(`the jobs file after the rename: ${json(jobsText())}`);
    }
    await clickOn('[data-saved-job-delete="Renamed job"]');
    await until("the delete confirmation", () =>
      evaluate(`!!${dialogButton("Delete")}`),
    );
    await mouseClickElement(evaluate, dialogButton("Delete"), "Delete");
    await until(
      "the deleted job gone",
      async () => !(await exists(savedJob("Renamed job"))),
    );
    if (jobsText().includes("Renamed job:")) fail("the deleted job is on disk");
    step("a saved job was renamed and deleted from the panel");

    // Edited outside the app: picked up; a gone filter says so, a bad
    // entry is a problem.
    writeFileSync(
      jobsFile,
      `${jobsText()}Gone filter:\n  snippet: Saved check\n  filter: Nowhere\nBroken job: {}\nOld secret:\n  snippet: Saved secret\n  answers:\n    vlan: '7'\n    enable_password: smoke-old-answer\n  folder: Jobs\n`,
    );
    await until("the edited jobs file picked up", () =>
      exists('[data-saved-job-problem="Gone filter"]'),
    );
    // An answer that looks like a secret, already in the file (#126): the
    // job says it is no longer used, never shows it, and the file is left
    // as it was.
    await until("the ignored answer named", () =>
      exists('[data-saved-job-ignored="enable_password"]'),
    );
    const ignored = await text('[data-saved-job-ignored="enable_password"]');
    if (
      !ignored.includes("no longer used") ||
      !ignored.includes("cannot remove it from the git history")
    ) {
      fail(`the ignored answer's notice reads ${json(ignored)}`);
    }
    if ((await text("[data-saved-jobs]")).includes("smoke-old-answer")) {
      fail("the ignored answer is shown");
    }
    if (!jobsText().includes("enable_password: smoke-old-answer")) {
      fail("the jobs file was rewritten");
    }
    step(
      "an answer that looks like a secret in the jobs file is named as no longer used, and left in the file",
    );
    if (
      !(await text('[data-saved-job-problem="Gone filter"]')).includes(
        "Nowhere",
      )
    ) {
      fail("the gone filter's job does not name the filter");
    }
    if (await exists(savedJob("Broken job"))) fail("a bad entry was listed");
    // The problem is at the foot of the Hosts view (and marks its icon).
    await until("the Hosts icon marked", () =>
      exists("[data-rail=hosts][data-problems]"),
    );
    await showSideView(evaluate, "hosts");
    await clickOn("[data-problems]");
    await until("the bad entry listed as a problem", async () =>
      (await text("[data-problems-list]")).includes("saved job `Broken job`"),
    );
    await showSideView(evaluate, "jobs");
    await clickOn(savedJob("Gone filter"));
    await until("the refusal", async () =>
      (await text("[data-saved-jobs-error]")).includes(
        "there is no saved filter called Nowhere",
      ),
    );
    if (await exists(dialog)) fail("a saved job whose filter is gone opened");
    await screenshot("saved-jobs");
    step(
      "an external edit was picked up: a job whose filter is gone says so, a bad entry is a problem",
    );
  } finally {
    rmSync(jobsFile, { force: true });
    if (savedSnippets === null) rmSync(snippetsFile, { force: true });
    else writeFileSync(snippetsFile, savedSnippets);
  }
  await until(
    "the saved jobs gone",
    async () => !(await exists("[data-saved-job]")),
  );
  await showSideView(evaluate, "hosts");
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
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const openName = () => text("[data-open-workspaces] span");
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
  // A second one, moved to a session window (#88): switching treats it
  // as those of the main window.
  writeFileSync(
    join(workspace, "ws-r2.yaml"),
    `address: 127.0.0.1\nport: ${SSH_PORT}\n`,
  );
  const secondHost = '[data-host="ws-r2.yaml"]';
  await until("the second host in the tree", () => exists(secondHost));
  await doubleClick(secondHost);
  await until("the credentials prompt", async () =>
    (await text("main h2")).includes("ws-r2"),
  );
  await fillForm({ username: USERNAME, password: PASSWORD });
  await until(
    "the second Connected status",
    async () => (await status()) === "Connected",
  );
  const away = await moveToNewWindow(evaluate, "ws-r2");
  away.devtools.close();
  await mouseClick(evaluate, `[data-tab-bar] [data-tab="${tab}"]`);
  await until("ws-r1 shown again", () => exists(`${slot}[data-active]`));
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
  // Earlier parts left sessions to its hosts open too; one is in a
  // session window (#88).
  if (
    !/\d+ sessions? to hosts of smoke-workspace[\s\S]*\(1 in session windows\)/.test(
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
  // An empty workspace says how to add a host, and offers the ways.
  await until("the empty workspace's offer", () =>
    exists("[data-side-view=hosts] [data-hosts-empty]"),
  );
  for (const kind of ["host", "csv", "nornir"]) {
    if (!(await exists(`[data-hosts-empty] [data-hosts-empty-add=${kind}]`))) {
      fail(`the empty workspace does not offer ${kind}`);
    }
  }
  await screenshot("empty-workspace");
  await clickOn("[data-hosts-empty-add=host]");
  await until("the new host form", () =>
    exists("[role=dialog] input[name=name]"),
  );
  // A first host as other tools write it: a port after the address is
  // refused with where the port goes; no name is said plainly (#116).
  await fillForm(
    { name: "edge1", address: "10.0.0.1:2222" },
    "[role=dialog] form",
  );
  await until("the address refused", async () =>
    (await text("[role=dialog] [role=alert]")).includes(
      "write the address without the port (10.0.0.1), then set Port to 2222",
    ),
  );
  await fillForm({ name: " ", address: "10.0.0.1" }, "[role=dialog] form");
  await until(
    "the empty name refused",
    async () =>
      (await text("[role=dialog] [role=alert]")) === "the name is empty",
  );
  if (existsSync(join(second, "edge1.yaml"))) {
    fail("a refused new host was written");
  }
  await clickButton(evaluate, "Cancel", "[role=dialog]");
  await until(
    "the new host form closed",
    async () => !(await exists("[role=dialog] input[name=name]")),
  );
  if (!(await exists(slot))) fail("the kept session's tab is gone");
  if ((await text(`${slot} [role=status]`)) !== "Connected") {
    fail(`the kept session is ${await text(`${slot} [role=status]`)}`);
  }
  // Its host's role comes from its own workspace, which defines it.
  if ((await sessionRole()) !== "router") {
    fail(`the kept session's role icon is ${await sessionRole()}`);
  }
  const keptAway = await connectWindow(away.label);
  const keptThere = await tabsIn(keptAway.evaluate);
  keptAway.devtools.close();
  if (
    !keptThere.some(
      (each) => each.name === "ws-r2" && each.state === "connected",
    )
  )
    fail(`the kept session window shows ${JSON.stringify(keptThere)}`);
  step(
    "a new empty workspace was created and opened; the live sessions were kept, one in a session window",
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
  await until(
    "the session window closed with its session",
    async () => !(await windowOpen(away.label)),
  );
  step(
    "switching with live sessions closed them when asked to, the session window's too",
  );

  // A folder of other things typed in the switcher is asked about first
  // (#131): Escape cancels the question, not the switcher, and nothing is
  // written; then answered yes it becomes a workspace, its one file
  // written, and it opens.
  const documents = otherThingsFolder();
  const documentsBefore = entriesBelow(documents);
  await openSwitcher();
  await fillForm(
    { "workspace-folder": documents },
    "[data-workspace-form=add]",
  );
  await until("the question about a folder of other things", () =>
    exists("[data-confirm-open-folder] [data-confirm-folder=cancel]"),
  );
  await checkOpenFolderQuestion(evaluate, text, documents);
  if (json(entriesBelow(documents)) !== json(documentsBefore))
    fail("the folder was written into before the answer");
  await devtools.send("Input.dispatchKeyEvent", { type: "keyDown", ...escape });
  await devtools.send("Input.dispatchKeyEvent", { type: "keyUp", ...escape });
  await until(
    "the question gone",
    async () => !(await exists("[data-confirm-open-folder]")),
  );
  if (!(await exists("[data-dialog=workspaces]")))
    fail("Escape on the question closed the switcher");
  if ((await openName()) !== "customer-b")
    fail(`after Cancel the open workspace is ${await openName()}`);
  await sleep(500);
  if (json(entriesBelow(documents)) !== json(documentsBefore))
    fail("the folder was written into after Cancel");
  await clickOn("[data-workspace-form=add] button[type=submit]");
  await until("the question again", () =>
    exists("[data-confirm-open-folder] [data-confirm-folder=open]"),
  );
  await clickOn("[data-confirm-folder=open]");
  await until(
    "the folder open as a workspace",
    async () => (await openName()) === "documents",
  );
  if (!idOf(documents)) fail("the folder made a workspace has no identifier");
  const written = Object.keys(entriesBelow(documents)).filter(
    (path) => !(path in documentsBefore),
  );
  if (json(written) !== json([join(documents, "_workspace.yaml")]))
    fail(`making the folder a workspace wrote ${json(written)}`);
  // Back to customer-b, and the folder off the list.
  await openSwitcher();
  await clickOn(`[data-workspace=${json(second)}]`);
  await until(
    "customer-b open again",
    async () => (await openName()) === "customer-b",
  );
  await openSwitcher();
  await clickOn(`[data-workspace=${json(documents)}] [data-workspace-remove]`);
  await until(
    "the folder off the list",
    async () => json(await known()) === json([`${second} (open)`, workspace]),
  );
  await devtools.send("Input.dispatchKeyEvent", { type: "keyDown", ...escape });
  await devtools.send("Input.dispatchKeyEvent", { type: "keyUp", ...escape });
  await until(
    "the switcher closed",
    async () => !(await exists("[data-dialog=workspaces]")),
  );
  step(
    "a folder of other things typed in the switcher is asked about (Escape cancels it, nothing written), and made a workspace on yes",
  );

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
    (await text("[data-workspace-error]")).includes(
      "not empty; open it as a workspace instead",
    ),
  );
  // The same words as the welcome view: open, not add (#116).
  if (
    (await text("[data-workspace-form=add] label")) !==
      "Open an existing folder" ||
    (await text("[data-workspace-form=add] button[type=submit]")) !== "Open"
  ) {
    fail(`the switcher offers: ${await text("[data-workspace-form=add]")}`);
  }
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
  const clickOn = (selector) => mouseClick(evaluate, selector);
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
      await expandFolder(evaluate, row);
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
    // The tree shows each host's environment before connecting: a label
    // for production, a mark for lab.
    await until("the lab host's mark in the tree", () =>
      exists('[data-host="lab-r9.yaml"] [data-host-environment=lab]'),
    );
    if (await exists('nav button[data-item=folder][data-path="Prod"]'))
      await expandFolder(
        evaluate,
        'nav button[data-item=folder][data-path="Prod"]',
      );
    await until("the production host's label in the tree", async () =>
      (
        await text(
          '[data-host="Prod/prod-r1.yaml"][data-environment=production] [data-host-environment=production]',
        )
      ).includes("PROD"),
    );
    step("the tree shows each host's environment");
    // Turned off: from now on only production hosts ask.
    await openPreferences(evaluate);
    await evaluate(`(() => {
      const field = document.querySelector("[data-view=preferences] [name=preference-confirm-multi-line-paste]");
      field.checked = false;
      field.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
    await until("the confirmation switch saved", () =>
      /confirm_multi_line_paste: false/.test(
        readFileSync(preferencesFile, "utf8"),
      ),
    );
    await closePreferences(evaluate);
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
      !/^rgb\((229, 72, 77|110, 15, 15)\)$/.test(marks.borderColor) ||
      marks.tabBackground === "rgba(0, 0, 0, 0)"
    ) {
      fail(`the production session is not marked: ${json(marks)}`);
    }
    await checkHeaderLine(evaluate, SHOWN_SESSION, [
      "[data-environment-badge=production]",
    ]);
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
    await showSideView(evaluate, "snippets");
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
    await showSideView(evaluate, "hosts");
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

/**
 * What protects the engineer follows a session into its window (#86),
 * after pasteSafety (its production prod-r1 and lab-r9 open, the paste
 * confirmation preference off): the production marks, a multi-line paste
 * confirmed over the pane with the host's line delay, snippets from the
 * main window's Snippets view to the terminal used last (named first;
 * the production question and the variables asked over that pane, in
 * that window, which comes forward), a reconnect's host-key question
 * asked and answered there, and Details opening the sheet in the main
 * window. The session is docked back at the end.
 */
async function safetyInSessionWindows({
  evaluate,
  devtools,
  screenshot,
  step,
}) {
  const json = (value) => JSON.stringify(value);
  const CTRL = 2;
  const SHIFT = 8;
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const tabs = () =>
    evaluate(`[...document.querySelectorAll("[data-tab-bar] [role=tab]")].map((tab) => ({
      id: Number(tab.dataset.tab),
      name: tab.querySelector("[data-tab-name]")?.textContent ?? "",
    }))`);
  const press = (client, key, code, windowsVirtualKeyCode, modifiers = 0) =>
    (async () => {
      const event = { key, code, windowsVirtualKeyCode, modifiers };
      await client.send("Input.dispatchKeyEvent", {
        type: "keyDown",
        ...event,
      });
      await client.send("Input.dispatchKeyEvent", { type: "keyUp", ...event });
    })();
  const typeIn = async (client, text) => {
    for (const char of text) {
      const key =
        char === "\r"
          ? { key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 }
          : { key: char };
      await client.send("Input.dispatchKeyEvent", {
        type: "keyDown",
        text: char,
        ...key,
      });
      await client.send("Input.dispatchKeyEvent", { type: "keyUp", ...key });
    }
  };
  const sessionLabels = async () => {
    const labels = [];
    for (const target of await pageTargets()) {
      const client = await connectTarget(target);
      const label = await evaluateIn(client)(
        "window.__TAURI_INTERNALS__?.metadata?.currentWindow?.label ?? null",
      ).catch(() => null);
      client.close();
      if (label?.startsWith("session-")) labels.push(label);
    }
    return labels;
  };
  const shown = SHOWN_SESSION;

  const prod = (await tabs()).find((tab) => tab.name === "prod-r1");
  if (!prod)
    fail(`safetyInSessionWindows needs prod-r1: ${json(await tabs())}`);
  await clickOn(`[data-tab-bar] [data-tab="${prod.id}"]`);
  await until("prod-r1 shown", () =>
    exists(`.slot[data-tab="${prod.id}"][data-active]`),
  );
  const before = new Set(await sessionLabels());
  await sessionAction(evaluate, "open-in-window", shown);
  const label = await until("the new session window", async () =>
    (await sessionLabels()).find((each) => !before.has(each)),
  );
  const window = await connectWindow(label);
  const there = window.evaluate;
  await until("prod-r1 in its window", () =>
    there(`!!document.querySelector(".slot[data-active] .terminal")`),
  );

  // Its production marks: the red edge, PROD on its tab, the badge.
  const marks = await there(`(() => {
    const tab = document.querySelector("[data-window-bar] [role=tab]");
    const session = document.querySelector("${shown} .session");
    const style = getComputedStyle(session);
    return {
      tab: tab?.dataset.environment,
      label: !!tab?.querySelector("[data-tab-production]"),
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
    !/^rgb\((229, 72, 77|110, 15, 15)\)$/.test(marks.borderColor)
  )
    fail(`prod-r1 lost its production marks in its window: ${json(marks)}`);
  step(`prod-r1 kept its red edge, PROD mark and badge in ${label}`);

  const screenThere = () => screenOf(there, shown);
  const ranThere = async (marker) =>
    new RegExp(`(^|\\n|\\$ )${marker}\\s*(\\n|$)`).test(await screenThere());
  const confirmation = () =>
    there(`(() => {
      const card = document.querySelector("${shown} [data-paste-confirm]");
      return card && {
        environment: card.dataset.pasteConfirm,
        lines: card.querySelector("[data-paste-lines]").dataset.pasteLines,
        text: card.querySelector("[data-paste-text]").textContent.replace(/\\r\\n/g, "\\n"),
      };
    })()`);
  // Blurred first: the window coming forward gives the terminal focus
  // again (focusin) even when it had it; DevTools never moves the OS focus.
  const focusThere = () =>
    there(`(() => {
      const field = document.querySelector("${shown} .terminal textarea");
      field.blur();
      field.focus();
    })()`);

  const savedClipboard = readClipboard();
  const knownHosts = join(appData, "known_hosts");
  const savedKnownHosts = readFileSync(knownHosts, "utf8");
  const snippetsFile = join(workspace, "_snippets.yaml");
  try {
    // A multi-line paste there asks there, over the pane, then goes out
    // with the folder's 400 ms between lines.
    writeClipboard("later 0 win-one\nlater 0 win-two\n");
    await focusThere();
    await press(window.devtools, "V", "KeyV", 86, CTRL | SHIFT);
    const asked = await until("the paste question in the window", confirmation);
    if (
      json(asked) !==
      json({
        environment: "production",
        lines: "2",
        text: "later 0 win-one\nlater 0 win-two\n",
      })
    )
      fail(`the paste question in the window shows ${json(asked)}`);
    if (await exists("[data-paste-confirm]"))
      fail("the paste question showed in the main window");
    const started = Date.now();
    await mouseClick(there, `${shown} [data-paste=send]`);
    await until(
      "the pasted lines run there",
      async () => (await ranThere("win-one")) && (await ranThere("win-two")),
    );
    const took = Date.now() - started;
    if (took < 400) fail(`two lines 400 ms apart arrived in ${took} ms`);
    step(
      `a multi-line paste in the session window was confirmed over its pane (production) and sent ${took} ms apart (delay 400 ms)`,
    );

    // The Snippets view names the terminal used last, in that window.
    writeFileSync(
      snippetsFile,
      'Win lines: "later 0 win-snip-1\\nlater 0 win-snip-2\\n"\nWin var: "later 0 win-{{word}}\\n"\n',
    );
    await showSideView(evaluate, "snippets");
    await until("the snippets in the panel", () =>
      exists('[data-snippet="Win var"]'),
    );
    const target = () =>
      evaluate(
        `document.querySelector("[data-snippet-target]")?.dataset.snippetTarget ?? null`,
      );
    await focusThere();
    await until(
      "the Snippets view naming prod-r1",
      async () => (await target()) === "prod-r1",
    );
    await clickOn('[data-snippet="Win lines"]');
    const snippetAsked = await until(
      "the snippet's paste question in the window",
      confirmation,
    );
    if (
      snippetAsked.environment !== "production" ||
      snippetAsked.lines !== "2" ||
      snippetAsked.text !== "later 0 win-snip-1\nlater 0 win-snip-2\n"
    )
      fail(`the snippet's question in the window shows ${json(snippetAsked)}`);
    if (await exists("[data-paste-confirm]"))
      fail("the snippet's question showed in the main window");
    // The shell asks for the focus; Windows may refuse it to a process
    // that is not in the foreground (a user typing elsewhere): said, not
    // failed.
    const forward = await until(
      "the session window forward",
      () => there("document.hasFocus()"),
      3000,
    ).catch(() => false);
    if (!forward)
      step("(Windows did not let the session window take the focus)");
    await mouseClick(there, `${shown} [data-paste=send]`);
    await until(
      "the snippet's lines run there",
      async () =>
        (await ranThere("win-snip-1")) && (await ranThere("win-snip-2")),
    );
    step(
      "the Snippets view named prod-r1 in its window; a multi-line snippet was confirmed over that pane, in that window, which was asked to come forward",
    );

    // Its variables are asked there too.
    await clickOn('[data-snippet="Win var"]');
    const variables = await until("the variables asked in the window", () =>
      there(`(() => {
        const dialog = document.querySelector("${shown} [data-snippet-variables]");
        return dialog && [...dialog.querySelectorAll("[data-variable]")].map((i) => i.dataset.variable);
      })()`),
    );
    if (json(variables) !== json(["word"]))
      fail(`the window asked for ${json(variables)}`);
    if (await exists("[data-snippet-variables]"))
      fail("the variables were asked in the main window");
    await there(`(() => {
      const form = document.querySelector("${shown} [data-snippet-variables] form");
      const input = form.querySelector("[name=variable-0]");
      input.value = "filled";
      input.dispatchEvent(new Event("input", { bubbles: true }));
      form.requestSubmit();
    })()`);
    await until("the filled snippet run there", () => ranThere("win-filled"));
    step(
      "a snippet's variables were asked over the pane in the session window",
    );

    // The name follows the focus back to the main window (brought to the
    // front first, as a click would: a page behind holds focus events).
    await devtools.send("Page.bringToFront");
    await evaluate(
      `document.querySelector("${shown} .terminal textarea").focus()`,
    );
    // (The main window's shown session: whichever tab took prod-r1's place.)
    const here = await evaluate(`(() => {
      const tab = document.querySelector("${shown}")?.dataset.tab;
      return document.querySelector('[data-tab-bar] [data-tab="' + tab + '"] [data-tab-name]')?.textContent ?? null;
    })()`);
    await until(
      `the Snippets view naming ${here} (the main window's) again`,
      async () => {
        const now = await target();
        if (now === here) return true;
        throw new Error(
          `${json(now)}; focus ${json(await evaluate("document.hasFocus() && document.activeElement?.className"))}`,
        );
      },
    );
    step("the snippet target followed the focus back to the main window");
    rmSync(snippetsFile);
    await showSideView(evaluate, "hosts");

    // A reconnect there asks about the host key there (it was forgotten).
    writeFileSync(knownHosts, "");
    await focusThere();
    await typeIn(window.devtools, "drop\r");
    await until("the session closed there", () =>
      there(`!!document.querySelector("${shown} [data-session-ended]")`),
    );
    await focusThere();
    await press(window.devtools, "Enter", "Enter", 13);
    await until("the host-key question in the window", () =>
      there(`(() => {
        const card = document.querySelector("${shown} [data-host-key=first-use]");
        return !!card && !card.querySelector("button:disabled");
      })()`),
    );
    if (await exists("[data-host-key]"))
      fail("the host-key question showed in the main window");
    await mouseClick(there, `[data-decision=trustOnce]`);
    await until("reconnected there", () =>
      there(`!!document.querySelector("[data-tab-state=connected]")`),
    );
    step(
      "a reconnect in the session window asked about the host key there, and connected once answered",
    );
    if (screenshot) await screenshot("session-window-safety");

    // Details from its header opens the sheet in the main window.
    await sessionAction(there, "details", shown);
    await until("the details sheet in the main window", () =>
      exists("[data-side-view=hosts] [data-details-sheet]"),
    );
    const selected = await evaluate(
      `document.querySelector("nav [data-item][aria-pressed=true], nav [data-item].selected")?.dataset.host ?? null`,
    );
    if (selected !== null && selected !== "Prod/prod-r1.yaml")
      fail(`Details selected ${json(selected)}`);
    step(
      "Details on the session window's header opened the sheet in the main window",
    );

    // Joining the group there asks the production question there (#87).
    await sessionAction(there, "multi-send", shown);
    const joinAsked = await until(
      "the join question in the session window",
      () =>
        there(`(() => {
        const question = document.querySelector("[data-confirm-join]");
        return question && [...question.querySelectorAll("[data-join-production]")].map((each) => each.dataset.joinProduction);
      })()`),
    );
    if (json(joinAsked) !== json(["prod-r1"]))
      fail(`the join question named ${json(joinAsked)}`);
    if (await exists("[data-confirm-join]"))
      fail("the join question was asked in the main window");
    await mouseClick(there, `[data-join=cancel]`);
    await sleep(300);
    if (await there(`!!document.querySelector("[data-multi-send-bar]")`))
      fail("prod-r1 joined though the question was cancelled");
    await sessionAction(there, "multi-send", shown);
    await until("the join question again", () =>
      there(`!!document.querySelector("[data-confirm-join]")`),
    );
    await mouseClick(there, `[data-join=join]`);
    await until("prod-r1 in the group", () =>
      there(
        `document.querySelector("[data-multi-send-bar]")?.dataset.groupSize === "1"`,
      ),
    );
    await sessionAction(there, "multi-send", shown);
    await until(
      "prod-r1 out of the group",
      async () =>
        !(await there(`!!document.querySelector("[data-multi-send-bar]")`)),
    );
    step(
      "joining the group in the session window asked the production question there; Cancel kept it out, Add to group put it in",
    );
  } finally {
    writeClipboard(savedClipboard);
    writeFileSync(knownHosts, savedKnownHosts);
    rmSync(snippetsFile, { force: true });
  }

  // Back to the main window, as it came.
  await sessionAction(there, "dock", shown);
  window.devtools.close();
  await until(`${label} closed`, async () => !(await windowOpen(label)));
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

// Input is told apart by where it came from (#34): typed and pasted text
// are the user's own; the terminal's answers to a device's queries and
// mouse reports are not (multi-send must not share them).
async function inputOrigins({
  type,
  evaluate,
  mouse,
  centreOf,
  devtools,
  step,
}) {
  const terminal = `document.querySelector("${SHOWN_SESSION} .terminal")`;
  const recent = () => evaluate(`${terminal}.input.recent()`);
  await evaluate(`${terminal}.input.record()`);
  const ctrlC = async () => {
    const key = { key: "c", code: "KeyC", windowsVirtualKeyCode: 67 };
    await devtools.send("Input.dispatchKeyEvent", {
      type: "keyDown",
      modifiers: 2,
      ...key,
    });
    await devtools.send("Input.dispatchKeyEvent", {
      type: "keyUp",
      modifiers: 2,
      ...key,
    });
  };
  const found = (origin, pattern) => async () =>
    (await recent()).some(
      (input) => input.origin === origin && pattern.test(input.data),
    );

  // The device asks for the cursor position and the device attributes; the
  // terminal answers by itself.
  await type("query\r");
  await until(
    "the cursor-position report, as a reply",
    found("reply", /^\x1b\[\d+;\d+R$/),
  );
  await until(
    "the device-attributes answer, as a reply",
    found("reply", /^\x1b\[\?[\d;]*c$/),
  );
  await until("the typed command, as typed", found("typed", /^q$/));
  await until("Enter, as typed", found("typed", /^\r$/));
  // The shell echoed the answers onto its command line: that is what
  // sharing them would do to other sessions.
  await ctrlC();
  await until("Ctrl+C, as typed", found("typed", /^\x03$/));
  step("the terminal's answers to device queries are told from typed input");

  await evaluate(`(() => {
    const data = new DataTransfer();
    data.setData("text/plain", "pasted-34");
    ${terminal}.querySelector("textarea").dispatchEvent(
      new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }),
    );
  })()`);
  await until("the paste, as pasted", found("pasted", /pasted-34/));
  await ctrlC();
  step("pasted text counts as the user's input");

  await type("mouse on\r");
  await until("mouse reporting on", () =>
    evaluate(`${terminal}.xterm.modes.mouseTrackingMode === "vt200"`),
  );
  await mouseClick(evaluate, `${SHOWN_SESSION} .terminal`);
  await until(
    "the mouse reports, as mouse",
    found("mouse", /^\x1b\[<0;\d+;\d+[Mm]$/),
  );
  await ctrlC();
  await type("mouse off\r");
  await until("mouse reporting off", () =>
    evaluate(`${terminal}.xterm.modes.mouseTrackingMode === "none"`),
  );
  const misplaced = (await recent()).filter(
    ({ origin, data }) =>
      (origin === "typed" || origin === "pasted") && data.includes("\x1b"),
  );
  if (misplaced.length > 0) {
    fail(
      `terminal-generated input counted as the user's: ${JSON.stringify(misplaced)}`,
    );
  }
  step("mouse reports are not counted as the user's input");
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
  // This session's terminal, also while the Preferences pane is active.
  const sessionTab = await evaluate(
    `document.querySelector("${SHOWN_SESSION}").dataset.tab`,
  );
  const terminal = `document.querySelector('.slot[data-tab="${sessionTab}"] .terminal').xterm`;
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
  // Preferences open as a tab (#68), dragged here into a pane of its own
  // beside the terminal, so that each change shows on that terminal at once.
  const openPreferencesBeside = async ({ shortcuts = false } = {}) => {
    await openPreferences(evaluate);
    if (shortcuts) {
      // A view tab: its icon, no connection dot or state.
      const look = await evaluate(`(() => {
        const tab = document.querySelector("[data-tab-view=preferences]");
        return {
          state: tab.dataset.tabState ?? null,
          icon: !!tab.querySelector(".view-icon svg"),
          dot: !!tab.querySelector(".state"),
          name: tab.querySelector("[data-tab-name]").textContent,
        };
      })()`);
      if (
        look.state !== null ||
        !look.icon ||
        look.dot ||
        look.name !== "Preferences"
      )
        fail(`the Preferences tab looks like a session's: ${json(look)}`);
      // The tab shortcuts work on it: move, next, previous.
      const order = () =>
        evaluate(`(() => {
          const bar = document.querySelector("[data-tab-view=preferences]").closest("[data-tab-bar]");
          return [...bar.querySelectorAll("[data-tab]")].map((tab) => tab.dataset.tabView ?? tab.dataset.tab);
        })()`);
      const before = (await order()).indexOf("preferences");
      if (before < 1)
        fail("the Preferences tab did not open after the sessions");
      await press("PageUp", "PageUp", 33, CTRL | SHIFT);
      await until(
        "the Preferences tab moved left (Ctrl+Shift+PageUp)",
        async () => (await order()).indexOf("preferences") === before - 1,
      );
      const viewShown = () =>
        evaluate(
          `!!document.querySelector(".slot[data-view-slot=preferences][data-view-active]")`,
        );
      await press("PageDown", "PageDown", 34, CTRL);
      await until(
        "the next tab (Ctrl+PageDown)",
        async () => !(await viewShown()),
      );
      await press("PageUp", "PageUp", 33, CTRL);
      await until("back to Preferences (Ctrl+PageUp)", viewShown);
      await press("PageDown", "PageDown", 34, CTRL | SHIFT);
      await until(
        "the Preferences tab moved back (Ctrl+Shift+PageDown)",
        async () => (await order()).indexOf("preferences") === before,
      );
    }
    // Two tabs in one pane already fold the tools at 800 px (#100).
    const twoTabs = await foldedToolEntries(
      evaluate,
      `document.querySelector("[data-tab-view=preferences]").closest("[data-tab-bar]")`,
      "the Preferences tab's pane",
    );
    if (
      JSON.stringify(twoTabs) !==
      JSON.stringify(["new-tab", "join-pane", "split-right", "split-down"])
    )
      fail(`the folded actions of one pane: ${JSON.stringify(twoTabs)}`);
    await paneTool(
      evaluate,
      `document.querySelector("[data-tab-view=preferences]").closest("[data-tab-bar]")`,
      "split-right",
      "the Preferences tab's pane",
    );
    await until("Preferences split into a pane of their own", () =>
      evaluate(
        `document.querySelector("[data-tab-view=preferences]").closest("[data-tab-bar]").querySelectorAll("[data-tab]").length === 1`,
      ),
    );
    // The terminal's tab, shown again in its own pane (beside).
    await mouseClick(evaluate, `[data-tab-bar] [data-tab="${sessionTab}"]`);
    await until("Preferences in a pane of their own, beside the terminal", () =>
      evaluate(`(() => {
        const view = document.querySelector(".slot[data-view-slot=preferences]");
        const session = document.querySelector('.slot[data-tab="${sessionTab}"]');
        return !!view && !view.hidden && !session.hidden &&
          view.dataset.pane !== session.dataset.pane;
      })()`),
    );
  };
  // Sets a preferences field as the user does, then leaves it (change).
  const setPreference = (name, value) =>
    evaluate(`(() => {
      const field = document.querySelector("[data-view=preferences] [name=${name}]");
      if (field.type === "checkbox") field.checked = ${json(value)};
      else field.value = ${json(value)};
      field.dispatchEvent(new Event("input", { bubbles: true }));
      field.dispatchEvent(new Event("change", { bubbles: true }));
    })()`);
  /** Closes the Preferences tab; its pane goes, and the terminal's pane is
   * active again. */
  const closePreferencesBeside = async ({ shortcut = false } = {}) => {
    if (shortcut) {
      // Ctrl+Shift+W closes it like any tab.
      await mouseClick(evaluate, `[data-tab-view=preferences]`);
      await until("the Preferences pane active", () =>
        evaluate(
          `!!document.querySelector(".slot[data-view-slot=preferences][data-view-active]")`,
        ),
      );
      await press("W", "KeyW", 87, CTRL | SHIFT);
      // Gone before closePreferences looks for its close button.
      await until(
        "the Preferences tab closed by Ctrl+Shift+W",
        async () =>
          !(await evaluate(
            `!!document.querySelector("[data-tab-view=preferences]")`,
          )),
      );
    }
    await closePreferences(evaluate);
    await until("the terminal's pane active again", () =>
      evaluate(
        `!!document.querySelector('${SHOWN_SESSION}[data-tab="${sessionTab}"]')`,
      ),
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
        (await option("theme.background")) === "#0b0d10",
    );
    step("the theme followed the system setting, live");

    // A chosen theme wins over the system's, and is saved.
    await openPreferencesBeside({ shortcuts: true });
    await setPreference("preference-theme", "light");
    await until(
      "the light theme chosen, on the terminal beside",
      async () =>
        (await pageTheme()) === "light" &&
        (await option("theme.background")) === "#fbfbfc" &&
        /theme: light/.test(preferencesFile()),
    );
    const colours = await evaluate(
      `getComputedStyle(document.documentElement).backgroundColor`,
    );
    if (colours !== "rgb(246, 247, 249)") {
      fail(`the page is not light: ${colours}`);
    }
    step("choosing the light theme applied it to the whole app and saved it");

    // The terminal's font, size and scrollback, from Preferences.
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
    await closePreferencesBeside({ shortcut: true });
    step(
      "Preferences in a tab beside the terminal: theme, font, size and scrollback showed on it at once; tab shortcuts worked on the view tab",
    );

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
    await mouseClick(evaluate, `${SHOWN_SESSION} [data-search=next]`);
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
    const rightClick = () =>
      mouseClick(evaluate, `${SHOWN_SESSION} .xterm-screen`, {
        button: "right",
      });
    writeClipboard("not a selection");
    await dragSelect();
    if (!(await evaluate(`${terminal}.hasSelection()`))) {
      fail("dragging selected nothing");
    }
    await sleep(800);
    if (!same(readClipboard(), "not a selection")) {
      fail(`selecting copied while copy-on-select is off: ${readClipboard()}`);
    }

    await openPreferencesBeside();
    await setPreference("preference-copy-on-select", true);
    await setPreference("preference-right-click-paste", true);
    await until("the copy and paste options saved", () =>
      /copy_on_select: true[\s\S]*right_click_paste: true/.test(
        preferencesFile(),
      ),
    );
    await closePreferencesBeside();
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
/**
 * The palette's actions (#76): typed with the terminal holding the keys,
 * each found by its name and run (by click, or Enter when no host matches),
 * then what it opened is checked and put back.
 */
async function paletteActions({
  evaluate,
  exists,
  type,
  key,
  ctrlK,
  enter,
  palette,
}) {
  const escape = { key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 };
  const action = (id) => `[data-palette-command=${JSON.stringify(id)}]`;
  /** Opens the palette from the terminal, types `query`, and runs the
   * action `id` (by Enter, or by clicking its row). */
  const run = async (id, query, by = "click") => {
    await evaluate(
      `document.querySelector("${SHOWN_SESSION} .xterm textarea")?.focus()`,
    );
    await key(ctrlK);
    await until("the palette", () => exists(`${palette} input`));
    await until("the palette's input focused", () =>
      evaluate(`document.activeElement?.name === "palette-query"`),
    );
    await type(query);
    await until(`the action ${id} for "${query}"`, () => exists(action(id)));
    if (by === "enter") {
      await until(`"${query}" found no host`, async () =>
        evaluate(`(() => {
          const first = document.querySelector("${palette} [role=option]");
          return first?.dataset.paletteCommand === ${JSON.stringify(id)} &&
            first.getAttribute("aria-selected") === "true";
        })()`),
      );
      await key(enter);
    } else {
      await mouseClick(evaluate, action(id));
    }
    await until("the palette closed", async () => !(await exists(palette)));
  };
  const railActive = (view) => exists(`[data-rail=${view}][data-active]`);

  // Every side view, and folding.
  for (const [view, name] of [
    ["snippets", "snippets"],
    ["jobs", "jobs"],
    ["backups", "config backups"],
    ["credentials", "credentials"],
    ["hosts", "hosts"],
  ]) {
    await run(`side-${view}`, name);
    await until(`the ${view} side view from the palette`, () =>
      railActive(view),
    );
    // Every side view and view tab is checked for contrast (#79).
    await checkContrast(evaluate, `side view ${view}`);
  }
  await run("fold-side-view", "fold side view");
  await until("the side view folded from the palette", () =>
    exists("[data-rail-folded]"),
  );
  await run("fold-side-view", "unfold");
  await until(
    "the side view back from the palette",
    async () =>
      !(await exists("[data-rail-folded]")) && (await railActive("hosts")),
  );

  // The view tabs that need no subject, and both imports.
  for (const [view, query] of [
    ["preferences", "preferences"],
    ["platforms", "platform definitions"],
    ["highlights", "highlight rules"],
    ["python-setup", "python setup"],
    ["csv-import", "import csv"],
    ["nornir-import", "import nornir"],
  ]) {
    const tab = `[data-tab-view=${JSON.stringify(view)}]`;
    const before = await exists(tab);
    await run(view, query);
    await until(`the ${view} tab from the palette`, () =>
      evaluate(`(() => {
        const tabs = document.querySelectorAll(${JSON.stringify(tab)});
        return tabs.length === 1 && tabs[0].getAttribute("aria-selected") === "true" &&
          !!document.querySelector(${JSON.stringify(`.slot.view:not([hidden]) [data-view=${view}]`)});
      })()`),
    );
    await checkContrast(evaluate, `view tab ${view}`);
    if (!before) {
      await mouseClick(evaluate, `${tab} [data-tab-close]`);
      await until(`the ${view} tab closed`, async () => !(await exists(tab)));
    }
  }

  // New host and new folder ask in the workspace's dialog; Escape cancels.
  for (const [id, query, title] of [
    ["new-host", "new host", "New host"],
    ["new-folder", "new folder", "New folder"],
  ]) {
    await run(id, query);
    await until(`the ${title} dialog from the palette`, () =>
      evaluate(
        `document.querySelector("[role=dialog] #dialog-title")?.textContent === ${JSON.stringify(title)}`,
      ),
    );
    await checkContrast(evaluate, `dialog ${id}`);
    await key(escape);
    await until(
      `the ${title} dialog cancelled`,
      async () => !(await exists("[role=dialog] #dialog-title")),
    );
  }

  // The switcher, by Enter (no host matches the words).
  await run("switch-workspace", "switch workspace", "enter");
  await until("the switcher from the palette", () =>
    exists("[data-dialog=workspaces]"),
  );
  await checkContrast(evaluate, "workspace switcher");
  await key(escape);
  await until(
    "the switcher closed",
    async () => !(await exists("[data-dialog=workspaces]")),
  );
}

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
  const clickOn = (selector) => mouseClick(evaluate, selector);
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
  // A saved filter's chip: a click applies it, another clears it.
  const chip = (name) => `[data-filter-chip=${JSON.stringify(name)}]`;
  const chipPressed = (name) =>
    evaluate(
      `document.querySelector(${JSON.stringify(chip(name))})?.getAttribute("aria-pressed") === "true"`,
    );
  const filterWords = (words) =>
    evaluate(`(() => {
      const input = document.querySelector("input[name=host-filter]");
      input.value = ${JSON.stringify(words)};
      input.dispatchEvent(new Event("input", { bubbles: true }));
    })()`);

  // Ctrl+K reaches the palette even while the terminal has the keyboard.
  // Shift+Enter shows the host in the tree, opening its folder.
  await mouseClick(evaluate, `nav [data-item=folder][data-path=Lab]`);
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

  await paletteActions({ evaluate, exists, type, key, ctrlK, enter, palette });
  step(
    "the palette opened every side view, the view tabs, new host and folder, both imports and the switcher",
  );

  // The filter field narrows the tree as words are typed.
  const odd = '[data-host="odd.yaml"]';
  await filterWords("lab router");
  await until(
    "the tree narrowed by the filter field",
    async () => (await exists(hostRow)) && !(await exists(odd)),
  );
  await screenshot("filter-field");
  step("the filter field narrowed the tree to the matching hosts");

  // The words are saved as a filter from the chips row (here changed to a
  // role condition), and the tree is viewed through it.
  await clickOn("[data-save-filter]");
  await until("the save filter dialog", () => exists("[role=dialog] form"));
  const offered = await evaluate(`(() => {
    const form = document.querySelector("[role=dialog] form");
    return [form.querySelector("[name=field]").value, form.querySelector("[name=value]").value];
  })()`);
  if (JSON.stringify(offered) !== '["text","lab router"]') {
    fail(`the save filter dialog offers ${JSON.stringify(offered)}`);
  }
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
  await until(
    "the tree viewed through the filter",
    async () =>
      (await exists(hostRow)) &&
      !(await exists(odd)) &&
      (await chipPressed("Lab routers")),
  );
  if (
    await evaluate(`document.querySelector("input[name=host-filter]").value`)
  ) {
    fail("the filter field kept the saved words");
  }
  await screenshot("filter");
  await clickOn(chip("Lab routers"));
  await until(
    "every host back",
    async () => (await exists(odd)) && !(await chipPressed("Lab routers")),
  );
  await clickOn(chip("Lab routers"));
  await until("the filter applied again", async () => !(await exists(odd)));
  step(
    "a filter was saved from the chips row; its chip applies it and clears it",
  );

  // The filter bar runs a snippet and backs up the configs of the hosts it
  // shows (#127: untested until then): each opens its job tab for them.
  for (const [button, title, what] of [
    [
      "[data-filter-run-snippet]",
      "Run a snippet on the hosts of Lab routers",
      "Run snippet",
    ],
    [
      "[data-filter-back-up]",
      "Back up configs of the hosts of Lab routers",
      "Back up configs",
    ],
  ]) {
    await clickOn(button);
    await until(`the job tab of the filter bar's ${what}`, async () =>
      (
        await evaluate(
          `document.querySelector(${JSON.stringify(`${JOB_TAB} h2`)})?.textContent ?? ""`,
        )
      ).includes(title),
    );
    await closeTabOf(evaluate, JOB_TAB);
  }
  step(
    "the filter bar's Run snippet and Back up configs opened a job tab for the filter's hosts",
  );

  await clickOn("[data-filter-delete]");
  await until("the delete confirmation", () => exists("[role=dialog] form"));
  await clickOn("[role=dialog] form button[type=submit]");
  await until("the filters file removed", () => !existsSync(filters));
  await until(
    "every host back",
    async () => (await exists(odd)) && !(await exists(chip("Lab routers"))),
  );
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

  await mouseClick(evaluate, odd);
  await showDetails(evaluate);
  await until("odd.yaml's side panel", async () =>
    (await text(".side h2")).includes("odd"),
  );

  writeFileSync(join(workspace, "pulled-r1.yaml"), "address: 192.0.2.20\n");
  writeFileSync(
    join(workspace, "odd.yaml"),
    "address: 192.0.2.9\nrole: mainframe\nvendor: Juniper\n",
  );
  await until("the new host in the tree", () => exists(pulled));
  await showDetails(evaluate);
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

  // The workspace folder removed (a network drive that disconnects) and
  // made again with the same files (it reconnects): the Hosts view says
  // it cannot be reached meanwhile, then live reload goes on (#132).
  const unavailable = "[data-workspace-unavailable=unreachable]";
  const kept = join(userData, "smoke-workspace-kept");
  cpSync(workspace, kept, { recursive: true });
  rmSync(workspace, { recursive: true, maxRetries: 10, retryDelay: 200 });
  const said = await until("the workspace said to be unreachable", () =>
    text(unavailable),
  );
  if (!/cannot be reached/.test(said))
    fail(`the unreachable workspace: ${said}`);
  if (!(await exists(odd))) fail("what was shown went with the folder");
  await screenshot("workspace-unreachable");
  cpSync(kept, workspace, { recursive: true });
  rmSync(kept, { recursive: true, maxRetries: 10, retryDelay: 200 });
  await until(
    "the workspace loaded again once back",
    async () => !(await exists(unavailable)),
  );
  writeFileSync(join(workspace, "pulled-r2.yaml"), "address: 192.0.2.21\n");
  const pulledAgain = 'nav [data-host="pulled-r2.yaml"]';
  await until("a host added after the folder came back", () =>
    exists(pulledAgain),
  );
  rmSync(join(workspace, "pulled-r2.yaml"));
  await until("that host gone again", async () => !(await exists(pulledAgain)));
  step(
    "the workspace folder removed and made again: said to be unreachable, then live reload went on",
  );
}

/** A credential profile is created in the Credentials view and assigned to
 * a host, which then connects without a prompt; a profile the workspace
 * names but nobody set up is asked for, saved and used; no secret reaches
 * a file. The app runs with its in-memory credential store, so the
 * keychain is never touched. */
async function credentialProfiles({
  evaluate,
  text,
  click,
  type,
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
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const hostRow = 'nav [data-host="Lab/smoke-r1.yaml"]';
  const hostFile = join(workspace, "Lab", "smoke-r1.yaml");
  const profilesDialog = CREDENTIALS;
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

  // With no profiles, the Credentials view says what one is and offers
  // the first, which is created there.
  await openProfiles(evaluate);
  const empty = await until("the empty Credentials view", () =>
    text(`${profilesDialog} [data-profiles-empty]`),
  );
  if (!/credential profile is a login/.test(empty))
    fail(`the empty Credentials view: ${empty}`);
  await clickOn(`${profilesDialog} [data-profile-first]`);
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
  if (await exists(`${profilesDialog} [data-profiles-empty]`))
    fail("the empty state stayed after the first profile");
  // The list says how a host uses one (#116).
  if (
    !(await text(`${profilesDialog} [data-profiles-use]`)).includes(
      "Credential profile setting",
    )
  ) {
    fail("the profiles list does not say how a host uses a profile");
  }
  if (await exists("[data-dialog=profiles]")) fail("a profiles dialog is left");
  await screenshot("profiles");
  await showSideView(evaluate, "hosts");
  step("the first credential profile was created in the Credentials view");

  // Assigned to a host, the profile logs in without a prompt.
  await until("the host in the tree", () => exists(hostRow));
  await clickOn(hostRow);
  await showDetails(evaluate);
  await until("the host's side panel", async () =>
    (await text(".side h2")).includes("smoke-r1"),
  );
  // The setting says what goes in it (#116).
  if (
    !(await text(".side [data-setting-note=credential_profile]")).includes(
      "a profile in the Credentials view, or a new name",
    )
  ) {
    fail("the Credential profile setting does not say what it takes");
  }
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

  // A wrong password in the profile fails with what to check; corrected in
  // the Credentials view, Enter reconnects with the corrected one (#130).
  const editPassword = async (password) => {
    await openProfiles(evaluate);
    await clickOn(`${profileRow("lab-login")} [data-profile-edit]`);
    await until("the edit form", () =>
      exists(`${profilesDialog} [name=profile-password]`),
    );
    await fillForm({ "profile-password": password }, `${profilesDialog} form`);
    await until("the profile saved", () =>
      exists(`${profilesDialog}[data-mode=list] [data-profile="lab-login"]`),
    );
  };
  const pressEnter = async () => {
    await evaluate(
      `document.querySelector('${SHOWN_SESSION} .xterm textarea').focus()`,
    );
    await type("\r");
  };
  await sessionAction(evaluate, "disconnect");
  await until("the session closed", async () =>
    (await status()).startsWith("Closed"),
  );
  await editPassword("not the password");
  await pressEnter();
  await until(
    "the profile's wrong password refused",
    async () => (await status()) === "Closed: authentication failed",
  );
  const endedBar = `${SHOWN_SESSION} [data-session-ended]`;
  await until("its advice", async () =>
    (await text(`${endedBar} [data-session-advice]`)).includes(
      "change them in the Credentials view, then reconnect",
    ),
  );
  if (!(await text(endedBar)).includes("Press Enter to reconnect.")) {
    fail(`the failure's bar says: ${await text(endedBar)}`);
  }
  await editPassword(PASSWORD);
  await pressEnter();
  await connected();
  await screenshot("reconnected-with-corrected-profile");
  await showSideView(evaluate, "hosts");
  await showDetails(evaluate);
  step(
    "a profile's password corrected in the Credentials view was used by Enter's reconnect",
  );

  // A profile the workspace names but this computer lacks is asked for by
  // name, saved, and used.
  await sessionAction(evaluate, "disconnect");
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

  // Profiles are edited, renamed and deleted in the Credentials view; a
  // profile the workspace names that is no longer stored shows as missing.
  await sessionAction(evaluate, "disconnect");
  await openProfiles(evaluate);
  await until(
    "both profiles listed",
    async () =>
      (await exists(profileRow("lab-login"))) &&
      (await exists(profileRow("team-login"))),
  );
  if (await exists(`${profilesDialog} [data-profile-missing]`))
    fail("a stored profile is marked missing");
  await clickOn(`${profileRow("lab-login")} [data-profile-edit]`);
  await until("the edit form", () =>
    evaluate(
      `document.querySelector("${profilesDialog} [name=profile-username]")?.value === ${JSON.stringify(USERNAME)}`,
    ),
  );
  await fillForm({ "profile-username": "operator" }, `${profilesDialog} form`);
  await until("the edited username listed", async () =>
    (await text(profileRow("lab-login"))).includes("operator, password"),
  );
  // Renamed, the workspace's name for it is missing; renamed back, not.
  const rename = async (from, to) => {
    await clickOn(`${profileRow(from)} [data-profile-edit]`);
    await until("the edit form", () =>
      exists(`${profilesDialog} [name=profile-name]`),
    );
    await fillForm({ "profile-name": to }, `${profilesDialog} form`);
    await until(`${to} listed`, () =>
      exists(
        `${profilesDialog}[data-mode=list] [data-profile="${to}"]:not([data-profile-missing])`,
      ),
    );
  };
  await rename("team-login", "team-renamed");
  await until("the old name shown as missing", () =>
    exists(`${profileRow("team-login")}[data-profile-missing]`),
  );
  await rename("team-renamed", "team-login");
  if (await exists(profileRow("team-renamed")))
    fail("the renamed profile kept its other name");
  await clickOn(`${profileRow("team-login")} [data-profile-delete]`);
  await until("the delete confirmation", async () =>
    (
      await text(`${profilesDialog} [data-credentials-confirm=profile]`)
    ).includes("Delete team-login"),
  );
  await clickOn(`${profilesDialog} [data-profile-action=delete]`);
  await until("the deleted profile shown as missing", async () =>
    (await text(`${profileRow("team-login")}[data-profile-missing]`)).includes(
      "not set up",
    ),
  );
  step(
    "a profile was edited, renamed and another deleted in the Credentials view",
  );

  // Named secrets: added, value replaced, refused twice by name, deleted;
  // the dialog shows names only, and a profile of the same name is kept
  // apart.
  const secretForm = `${profilesDialog} form[data-secret-form]`;
  const secretRow = (name) => `${profilesDialog} [data-secret="${name}"]`;
  const secretValues = ["t0ken-v4lue", "r0tated-v4lue"];
  // Neither the page's markup nor any field's value holds a secret value.
  const valueShown = () =>
    evaluate(`(() => {
      const values = ${JSON.stringify(secretValues)};
      const page = document.documentElement.outerHTML;
      const fields = [...document.querySelectorAll("input, textarea")].map((i) => i.value);
      return values.some((v) => page.includes(v) || fields.includes(v));
    })()`);
  await clickOn(`${profilesDialog} [data-secret-new]`);
  await until("the secret form", () =>
    exists(`${secretForm} [name=secret-name]`),
  );
  await fillForm(
    { "secret-name": "lab-login", "secret-value": secretValues[0] },
    secretForm,
  );
  await until("the new secret listed", () => exists(secretRow("lab-login")));
  if (!(await exists(profileRow("lab-login")))) {
    fail("adding a secret named like a profile changed the profile");
  }
  if (await valueShown()) fail("a secret's value is shown after saving");
  await clickOn(`${secretRow("lab-login")} [data-secret-replace]`);
  await until(
    "the replace form",
    async () =>
      (await exists(`${secretForm} [name=secret-value]`)) &&
      !(await exists(`${secretForm} [name=secret-name]`)),
  );
  await fillForm({ "secret-value": secretValues[1] }, secretForm);
  await until(
    "the secret list again",
    async () =>
      !(await exists(secretForm)) && (await exists(secretRow("lab-login"))),
  );
  if (await valueShown()) fail("a replaced secret's value is shown");
  await clickOn(`${profilesDialog} [data-secret-new]`);
  await until("the secret form", () =>
    exists(`${secretForm} [name=secret-name]`),
  );
  await fillForm(
    { "secret-name": "LAB-LOGIN", "secret-value": "x" },
    secretForm,
  );
  await until("the duplicate name refused", async () =>
    (await text(`${profilesDialog} [data-secret-error]`)).includes(
      "already exists",
    ),
  );
  await clickOn(`${secretForm} [data-secret-action=cancel]`);
  await screenshot("secrets");
  await clickOn(`${secretRow("lab-login")} [data-secret-delete]`);
  await until("the secret delete confirmation", () =>
    exists(`${profilesDialog} [data-secret-action=delete]`),
  );
  await clickOn(`${profilesDialog} [data-secret-action=delete]`);
  await until(
    "the secret gone",
    async () =>
      (await exists(`${profilesDialog} [data-secrets]`)) &&
      !(await exists(secretRow("lab-login"))),
  );
  if (!(await exists(profileRow("lab-login")))) {
    fail("deleting a secret named like a profile deleted the profile");
  }
  await showSideView(evaluate, "hosts");
  await showDetails(evaluate);
  step("a named secret was added, replaced and deleted; only its name shown");

  for (const secret of [PASSWORD, "en4ble-s3cret", ...secretValues]) {
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
  const profilesDialog = CREDENTIALS;
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
  await sessionAction(evaluate, "disconnect", SHOWN_SESSION, 0).catch(() => {});
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
  for (const control of ["[name=answer-0]", "button[type=submit]"])
    await reachable(evaluate, `${loginPrompt} ${control}`);
  await fillForm({ "answer-0": ONE_TIME_CODE }, loginPrompt);
  await connected();
  if (await exists(loginPrompt)) fail("the question stayed on screen");
  step("a one-time code the server asked for was shown and answered");
  await sessionAction(evaluate, "disconnect");

  // The device's login banner (#134) is in the terminal before the
  // question, which leaves it readable above. The ended bar's "New
  // connection" is drawn a moment after the disconnect: clicked until the
  // form is there.
  await until("the quick-connect form", async () => {
    if (await exists("input[name=host]")) return true;
    await click("New connection").catch(() => {});
    return false;
  });
  await fillForm(
    {
      host: "127.0.0.1",
      port: String(BANNER_PORT),
      username: OTP_USERNAME,
      password: PASSWORD,
    },
    "main form",
  );
  // A port of its own, so its host key is asked about first (the key
  // exchange comes before any banner): trusted for this connection.
  const trustOnce = "[data-host-key=first-use] [data-decision=trustOnce]";
  await until("the banner server's host-key question", () =>
    exists(`${trustOnce}:not(:disabled)`),
  );
  await mouseClick(evaluate, trustOnce);
  await until("the question after the banner", async () =>
    (await text(loginPrompt)).includes("Verification code:"),
  );
  await until("the login banner in the terminal", async () => {
    const shown = await screen();
    return (
      shown.includes("AUTHORIZED ACCESS ONLY") &&
      shown.includes("This is the test server's login banner.")
    );
  });
  await until("the card's note on the banner", () =>
    exists(`${loginPrompt} [data-login-banner-above]`),
  );
  await screenshot("login-banner");
  await fillForm({ "answer-0": ONE_TIME_CODE }, loginPrompt);
  await connected();
  if (!(await screen()).includes("AUTHORIZED ACCESS ONLY"))
    fail("the login banner left the terminal once connected");
  step("the device's login banner was shown before its question");
  await sessionAction(evaluate, "disconnect");

  // Profiles that log in with the SSH agent.
  const createAgentProfile = async (name, username) => {
    await newProfile(evaluate);
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
    await showSideView(evaluate, "hosts");
    await showDetails(evaluate);
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
  await sessionAction(evaluate, "disconnect");

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
  // With what to check (#116).
  if (
    !(await text("[data-password-fallback] [data-session-advice]")).includes(
      "that the device has your public key",
    )
  ) {
    fail("the agent failure does not say what to check");
  }
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
  await sessionAction(evaluate, "disconnect");

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
    // A host with no credential profile says where one keeps the login
    // (#116).
    if (
      !(await text("main [data-credentials-hint]")).includes(
        "Credential profile setting",
      )
    ) {
      fail("the login asked for does not say how to keep it");
    }
    await fillForm({ username: USERNAME, password: PASSWORD }, "main form");
  };

  await sessionAction(evaluate, "disconnect", SHOWN_SESSION, 0).catch(() => {});
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
  await reachable(evaluate, `${failureCard} button`);
  await screenshot("negotiation-failure");
  step("a failed negotiation listed the device's algorithms and the switch");

  // Switch on, in the side panel: the session connects, marked.
  await newConnection();
  await mouseClick(evaluate, hostRow);
  await showDetails(evaluate);
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
  await checkHeaderLine(evaluate, SHOWN_SESSION, ["[data-legacy-crypto]"]);
  await screenshot("legacy-crypto");
  step("with the switch on, the old device connected, marked legacy crypto");
  await sessionAction(evaluate, "disconnect");

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
  await sessionAction(evaluate, "disconnect");
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

  await sessionAction(evaluate, "disconnect", SHOWN_SESSION, 0).catch(() => {});
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
  await checkHeaderLine(evaluate, SHOWN_SESSION, [
    "[data-unencrypted]",
    "[data-session-protocol=telnet]",
  ]);
  await screenshot("telnet-session");
  step(
    `double-clicking a telnet host logged in in the terminal (${size}), marked unencrypted`,
  );

  // The session's header opens its host's details, bringing back the
  // Hosts view if another one is shown.
  await mouseClick(evaluate, `[data-rail=snippets]`);
  await until("the snippets view", () => exists("[data-side-view=snippets]"));
  await sessionAction(evaluate, "details");
  await until("the telnet host's details from its session", async () =>
    (
      await text("[data-side-view=hosts] [data-details-sheet=host] h2")
    ).includes("console-sw"),
  );
  step("a session's Details opened its host's sheet over the Hosts view");

  // The side panel offers SSH for the telnet host, and telnet for an SSH one.
  await sessionAction(evaluate, "disconnect");
  await newConnection();
  await mouseClick(evaluate, hostRow);
  await showDetails(evaluate);
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
  await mouseClick(evaluate, sidePanelButton("telnet"));
  await loginPrompt();
  if (!(await exists(marker))) fail("the telnet session is not marked");
  step(
    "the side panel opened a telnet session to a host whose protocol is SSH",
  );
  await sessionAction(evaluate, "disconnect");
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
  const clickOn = (selector) => mouseClick(evaluate, selector);
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
  await showDetails(evaluate);
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
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const dialog = "[role=dialog] form";
  const dialogOpen = () =>
    evaluate(`!!document.querySelector(${JSON.stringify(dialog)})`);
  const folderRow = (path) => `nav [data-item=folder][data-path="${path}"]`;
  const hostRow = (id) => `nav [data-host="${id}"]`;
  const hostFile =
    "address: 10.9.0.1\nvendor: Arista\nrack_unit: 12\nrole: firewall\n";
  // What the side panel says applies, and where it comes from.
  const shows = async (key, expected) => {
    await showDetails(evaluate);
    return until(
      `${key} shown as "${expected}"`,
      async () =>
        (await text(`.side [data-origin="${key}"]`)).trim() === expected,
    );
  };

  // Every control of the Hosts view's header is fully inside the side
  // view, and no toolbar of text buttons is left.
  const clipped = await evaluate(`(() => {
    const panel = document.querySelector("[data-side-view=hosts]").getBoundingClientRect();
    return [...document.querySelectorAll("[data-side-view=hosts] header button")]
      .filter((b) => {
        const r = b.getBoundingClientRect();
        return r.width === 0 || r.left < panel.left || r.right > panel.right + 0.5;
      })
      .map((b) => b.getAttribute("aria-label") ?? b.textContent.trim());
  })()`);
  if (clipped.length > 0) {
    fail(`Hosts view controls cut off: ${clipped.join(", ")}`);
  }
  if (await evaluate(`!!document.querySelector(".toolbar")`)) {
    fail("a toolbar of text buttons is still there");
  }
  // The add control offers hosts, folders and both imports.
  await clickOn("[data-add-menu]");
  const additions = await evaluate(
    `[...document.querySelectorAll("[data-add-entries] [data-add]")].map((b) => b.dataset.add)`,
  );
  if (JSON.stringify(additions) !== '["host","folder","csv","nornir"]') {
    fail(`the add control offers ${JSON.stringify(additions)}`);
  }
  await screenshot("add-menu");
  await evaluate(
    `window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }))`,
  );
  await until(
    "the add menu closed",
    async () =>
      !(await evaluate(`!!document.querySelector("[data-add-entries]")`)),
  );
  step(
    "the Hosts view's controls are visible; one add control offers what to add",
  );

  // A folder and a host, created from the add control at the top level.
  await clickOn("[data-workspace-settings]");
  await clickOn("[data-add-menu]");
  await clickOn("[data-add=folder]");
  await until("the new folder dialog", dialogOpen);
  await fillForm({ name: "Edge" }, dialog);
  await until("the Edge folder on disk", () => existsSync(file("Edge")));
  await clickOn("[data-add-menu]");
  await clickOn("[data-add=host]");
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
  await showDetails(evaluate);
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

  // Dragging the host onto another folder moves its file (in the whole
  // tree: the details sheet closed, back to the tree).
  if (await evaluate(`!!document.querySelector("[data-details-close]")`)) {
    await clickOn("[data-details-close]");
  }
  await until(
    "the details sheet closed",
    async () =>
      !(await evaluate(`!!document.querySelector("[data-details-sheet]")`)),
  );
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
  // The platform is a setting too: its own value goes after the vendor.
  await shows("platform", "none");
  await changeField("[name=setting-platform]", "srlinux");
  await until(
    "the host's platform in its file",
    () =>
      contents("Lab/edge-r2.yaml") ===
      "address: 10.9.0.1\nvendor: Arista\nplatform: srlinux\nrack_unit: 12\nrole: firewall\n",
  );
  await shows("platform", "srlinux · set on this host");
  await clickOn('.side [aria-label="Clear Platform"]');
  await until(
    "the host's platform cleared from its file",
    () => contents("Lab/edge-r2.yaml") === hostFile,
  );
  await shows("platform", "none");
  step(
    "the side panel showed effective settings and origins, set and cleared one",
  );

  // A folder's description is saved to its settings file.
  await clickOn(folderRow("Edge"));
  await showDetails(evaluate);
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
  await changeField("[name=setting-platform]", "eos");
  await until(
    "the folder's platform in its file",
    () =>
      contents("Edge/_folder.yaml") ===
      "description: Edge routers\nplatform: eos\nenvironment: lab\n",
  );
  await shows("platform", "eos · set on this folder");
  await clickOn('.side [aria-label="Clear Platform"]');
  await until(
    "the folder's platform cleared from its file",
    () =>
      contents("Edge/_folder.yaml") ===
      "description: Edge routers\nenvironment: lab\n",
  );
  await shows("platform", "none");
  step("a setting on a folder was written to its settings file");

  // Platform definitions, in their tab (opened from the details sheet's
  // platform setting, then from the palette): the folder's platform finds
  // none yet; one is created, a built-in changed and reset, a bad pattern
  // refused, and an external edit of the file picked up.
  const platformsFile = "_platforms.yaml";
  const platformsDialog = "[data-view=platforms]";
  const platformMatch = () =>
    evaluate(
      `document.querySelector(".side [data-platform-match]")?.getAttribute("data-platform-match") ?? null`,
    );
  const platformRow = (name) => `${platformsDialog} [data-platform="${name}"]`;
  const platformSource = (name) =>
    evaluate(
      `document.querySelector(${JSON.stringify(platformRow(name))})?.getAttribute("data-platform-source") ?? null`,
    );
  const platformsShown = () =>
    evaluate(`(() => {
      const view = document.querySelector("${platformsDialog}");
      return !!view && !view.closest("[hidden]") &&
        !!document.querySelector("[data-tab-view=platforms]");
    })()`);
  const openPlatforms = async (from) => {
    if (from === "sheet") {
      await clickOn(".side [data-open-platforms]");
    } else {
      // Ctrl+K, then the palette's command.
      await evaluate(
        `window.dispatchEvent(new KeyboardEvent("keydown", { key: "k", code: "KeyK", ctrlKey: true, bubbles: true }))`,
      );
      await until("the palette", () =>
        evaluate(`!!document.querySelector("input[name=palette-query]")`),
      );
      await evaluate(`(() => {
        const input = document.querySelector("input[name=palette-query]");
        input.value = "platform definitions";
        input.dispatchEvent(new Event("input", { bubbles: true }));
      })()`);
      await until("the platforms command in the palette", () =>
        evaluate(
          `!!document.querySelector("[data-palette-command=platforms]")`,
        ),
      );
      await clickOn("[data-palette-command=platforms]");
    }
    await until(`the platforms tab (from the ${from})`, platformsShown);
  };
  const closePlatforms = async () => {
    await clickOn(`${platformsDialog} [data-platform-action=close]`);
    await until(
      "the platforms tab closed",
      async () =>
        !(await evaluate(
          `!!document.querySelector("${platformsDialog}") || !!document.querySelector("[data-tab-view=platforms]")`,
        )),
    );
  };
  const platformForm = `${platformsDialog} [data-platform-form]`;
  const formOpen = () =>
    evaluate(`!!document.querySelector(${JSON.stringify(platformForm)})`);

  await changeField("[name=setting-platform]", "RouterOS");
  await until(
    "the folder's platform finds no definition",
    async () => (await platformMatch()) === "",
  );
  await openPlatforms("sheet");
  if (
    !(await evaluate(`!!document.querySelector(".side [data-platform-match]")`))
  )
    fail("the details sheet is not in view beside the platforms tab");
  for (const name of ["srlinux", "eos", "frr", "ios", "nxos", "junos"]) {
    if ((await platformSource(name)) !== "builtIn") {
      fail(`the built-in platform ${name} is not listed as built in`);
    }
  }
  await clickOn(`${platformsDialog} [data-platform-action=new]`);
  await until("the new platform form", formOpen);
  if (
    !(await evaluate(
      `!!document.querySelector(${JSON.stringify(`${platformForm} [data-platform-note=file-folder]`)})?.textContent.includes("Where files belong")`,
    ))
  )
    fail("the platform form does not say what the file folder is for");
  await fillForm(
    {
      "platform-name": "mikrotik",
      "platform-other-names": "routeros, mikro",
      "platform-prompt": "\\[\\S+@\\S+\\] >",
      "platform-show-config": "/export",
      "platform-file-folder": "flash/",
    },
    platformForm,
  );
  await until(
    "the new definition in the platforms file",
    () =>
      contents(platformsFile) ===
      "mikrotik:\n  other_names:\n  - routeros\n  - mikro\n  prompt: \\[\\S+@\\S+\\] >\n  show_config: /export\n  file_folder: flash/\n",
  );
  await until(
    "the new definition listed",
    async () => (await platformSource("mikrotik")) === "workspace",
  );
  if (
    (await evaluate(
      `document.querySelector(${JSON.stringify(`${platformRow("mikrotik")} [data-platform-file-folder]`)})?.textContent ?? null`,
    )) !== "files in flash/"
  )
    fail("the new definition's file folder is not listed");
  await screenshot("platforms");

  // A built-in changed: only the difference is written; reset removes it.
  await clickOn(`${platformRow("eos")} [data-platform-action=edit]`);
  await until("the eos form", formOpen);
  await fillForm(
    {
      "platform-disable-paging": "terminal pager 0",
      "platform-file-folder": "/mnt/flash/images",
    },
    platformForm,
  );
  await until("the changed built-in in the platforms file", () =>
    contents(platformsFile)?.endsWith(
      "eos:\n  disable_paging: terminal pager 0\n  file_folder: /mnt/flash/images\n",
    ),
  );
  await until(
    "eos listed as changed",
    async () => (await platformSource("eos")) === "changed",
  );
  await clickOn(`${platformRow("eos")} [data-platform-action=reset]`);
  await until(
    "the reset built-in gone from the platforms file",
    () => !contents(platformsFile)?.includes("eos:"),
  );
  await until(
    "eos listed as built in again",
    async () => (await platformSource("eos")) === "builtIn",
  );

  // A pattern that cannot be used is refused, and the file stays.
  const before = contents(platformsFile);
  await clickOn(`${platformsDialog} [data-platform-action=new]`);
  await until("the new platform form", formOpen);
  await fillForm(
    { "platform-name": "bad", "platform-prompt": "(" },
    platformForm,
  );
  await until("the refused pattern's message", () =>
    evaluate(
      `!!document.querySelector("${platformsDialog} [data-platform-error-message]")`,
    ),
  );
  if (contents(platformsFile) !== before) {
    fail("a refused definition changed the platforms file");
  }
  await clickOn(`${platformsDialog} [data-platform-action=cancel]`);
  await closePlatforms();
  step(
    "platform definitions in a tab opened from the details sheet were created, changed, reset and refused",
  );

  // The folder's platform (another name of the new definition) finds it,
  // and the platform field offers the defined platforms.
  await until(
    "the folder's platform finds the new definition",
    async () => (await platformMatch()) === "mikrotik",
  );
  const offered = await evaluate(
    `[...document.querySelectorAll("#platforms option")].map((o) => o.value)`,
  );
  for (const name of ["srlinux", "eos", "junos", "mikrotik"]) {
    if (!offered.includes(name)) {
      fail(`the platform field does not offer ${name}: ${offered.join(", ")}`);
    }
  }

  // Edited outside the app: a broken prompt is shown and not used.
  writeFileSync(
    file(platformsFile),
    "mikrotik:\n  prompt: '(oops'\n  other_names: routeros\n",
  );
  await until(
    "the external edit picked up",
    async () => (await platformMatch()) === "",
  );
  await openPlatforms("palette");
  await until("the broken pattern shown with its definition", () =>
    evaluate(
      `!!document.querySelector(${JSON.stringify(`${platformRow("mikrotik")} [data-platform-error]`)})`,
    ),
  );
  await clickOn(`${platformRow("mikrotik")} [data-platform-action=delete]`);
  await until(
    "the platforms file gone with its last definition",
    () => contents(platformsFile) === null,
  );
  await closePlatforms();
  await clickOn('.side [aria-label="Clear Platform"]');
  await until(
    "the folder's platform cleared again",
    () =>
      contents("Edge/_folder.yaml") ===
      "description: Edge routers\nenvironment: lab\n",
  );
  step(
    "a folder's platform found its definition, and an external edit was picked up in the platforms tab opened from the palette",
  );

  // Moving a folder from its context menu.
  const openMenu = async (selector) => {
    await mouseClick(evaluate, selector, { button: "right" });
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
  // The files go before the dialog does: the next part clicks the tabs.
  await until(
    "the delete confirmation closed",
    async () => !(await evaluate(`!!document.querySelector("[role=dialog]")`)),
  );
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
  const clickOn = (selector) => mouseClick(evaluate, selector);
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
    await paneTool(
      evaluate,
      `document.querySelector(".pane.active [data-tab-bar]")`,
      "new-tab",
      "the active pane",
    );
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
    // Over the tab first, then in steps to the target, as a hand would: a
    // press with no move before it, then one jump, was missed now and then.
    await mouse("mouseMoved", from);
    await mouse("mousePressed", from, { button: "left", clickCount: 1 });
    await mouse(
      "mouseMoved",
      { x: from.x + 10, y: from.y + 2 },
      {
        button: "left",
        buttons: 1,
      },
    );
    for (let step = 1; step <= 4; step++) {
      await mouse(
        "mouseMoved",
        {
          x: from.x + 10 + ((to.x - from.x - 10) * step) / 4,
          y: from.y + 2 + ((to.y - from.y - 2) * step) / 4,
        },
        { button: "left", buttons: 1 },
      );
    }
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
  await paneTool(
    evaluate,
    `document.querySelector(".pane.active [data-tab-bar]")`,
    "split-right",
    "the active pane",
  );
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
  // At 800 px the top-right pane has folded its tools and its new-tab
  // button into the "…" button (#100); the tab and the window's buttons
  // stay clickable. Split from its menu. The bar folds once it has measured
  // itself, a moment after the split.
  await until("the top-right pane folded its tools", () =>
    evaluate(
      `!!document.querySelector(".pane.active [data-tab-bar] [data-pane-tools-more]")`,
    ),
  );
  const sideBySide = await foldedToolEntries(
    evaluate,
    `document.querySelector(".pane.active [data-tab-bar]")`,
    "the top-right pane",
  );
  if (
    JSON.stringify(sideBySide) !==
    JSON.stringify([
      "new-tab",
      "join-pane",
      "split-right",
      "split-down",
      "close-pane",
    ])
  )
    fail(`the folded pane actions: ${JSON.stringify(sideBySide)}`);
  if (
    !(await paneTool(
      evaluate,
      `document.querySelector(".pane.active [data-tab-bar]")`,
      "split-down",
      "the active pane",
    ))
  )
    fail("the narrow top-right pane did not fold its tools");
  await until("three panes", async () => (await layout()).length === 3);
  step(
    "at 800 px the top-right pane folded its tools into a menu (new tab, multi-send, split right and down, close pane); the tab and the window's buttons stayed clickable",
  );
  const c = await openSession();
  panes = await layout();
  if (!panes[2].tabs.some((t) => t.id === c)) {
    fail(`the new session is not in the new pane: ${json(panes)}`);
  }
  await screenshot("split-panes");
  step("the window split right and down, each pane showing its own session");

  // The window's buttons (#80) end the top-right pane's bar, cover no tab,
  // and only the bars at the window's top drag it.
  const frame = await evaluate(`(() => {
    const bar = (pane) => document.querySelector('[data-tab-bar="' + pane + '"]');
    const controls = [...document.querySelectorAll("[data-window-control]")];
    // What shows of each tab: clipped by its tab strip, which scrolls.
    const tabs = [...document.querySelectorAll("[data-tab-bar] [data-tab]")]
      .map((tab) => {
        const box = tab.getBoundingClientRect();
        const strip = tab.closest("[role=tablist]").getBoundingClientRect();
        const left = Math.max(box.left, strip.left);
        const right = Math.min(box.right, strip.right);
        return { left, right, top: box.top, bottom: box.bottom, width: Math.max(0, right - left) };
      });
    const overlaps = controls.some((control) => {
      const box = control.getBoundingClientRect();
      return tabs.some((tab) => tab.left < box.right && box.left < tab.right && tab.top < box.bottom && box.top < tab.bottom);
    });
    return {
      where: controls.map((control) => Number(control.closest("[data-tab-bar]")?.dataset.tabBar)),
      overlaps,
      // Each pane's shown tab stays readable, and the buttons stay inside
      // the window, however narrow the pane.
      narrowest: Math.min(...tabs.map((tab) => Math.round(tab.width))),
      outside: controls.filter((control) => control.getBoundingClientRect().right > window.innerWidth + 0.5).length,
      drag: ${json(panes.map((p) => p.pane))}.map((pane) => bar(pane).dataset.tauriDragRegion ?? null),
    };
  })()`);
  if (
    json(frame.where) !== json([panes[1].pane, panes[1].pane, panes[1].pane]) ||
    frame.overlaps ||
    frame.narrowest < 45 ||
    frame.outside > 0 ||
    json(frame.drag) !== json(["deep", "deep", null])
  ) {
    fail(`the window's frame over split panes: ${json(frame)}`);
  }
  step(
    "with panes side by side, the window's buttons end the top-right pane's bar, cover no tab, and the pane below has neither buttons nor a drag region",
  );
  // At its narrowest, with the side view open and folded (#120), the
  // three panes kept.
  {
    const sideShown = () =>
      evaluate(`!!document.querySelector("[data-side-view]")`);
    const toggleSide = async () => {
      const before = await sideShown();
      await pressIn(devtools, "B", "KeyB", 66, CTRL | SHIFT);
      await until(
        "the side view folded or back",
        async () => (await sideShown()) !== before,
      );
    };
    const widths = {};
    for (const round of [0, 1]) {
      const open = await sideShown();
      widths[open ? "open" : "folded"] = await narrowFrame(
        evaluate,
        `the main window with three panes, the side view ${open ? "open" : "folded"}`,
        480,
      );
      if ((await layout()).length !== 3) fail("the narrow window lost a pane");
      if (round === 0) await toggleSide();
    }
    await toggleSide();
    step(
      `at its narrowest (${widths.open} px) with three panes, the side view open and folded, the main window's three buttons stayed whole inside it at its top right, over nothing, beside an empty part that drags it, and Maximize and Restore answered`,
    );
  }

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

  // Questions about one tab cover only its pane (#74). `within(dialog,
  // tab)`: the dialog lies over the tab's slot and not over `a`'s pane.
  const placedOver = (dialog, tab) =>
    evaluate(`(() => {
      const card = document.querySelector(${json(dialog)});
      if (!card) return null;
      const box = card.getBoundingClientRect();
      const own = document.querySelector('.slot[data-tab="${tab}"]').getBoundingClientRect();
      const other = document.querySelector('.slot[data-tab="${a}"]').getBoundingClientRect();
      const inside = box.left >= own.left - 1 && box.right <= own.right + 1 &&
        box.top >= own.top - 1 && box.bottom <= own.bottom + 1;
      const overlaps = box.left < other.right && box.right > other.left &&
        box.top < other.bottom && box.bottom > other.top;
      return { inside, overlaps, pane: card.closest("[data-pane-dialog]")?.dataset.paneDialog ?? null };
    })()`);
  const cPane = String((await layout())[0].pane);

  // A paste confirmation dims only its pane: the terminal in the other
  // pane still takes typing while it is open; Escape cancels it.
  await evaluate(
    `document.querySelector(${json(`${slot(c)} .terminal textarea`)}).focus()`,
  );
  writeClipboard("pane-one\npane-two\n");
  await press("V", "KeyV", 86, CTRL | SHIFT);
  await until("the paste question over its pane", () =>
    exists(`${slot(c)} [data-paste-confirm]`),
  );
  const pastePlace = await placedOver(`${slot(c)} [data-paste-confirm]`, c);
  if (!pastePlace.inside || pastePlace.overlaps) {
    fail(
      `the paste question is not over its own pane only: ${json(pastePlace)}`,
    );
  }
  await screenshot("paste-over-its-pane");
  await evaluate(
    `document.querySelector(${json(`${slot(a)} .terminal textarea`)}).focus()`,
  );
  await type("size\r");
  await until("typing in the other pane during the paste question", async () =>
    /size\s*\d+x\d+[\s\S]*size\s*\d+x\d+/.test(await screenOfTab(a)),
  );
  if (!(await exists(`${slot(c)} [data-paste-confirm]`))) {
    fail("typing in another pane cancelled the paste question");
  }
  await evaluate(
    `document.querySelector(${json(`${slot(c)} [data-paste=cancel]`)}).focus()`,
  );
  await press("Escape", "Escape", 27, 0);
  await until(
    "the paste question cancelled by Escape",
    async () => !(await exists(`${slot(c)} [data-paste-confirm]`)),
  );
  if ((await screenOfTab(c)).includes("pane-one")) {
    fail("a cancelled paste reached the device");
  }
  step(
    "a paste question covered only its pane; the other pane took typing meanwhile; Escape cancelled it",
  );

  // Closing a tab with a live session asks first, over its pane; Cancel
  // keeps it, and so does Escape.
  await clickOn(`[data-tab="${c}"] [data-tab-close]`);
  await until("the close confirmation", () => exists("[data-confirm-close]"));
  const closePlace = await placedOver("[data-confirm-close] form", c);
  if (!closePlace.inside || closePlace.overlaps || closePlace.pane !== cPane) {
    fail(`the close question is not over its pane: ${json(closePlace)}`);
  }
  await screenshot("close-confirmation");
  await press("Escape", "Escape", 27, 0);
  await until(
    "the close question cancelled by Escape",
    async () => !(await exists("[data-confirm-close]")),
  );
  if (!(await allTabs()).some((t) => t.id === c)) fail("Escape closed the tab");
  await clickOn(`[data-tab="${c}"] [data-tab-close]`);
  await confirmClose("cancel");
  if (!(await allTabs()).some((t) => t.id === c)) fail("Cancel closed the tab");
  // The shortcut from the terminal (the focus may still be returning
  // from the question's Cancel).
  await evaluate(
    `document.querySelector(${json(`${slot(c)} .terminal textarea`)}).focus()`,
  );
  await press("W", "KeyW", 87, CTRL | SHIFT);
  await confirmClose("close");
  await until("the tab closed", async () =>
    (await allTabs()).every((t) => t.id !== c),
  );
  step(
    "closing a live session's tab asked for confirmation (Ctrl+Shift+W too)",
  );

  // Closing a pane gives its tabs to the pane beside it.
  await paneTool(
    evaluate,
    `document.querySelector('.pane[data-pane="${(await layout())[1].pane}"] [data-tab-bar]')`,
    "close-pane",
    "the second pane",
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

// Multi-send (#35), on three of the sessions tabsAndPanes left open: two
// join the group (header button, Ctrl+Shift+M), what is typed in one
// reaches both (also the hidden one) and not the third; a disconnected
// member is marked and skipped, and receives again after reconnecting;
// ending the group makes typing local again. Safety (#36): a production
// session (and a pane holding one) asks before joining, Cancel focused; a
// multi-line paste asks once for the whole group, then reaches every
// member; a snippet is filled with each member's own values.
async function multiSend({
  evaluate,
  devtools,
  doubleClick,
  fillForm,
  text,
  status,
  screenshot,
  step,
}) {
  const json = (value) => JSON.stringify(value);
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
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
  const press = async (key, code, windowsVirtualKeyCode, modifiers) => {
    const event = { key, code, windowsVirtualKeyCode, modifiers };
    await devtools.send("Input.dispatchKeyEvent", {
      type: "keyDown",
      ...event,
    });
    await devtools.send("Input.dispatchKeyEvent", { type: "keyUp", ...event });
  };
  const toggleShortcut = async () => {
    const event = {
      key: "M",
      code: "KeyM",
      windowsVirtualKeyCode: 77,
      modifiers: CTRL | SHIFT,
    };
    await devtools.send("Input.dispatchKeyEvent", {
      type: "keyDown",
      ...event,
    });
    await devtools.send("Input.dispatchKeyEvent", { type: "keyUp", ...event });
  };
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
  // Each tab's id, state and multi-send marker, in order.
  const tabs = () =>
    evaluate(`[...document.querySelectorAll("[role=tab]")].map((tab) => ({
      id: Number(tab.dataset.tab),
      state: tab.dataset.tabState,
      member: tab.dataset.multiSend ?? null,
      label: !!tab.querySelector("[data-tab-multi-send]"),
    }))`);
  const tabOf = async (id) => (await tabs()).find((tab) => tab.id === id);
  const slot = (tab) => `.slot[data-tab="${tab}"]`;
  const screenOfTab = (tab) => screenOf(evaluate, slot(tab));
  const show = async (tab) => {
    await clickOn(`[data-tab="${tab}"]`);
    await until(`tab ${tab} shown`, () => exists(`${slot(tab)}[data-active]`));
  };
  // A line the scripted shell echoes, as a command of its own.
  const ran = async (tab, word) =>
    new RegExp(`\\$ ${word}\\s*(\\n|$)`).test(await screenOfTab(tab));
  const barSize = (tab) =>
    evaluate(
      `document.querySelector(${json(`${slot(tab)} [data-multi-send-bar]`)})?.dataset.groupSize ?? null`,
    );
  const focusTerminal = (tab) =>
    evaluate(
      `document.querySelector(${json(`${slot(tab)} .terminal textarea`)}).focus()`,
    );
  // A shell command's answer or its refusal, as this page gets it.
  const invokeResult = async (command, args) => {
    await evaluate(`(() => {
      window.__smokeResult = null;
      window.__TAURI_INTERNALS__
        .invoke(${json(command)}, ${json(args)})
        .then((value) => (window.__smokeResult = { value: value ?? null }), (error) => (window.__smokeResult = { error: String(error) }));
    })()`);
    return until(`the answer to ${command}`, () =>
      evaluate(`window.__smokeResult`),
    );
  };
  // The message the window shows in its Hosts view, if any.
  const editError = () =>
    evaluate(`document.querySelector(".edit-error span")?.textContent ?? null`);
  // Shows each of `tabs` (held output is written once shown) and fails if
  // `word` reached it.
  const reachedNone = async (tabs, word, what) => {
    for (const tab of tabs) {
      await show(tab);
      await sleep(300);
      if ((await screenOfTab(tab)).includes(word)) {
        fail(`${what}: it reached tab ${tab}`);
      }
    }
  };

  const [a, b, c] = (await tabs()).map((tab) => tab.id);
  if (c === undefined) fail("multiSend needs three open sessions");
  if ((await tabs()).some((tab) => tab.member !== null)) {
    fail("a tab is in a multi-send group before any joined");
  }

  // The session header's button adds its tab; Ctrl+Shift+M (with the
  // terminal focused) adds the next one.
  await show(a);
  await sessionAction(evaluate, "multi-send", slot(a));
  await until("the first member's marker", async () => {
    const tab = await tabOf(a);
    return tab.member === "receiving" && tab.label;
  });
  if ((await barSize(a)) !== "1") fail("the bar does not count one member");
  await show(b);
  await evaluate(
    `document.querySelector(${json(`${slot(b)} .terminal textarea`)}).focus()`,
  );
  await toggleShortcut();
  await until(
    "both members counted",
    async () => (await barSize(a)) === "2" && (await barSize(b)) === "2",
  );
  const marked = await tabs();
  if (
    marked.find((t) => t.id === b).member !== "receiving" ||
    marked.find((t) => t.id === c).member !== null ||
    (await exists(`${slot(c)} [data-multi-send-bar]`))
  ) {
    fail(`unexpected multi-send markers: ${json(marked)}`);
  }
  await screenshot("multi-send-group");
  step(
    "two tabs joined the multi-send group (header button, Ctrl+Shift+M), marked on their tabs and terminals",
  );

  // Typed in b: a (hidden) gets it too, c does not.
  await type("multisend1\r");
  await until("the typing in the member typed in", () => ran(b, "multisend1"));
  await show(a);
  await until("the typing in the hidden member", () => ran(a, "multisend1"));
  await show(c);
  await sleep(300);
  if (await ran(c, "multisend1")) fail("the tab outside the group got it");
  step("typing in one member reached every member, hidden ones too, only them");

  // A disconnected member is marked and skipped; reconnected, it receives.
  await show(a);
  await sessionAction(evaluate, "disconnect", slot(a));
  await until(
    "the disconnected member marked",
    async () =>
      (await tabOf(a)).member === "not-receiving" &&
      (await exists(`${slot(a)} [data-not-receiving]`)),
  );
  await checkHeaderLine(evaluate, slot(a), [
    "[data-multi-send-bar] [data-not-receiving]",
  ]);
  await screenshot("multi-send-not-receiving");
  await show(b);
  await type("multisend2\r");
  await until("the typing in the connected member", () => ran(b, "multisend2"));
  await show(a);
  await sleep(300);
  if (await ran(a, "multisend2")) fail("the disconnected member got input");
  await type("\r");
  await until(
    "the member reconnected and receiving",
    async () => (await tabOf(a)).member === "receiving",
  );
  await until("the reconnected member's prompt", async () =>
    /reconnecting[\s\S]*test-ssh\$ *$/.test((await screenOfTab(a)).trimEnd()),
  );
  await type("multisend3\r");
  await until("the typing from the reconnected member", () =>
    ran(a, "multisend3"),
  );
  await show(b);
  await until("that typing in the other member", () => ran(b, "multisend3"));
  step(
    "a disconnected member was marked and skipped, and received again after reconnecting",
  );

  // A production session asks before it joins, with Cancel focused; so
  // does adding its pane, naming it.
  const prodFile = join(workspace, "ms-prod.yaml");
  writeFileSync(
    prodFile,
    `address: 127.0.0.1\nport: ${SSH_PORT}\nenvironment: production\npaste_line_delay: 300\n`,
  );
  const snippetsFile = join(workspace, "_snippets.yaml");
  const savedSnippets = existsSync(snippetsFile)
    ? readFileSync(snippetsFile, "utf8")
    : null;
  const savedClipboard = readClipboard();
  try {
    const host = '[data-host="ms-prod.yaml"]';
    await until("the production host in the tree", () => exists(host));
    await doubleClick(host);
    await until("the credentials prompt", async () =>
      (await text("main h2")).includes("ms-prod"),
    );
    await fillForm({ username: USERNAME, password: PASSWORD });
    await until(
      "the production session connected",
      async () => (await status()) === "Connected",
    );
    const d = Number(
      await evaluate(`document.querySelector("${SHOWN_SESSION}").dataset.tab`),
    );
    await until("the production session's prompt", async () =>
      (await screenOfTab(d)).includes("test-ssh$"),
    );
    const joinQuestion = () =>
      evaluate(`(() => {
        const card = document.querySelector("[data-confirm-join]");
        return card && {
          count: card.dataset.joinCount,
          production: [...card.querySelectorAll("[data-join-production]")].map((li) => li.dataset.joinProduction),
          focused: document.activeElement?.dataset.join ?? null,
          pane: card.closest("[data-pane-dialog]")?.dataset.paneDialog ?? null,
        };
      })()`);
    const groupSize = async () =>
      (await tabs()).filter((tab) => tab.member !== null).length;

    const pane = await evaluate(
      `document.querySelector(${json(slot(d))}).dataset.pane`,
    );

    // The shell itself refuses a production session's join that does not
    // say its question was answered (#123), by the tab or by its pane.
    for (const edit of [
      { kind: "joinGroup", tab: d, confirmed: false },
      { kind: "joinPane", pane: Number(pane), confirmed: false },
    ]) {
      const refused = await invokeResult("layout_edit", { edit });
      if (!refused.error?.includes("production session")) {
        fail(
          `an unconfirmed ${edit.kind} of a production session gave ${json(refused)}`,
        );
      }
    }
    if ((await tabOf(d)).member !== null || (await groupSize()) !== 2) {
      fail("a refused join changed the group");
    }
    step(
      "the shell refused a production session's join (tab and pane) that did not say its question was answered",
    );

    await paneTool(
      evaluate,
      `document.querySelector('.pane[data-pane="${pane}"] [data-tab-bar]')`,
      "join-pane",
      `pane ${pane}`,
    );
    const paneAsked = await until("the pane's join question", joinQuestion);
    if (
      json(paneAsked.production) !== json(["ms-prod"]) ||
      Number(paneAsked.count) < 2 ||
      paneAsked.focused !== "cancel" ||
      paneAsked.pane !== String(pane)
    ) {
      fail(`the pane's join question shows ${json(paneAsked)}`);
    }
    await clickOn("[data-join=cancel]");
    await until("the question gone", async () => !(await joinQuestion()));
    if ((await groupSize()) !== 2)
      fail("a cancelled pane join changed the group");

    await sessionAction(evaluate, "multi-send", slot(d));
    const asked = await until("the production join question", joinQuestion);
    if (
      json(asked) !==
      json({
        count: "1",
        production: ["ms-prod"],
        focused: "cancel",
        pane: String(pane),
      })
    ) {
      fail(`the production join question shows ${json(asked)}`);
    }
    await screenshot("multi-send-production-join");
    // Escape cancels it, as Cancel does.
    await evaluate(`document.querySelector("[data-join=cancel]").focus()`);
    await press("Escape", "Escape", 27, 0);
    await until("the question gone", async () => !(await joinQuestion()));
    if ((await tabOf(d)).member !== null)
      fail("Cancel added the production tab");
    await sessionAction(evaluate, "multi-send", slot(d));
    await until("the production join question", joinQuestion);
    await clickOn("[data-join=join]");
    await until(
      "the production member",
      async () => (await tabOf(d)).member === "receiving",
    );
    step(
      "a production session (and its pane) asked before joining, Cancel focused; Cancel kept it out, Add joined it",
    );

    // A member's own shortcuts stay its own: Ctrl+C with a selection
    // copies (no ^C reaches any member), Ctrl+Shift+F searches it alone.
    await show(b);
    await focusTerminal(b);
    await evaluate(
      `document.querySelector(${json(`${slot(b)} .terminal`)}).xterm.selectAll()`,
    );
    writeClipboard("nothing copied yet");
    await press("c", "KeyC", 67, CTRL);
    await until("the selection copied", () =>
      readClipboard().includes("multisend"),
    );
    await press("F", "KeyF", 70, CTRL | SHIFT);
    await until("the member's search bar", () =>
      exists(`${slot(b)} [data-terminal-search]`),
    );
    if (
      (await exists(`${slot(a)} [data-terminal-search]`)) ||
      (await exists(`${slot(d)} [data-terminal-search]`))
    ) {
      fail("the search opened in another member too");
    }
    await press("Escape", "Escape", 27, 0);
    await until(
      "the search bar closed",
      async () => !(await exists(`${slot(b)} [data-terminal-search]`)),
    );
    await focusTerminal(b);
    await type("mslocal\r");
    const interrupted = /\^C[^\n]*\n[^\n]*\$ mslocal/;
    for (const tab of [b, a, d]) {
      await show(tab);
      await until(`the typing in tab ${tab}`, () => ran(tab, "mslocal"));
      if (interrupted.test(await screenOfTab(tab))) {
        fail(`the copy's Ctrl+C reached tab ${tab}`);
      }
    }
    step(
      "in a member, Ctrl+C with a selection copied and reached no device, and the search opened in that member only",
    );

    // A paste begun in a tab outside the group stays that tab's when the
    // tab joins while the paste is asked about (#123): Send then pastes
    // into it alone, and no member gets a line.
    await show(c);
    await focusTerminal(c);
    writeClipboard("ms-alone-one\nms-alone-two\n");
    await pasteKeys();
    const alone = `${slot(c)} [data-paste-confirm]`;
    await until("the paste question in the tab outside", () => exists(alone));
    if (await exists(`${alone}[data-paste-members]`)) {
      fail("a paste outside the group was asked about as the group's");
    }
    await toggleShortcut();
    await until(
      "the tab joined while its paste is asked about",
      async () => (await tabOf(c)).member === "receiving",
    );
    await clickOn(`${slot(c)} [data-paste=send]`);
    await until("the paste in its own tab", async () => {
      const shown = await screenOfTab(c);
      return (
        shown.includes("$ ms-alone-one") && shown.includes("$ ms-alone-two")
      );
    });
    await reachedNone([a, b, d], "ms-alone", "a paste begun outside the group");
    await show(c);
    await focusTerminal(c);
    await toggleShortcut();
    await until(
      "the tab out of the group again",
      async () => (await tabOf(c)).member === null,
    );
    step(
      "a paste begun outside the group, its tab joining while it was asked about, reached that tab alone",
    );

    // A multi-line paste in one member asks once for the whole group, as
    // for production, naming every member; then each member gets it.
    await show(b);
    await evaluate(
      `document.querySelector(${json(`${slot(b)} .terminal textarea`)}).focus()`,
    );
    writeClipboard("ms-one\nms-two\n");
    await pasteKeys();
    const pasteQuestion = () =>
      evaluate(`(() => {
        const cards = document.querySelectorAll("[data-paste-confirm]");
        const card = document.querySelector(${json(`${slot(b)} [data-paste-confirm]`)});
        return card && {
          cards: cards.length,
          environment: card.dataset.pasteConfirm,
          members: card.dataset.pasteMembers,
          lines: card.querySelector("[data-paste-lines]").dataset.pasteLines,
          names: [...card.querySelectorAll("[data-paste-member]")].map((li) => li.dataset.pasteMember),
          text: card.querySelector("[data-paste-text]").textContent.replace(/\\r\\n/g, "\\n"),
          focused: document.activeElement?.dataset.paste ?? null,
          overPane: (() => {
            const box = card.getBoundingClientRect();
            const own = card.closest(".slot").getBoundingClientRect();
            return box.left >= own.left - 1 && box.right <= own.right + 1 &&
              box.top >= own.top - 1 && box.bottom <= own.bottom + 1;
          })(),
        };
      })()`);
    const names = await evaluate(
      `Object.fromEntries([${[a, b, d].join(",")}].map((tab) => [tab, document.querySelector('[data-tab-bar] [data-tab="' + tab + '"]').textContent.trim()]))`,
    );
    const pasted = await until("the group's paste question", pasteQuestion);
    if (
      pasted.cards !== 1 ||
      pasted.environment !== "production" ||
      pasted.members !== "3" ||
      pasted.lines !== "2" ||
      pasted.names.length !== 3 ||
      !pasted.names.includes("ms-prod") ||
      pasted.text !== "ms-one\nms-two\n" ||
      pasted.focused !== "cancel" ||
      !pasted.overPane
    ) {
      fail(`the group's paste question shows ${json(pasted)} (${json(names)})`);
    }
    await screenshot("multi-send-paste");
    await clickOn(`${slot(b)} [data-paste=send]`);
    // Held output is written once a hidden tab is shown.
    for (const tab of [b, a, d]) {
      await show(tab);
      await until(`the pasted lines in tab ${tab}`, async () => {
        const shown = await screenOfTab(tab);
        return shown.includes("$ ms-one") && shown.includes("$ ms-two");
      });
    }
    await show(c);
    await sleep(300);
    if ((await screenOfTab(c)).includes("ms-one")) {
      fail("the tab outside the group got the paste");
    }
    step(
      "a multi-line paste in one member asked once (production, every member listed), then reached every member",
    );

    // The group changing while a group paste is asked about (a member
    // leaving, here from outside this page's own edits, as another window
    // would) closes the question and calls the paste off, saying so
    // (#123): nobody gets it.
    await show(b);
    await focusTerminal(b);
    writeClipboard("ms-off-one\nms-off-two\n");
    await pasteKeys();
    const offered = await until("the group's paste question", pasteQuestion);
    if (offered.members !== "3") {
      fail(`the group's paste question shows ${json(offered)}`);
    }
    const left = await invokeResult("layout_edit", {
      edit: { kind: "leaveGroup", tab: a },
    });
    if (left.error !== undefined)
      fail(`a member could not leave: ${left.error}`);
    await until("the question closed", async () => !(await pasteQuestion()));
    const calledOff = await until("the paste called off, said", editError);
    if (
      !calledOff.startsWith("Cannot paste to the multi-send group:") ||
      !calledOff.includes("changed") ||
      !calledOff.includes("nothing was pasted")
    ) {
      fail(`the called-off paste said ${json(calledOff)}`);
    }
    await screenshot("multi-send-paste-called-off");
    await reachedNone([b, a, d, c], "ms-off", "a called-off group paste");
    await clickOn(".edit-error .dismiss");
    await show(a);
    await sessionAction(evaluate, "multi-send", slot(a));
    await until(
      "the member back in the group",
      async () =>
        (await tabOf(a)).member === "receiving" && (await groupSize()) === 3,
    );
    step(
      "the group changing while a group paste was asked about closed the question and called the paste off, saying so; nobody got it",
    );

    // A snippet is filled per member: the production host gives its own
    // name; the quick connects have none, so it is asked once, for both.
    writeFileSync(snippetsFile, "Who: who-{{name}}\n");
    await showSideView(evaluate, "snippets");
    await until("the snippet in the panel", () =>
      exists('[data-snippet="Who"]'),
    );
    await show(b);
    await clickOn('[data-snippet="Who"]');
    const variables = await until("the variables asked for", () =>
      evaluate(`(() => {
        const dialog = document.querySelector("${SHOWN_SESSION} [data-snippet-variables]");
        return dialog && [...dialog.querySelectorAll("[data-variable]")].map((i) => i.dataset.variable);
      })()`),
    );
    if (json(variables) !== json(["name"])) {
      fail(`the group snippet asked for ${json(variables)}`);
    }
    await fillForm(
      { "variable-0": "quick" },
      `${SHOWN_SESSION} [data-snippet-variables] form`,
    );
    for (const tab of [b, a]) {
      await show(tab);
      await until(`the answer in tab ${tab}`, async () =>
        (await screenOfTab(tab)).includes("$ who-quick"),
      );
    }
    await show(d);
    await until("the production member's own value", async () =>
      (await screenOfTab(d)).includes("$ who-ms-prod"),
    );
    if ((await screenOfTab(d)).includes("who-quick")) {
      fail("the production member got the answer instead of its own name");
    }
    await showSideView(evaluate, "hosts");
    step(
      "a snippet sent to the group was filled per member; the missing variable was asked once",
    );

    // The production tab goes, with its host.
    await clickOn(`[data-tab="${d}"] [data-tab-close]`);
    await until("the close question", () => exists("[data-confirm-close]"));
    await clickOn("[data-confirm=close]");
    await until("the production tab closed", async () =>
      (await tabs()).every((tab) => tab.id !== d),
    );
    await show(b);
    await evaluate(
      `document.querySelector(${json(`${slot(b)} .terminal textarea`)}).focus()`,
    );
  } finally {
    writeClipboard(savedClipboard);
    if (savedSnippets === null) rmSync(snippetsFile, { force: true });
    else writeFileSync(snippetsFile, savedSnippets);
    rmSync(prodFile, { force: true });
  }

  // Ending the group: no markers, typing is local again.
  await sessionAction(evaluate, "end-group", slot(b));
  await until(
    "the group ended",
    async () =>
      (await tabs()).every((tab) => tab.member === null) &&
      !(await exists("[data-multi-send-bar]")),
  );
  await type("multisend4\r");
  await until("the local typing", () => ran(b, "multisend4"));
  await show(a);
  await sleep(300);
  if (await ran(a, "multisend4")) fail("typing still reached the old member");
  await show(b);
  step("ending the group removed the markers and made typing local again");
}

/**
 * Session windows (#84), on the sessions `multiSend` leaves (three, put
 * in the group again here). A member moved to a window of its own (from
 * its menu) keeps its header, colours and size there, and stays in the
 * group (#87): typing in it reaches the members in the main window and
 * the other way round, a two-line group paste there is asked once there
 * and arrives whole, once, in each member, and leaving, joining and
 * ending the group there show in both windows. Dock back returns it. Moved
 * again by the shortcut while a numbered listing arrives, the listing is
 * unbroken; a full-screen program's modes survive Dock back. A question
 * open stops a move. A closed session moves and reconnects there; a
 * reloaded session window takes its session again, saying what was lost;
 * closing the window docks the session back.
 */
async function sessionWindows({ evaluate, devtools, screenshot, step }) {
  const json = (value) => JSON.stringify(value);
  const clickOn = (selector) => mouseClick(evaluate, selector);
  const exists = (selector) =>
    evaluate(`!!document.querySelector(${json(selector)})`);
  const CTRL = 2;
  const SHIFT = 8;
  const keysOn = (client) => ({
    async press(key, code, windowsVirtualKeyCode, modifiers = 0) {
      const event = { key, code, windowsVirtualKeyCode, modifiers };
      await client.send("Input.dispatchKeyEvent", {
        type: "keyDown",
        ...event,
      });
      await client.send("Input.dispatchKeyEvent", { type: "keyUp", ...event });
    },
    async type(text) {
      for (const char of text) {
        const key =
          char === "\r"
            ? { key: "Enter", code: "Enter", windowsVirtualKeyCode: 13 }
            : { key: char };
        await client.send("Input.dispatchKeyEvent", {
          type: "keyDown",
          text: char,
          ...key,
        });
        await client.send("Input.dispatchKeyEvent", { type: "keyUp", ...key });
      }
    },
  });
  const main = keysOn(devtools);
  const moveShortcut = (keys) => keys.press("N", "KeyN", 78, CTRL | SHIFT);
  const tabs = () =>
    evaluate(`[...document.querySelectorAll("[role=tab]")].map((tab) => ({
      id: Number(tab.dataset.tab),
      state: tab.dataset.tabState,
      member: tab.dataset.multiSend ?? null,
      name: tab.querySelector("[data-tab-name]")?.textContent ?? "",
    }))`);
  const slot = (tab) => `.slot[data-tab="${tab}"]`;
  const shown = SHOWN_SESSION;
  const show = async (tab) => {
    await clickOn(`[data-tab="${tab}"]`);
    await until(`tab ${tab} shown`, () => exists(`${slot(tab)}[data-active]`));
  };
  const focusTerminal = (target, selector) =>
    target(
      `document.querySelector(${json(`${selector} .terminal textarea`)}).focus()`,
    );
  /** Every line of the terminal's normal buffer (scrollback and screen). */
  const lines = (target, selector) =>
    target(`(() => {
      const terminal = document.querySelector(${json(`${selector} .terminal`)})?.xterm;
      if (!terminal) return [];
      const buffer = terminal.buffer.normal;
      const lines = [];
      for (let y = 0; y < buffer.length; y++)
        lines.push(buffer.getLine(y)?.translateToString(true) ?? "");
      return lines;
    })()`);
  const ran = async (target, selector, word) =>
    new RegExp(`\\$ ${word}\\s*(\\n|$)`).test(await screenOf(target, selector));
  const ranAnywhere = async (target, selector, word) =>
    (await lines(target, selector)).some((line) =>
      new RegExp(`\\$ ${word}\\s*$`).test(line),
    );
  /** What the session's header, border and terminal show. */
  const looks = (target, selector) =>
    target(`(() => {
      const session = document.querySelector(${json(`${selector} .session`)});
      const header = session?.querySelector("[data-session-header]");
      const terminal = session?.querySelector(".terminal")?.xterm;
      const buffer = terminal?.buffer.normal;
      let red = null;
      for (let y = (buffer?.length ?? 0) - 1; y >= 0 && !red; y--) {
        const line = buffer.getLine(y);
        const at = line?.translateToString(true).indexOf("down up") ?? -1;
        if (at < 0) continue;
        const cell = line.getCell(at);
        red = { palette: cell.isFgPalette(), color: cell.getFgColor(), bold: cell.isBold() };
      }
      return header && {
        title: header.querySelector(".title")?.textContent,
        protocol: header.querySelector("[data-session-protocol]")?.dataset.sessionProtocol ?? null,
        environment: session.dataset.sessionEnvironment,
        red,
        size: { cols: terminal.cols, rows: terminal.rows },
      };
    })()`);
  const modes = (target, selector) =>
    target(`(() => {
      const terminal = document.querySelector(${json(`${selector} .terminal`)})?.xterm;
      if (!terminal) return null;
      const modes = terminal.modes;
      return {
        buffer: terminal.buffer.active.type,
        bracketedPaste: modes.bracketedPasteMode,
        applicationCursor: modes.applicationCursorKeysMode,
        mouse: modes.mouseTrackingMode,
        encoding: terminal._core?.coreMouseService?.activeEncoding ?? null,
        screen: terminal.buffer.active.getLine(0)?.translateToString(true) ?? "",
      };
    })()`);
  const openWindow = async (label) => {
    const window = await connectWindow(label);
    await until(`the session in ${label}`, () =>
      window.evaluate(
        `!!document.querySelector(".slot[data-active] .terminal")`,
      ),
    );
    return window;
  };
  const gone = (label) =>
    until(`${label} closed`, async () => !(await windowOpen(label)));
  const several = {
    evaluate,
    tabs,
    slot,
    show,
    keysOn,
    main,
    focusTerminal,
    lines,
    ran,
    ranAnywhere,
    openWindow,
    gone,
    step,
    screenshot,
  };
  // Alone (SMOKE_ONLY): its windows are the first two.
  if (ONLY === "severalInAWindow") {
    await severalInAWindow({ ...several, windows: [1, 2] });
    return;
  }
  // The moving session's tab here, if any: the main window keeps only
  // the other two members' tabs meanwhile.
  const movingTab = async () =>
    (await tabs()).find((tab) => !known.has(tab.id));
  const groupSizes = () =>
    evaluate(
      `[...document.querySelectorAll("[data-multi-send-bar]")].map((bar) => bar.dataset.groupSize)`,
    );

  // Three members, as multiSend left them, in the group again; the one
  // to move printed colours first (alone).
  const sessionTabs = (await tabs()).filter((tab) => tab.state);
  const [a, b, c] = sessionTabs.map((tab) => tab.id);
  if (c === undefined) fail("sessionWindows needs three open sessions");
  // Tabs seen before the moves: a session coming back gets a new one.
  const known = new Set((await tabs()).map((tab) => tab.id));
  const members = async () =>
    (await tabs()).filter((tab) => [a, b, c].includes(tab.id) && tab.member);
  const name = (await tabs()).find((tab) => tab.id === b).name;
  await show(b);
  await focusTerminal(evaluate, slot(b));
  await main.type("colors\r");
  await until("the colours printed", async () =>
    (await screenOf(evaluate, slot(b))).includes("down up"),
  );
  for (const tab of [a, b, c]) {
    await show(tab);
    await sessionAction(evaluate, "multi-send", slot(tab));
  }
  await until(
    `three members (tabs: ${JSON.stringify(await tabs())})`,
    async () => (await members()).length === 3,
  );
  await show(b);
  const before = await looks(evaluate, slot(b));

  // From the session's menu, a member: its tab closes here.
  await sessionAction(evaluate, "open-in-window", slot(b));
  let window = await openWindow("session-1");
  if (await movingTab()) fail("the moved session's tab is still here");
  for (const selector of [
    "[data-window-bar] [role=tab]",
    "[data-session-header]",
    "[data-status-line] [data-status-size]",
    "[data-window-controls]",
  ]) {
    if (!(await window.evaluate(`!!document.querySelector(${json(selector)})`)))
      fail(`the session window lacks ${selector}`);
  }
  for (const selector of [
    "[data-rail]",
    "[data-side-view]",
    "[data-welcome]",
    "[data-new-tab]",
    "[data-join-pane]",
  ]) {
    if (await window.evaluate(`!!document.querySelector(${json(selector)})`))
      fail(`the session window shows ${selector}`);
  }
  const title = await window.evaluate("document.title");
  if (!title.startsWith(name))
    fail(`the session window's title is ${json(title)}`);
  const after = await looks(window.evaluate, shown);
  const { size: sizeBefore, ...restBefore } = before;
  const { size: sizeAfter, ...restAfter } = after;
  if (json(restAfter) !== json(restBefore) || !after.red?.palette)
    fail(`the session moved as ${json(after)}, was ${json(before)}`);
  step(
    `Open in window moved it to "${title}": its tab, header, terminal and status line, no rail or views, the same header and colours`,
  );
  if (json(sizeAfter) !== json(sizeBefore))
    step(
      `(the screen did not allow ${json(sizeBefore)}: the terminal is ${json(sizeAfter)})`,
    );
  await checkContrast(window.evaluate, "session window");
  await checkOwnFrame(window.evaluate, "the session window");
  {
    const least = await narrowFrame(
      window.evaluate,
      "a session window with one pane",
      420,
    );
    step(
      `at its narrowest (${least} px) with one pane, the session window's three buttons stayed whole inside it at its top right, over nothing, beside an empty part that drags it, and Maximize and Restore answered`,
    );
  }

  // It stays in the group (#87): every member, in both windows, counts
  // three.
  const windowSizes = () =>
    window.evaluate(
      `[...document.querySelectorAll("[data-multi-send-bar]")].map((bar) => bar.dataset.groupSize)`,
    );
  const sizesAre = (description, inMain, inWindow) =>
    until(description, async () => {
      const here = await groupSizes();
      const there = await windowSizes();
      return (
        json(here) === json(inMain) && json(there) === json(inWindow) && true
      );
    }).catch(async (error) =>
      fail(
        `${error.message}: main ${json(await groupSizes())}, window ${json(await windowSizes())}`,
      ),
    );
  await sizesAre("the group of three in both windows", ["3", "3"], ["3"]);
  if (
    !(await window.evaluate(
      `!!document.querySelector("[data-tab][data-multi-send]")`,
    ))
  )
    fail("the moved member's tab has no multi-send marker");
  step(
    "the member that moved stayed in the group: its window and the main window show Multi-send 3",
  );

  // Typing in it reaches the members in the main window, without a click.
  const keys = keysOn(window.devtools);
  await until("the session window's terminal focused", () =>
    window.evaluate(
      `document.hasFocus() && !!document.activeElement?.closest(".terminal")`,
    ),
  );
  await keys.type("winmoved\r");
  await until("the typing echoed in the session window", () =>
    ran(window.evaluate, shown, "winmoved"),
  );
  for (const tab of [a, c]) {
    await show(tab);
    await until(`the typing in member ${tab} of the main window`, () =>
      ranAnywhere(evaluate, slot(tab), "winmoved"),
    );
  }
  // And typing in the main window reaches the session window's member.
  await focusTerminal(evaluate, slot(c));
  await main.type("frommain\r");
  await until("the main window's typing in the session window", () =>
    ranAnywhere(window.evaluate, shown, "frommain"),
  );
  step(
    "typing in a member in either window reached the members in the other window",
  );
  if (screenshot) await screenshot("session-window");

  // A two-line paste there: asked once, there, naming every member;
  // each gets both lines once.
  const countOf = async (target, selector, word) =>
    (await lines(target, selector)).filter((line) =>
      new RegExp(`\\$ ${word}\\s*$`).test(line),
    ).length;
  const pastedClipboard = readClipboard();
  try {
    writeClipboard("grpone\ngrptwo\n");
    await focusTerminal(window.evaluate, shown);
    await keys.press("V", "KeyV", 86, CTRL | SHIFT);
    const question = await until("the group paste question there", () =>
      window.evaluate(`(() => {
        const question = document.querySelector(".slot[data-active] [data-paste-confirm]");
        return question && {
          members: question.dataset.pasteMembers,
          names: [...question.querySelectorAll("[data-paste-member]")].map((member) => member.dataset.pasteMember),
        };
      })()`),
    );
    if (question.members !== "3")
      fail(`the group paste question named ${json(question)}`);
    if (await exists("[data-paste-confirm]"))
      fail("the group paste was asked in the main window too");
    await mouseClick(window.evaluate, `[data-paste=send]`);
    const targets = [
      [window.evaluate, shown, "the session window's member"],
      [evaluate, slot(a), `member ${a}`],
      [evaluate, slot(c), `member ${c}`],
    ];
    for (const [target, selector, where] of targets) {
      if (selector !== shown) await show(Number(/\d+/.exec(selector)[0]));
      await until(
        `the paste whole in ${where}`,
        async () => (await countOf(target, selector, "grptwo")) >= 1,
      );
    }
    await sleep(500);
    for (const [target, selector, where] of targets) {
      const once = [
        await countOf(target, selector, "grpone"),
        await countOf(target, selector, "grptwo"),
      ];
      if (json(once) !== json([1, 1]))
        fail(`${where} got the paste's lines ${json(once)} times`);
    }
  } finally {
    writeClipboard(pastedClipboard);
  }
  step(
    "a two-line paste in the session window was asked once there, naming 3 members, and arrived whole, once, in each member of both windows",
  );

  // From the main window: a cancelled paste reaches nobody; a confirmed
  // one reaches the session window's member once.
  const fromMain = readClipboard();
  try {
    writeClipboard("grpnone\ngrpnone2\n");
    await show(c);
    await focusTerminal(evaluate, slot(c));
    await main.press("V", "KeyV", 86, CTRL | SHIFT);
    const asked = await until(
      "the group paste question in the main window",
      () =>
        evaluate(
          `document.querySelector(${json(`${slot(c)} [data-paste-confirm]`)})?.dataset.pasteMembers ?? null`,
        ),
    );
    if (asked !== "3")
      fail(`the main window's question named ${asked} members`);
    if (
      await window.evaluate(`!!document.querySelector("[data-paste-confirm]")`)
    )
      fail("the main window's group paste was asked in the session window too");
    await clickOn(`${slot(c)} [data-paste=cancel]`);
    await sleep(800);
    for (const [target, selector, where] of [
      [window.evaluate, shown, "the session window's member"],
      [evaluate, slot(c), `member ${c}`],
    ]) {
      if ((await countOf(target, selector, "grpnone")) > 0)
        fail(`a cancelled group paste reached ${where}`);
    }
    writeClipboard("grpmain\ngrpmain2\n");
    await focusTerminal(evaluate, slot(c));
    await main.press("V", "KeyV", 86, CTRL | SHIFT);
    await until("the second group paste question", () =>
      exists(`${slot(c)} [data-paste-confirm]`),
    );
    await clickOn(`${slot(c)} [data-paste=send]`);
    await until(
      "the main window's paste in the session window",
      async () => (await countOf(window.evaluate, shown, "grpmain2")) >= 1,
    );
    await sleep(500);
    const once = [
      await countOf(window.evaluate, shown, "grpmain"),
      await countOf(window.evaluate, shown, "grpmain2"),
      await countOf(window.evaluate, shown, "grpnone"),
    ];
    if (json(once) !== json([1, 1, 0]))
      fail(`the session window's member got ${json(once)}`);
  } finally {
    writeClipboard(fromMain);
  }
  step(
    "from the main window, a cancelled group paste reached nobody, and a confirmed one reached the session window's member once",
  );

  // Leave, join and end the group from the session window.
  await sessionAction(window.evaluate, "multi-send", shown);
  await sizesAre("the group of two after leaving there", ["2", "2"], []);
  await sessionAction(window.evaluate, "multi-send", shown);
  await sizesAre("the group of three after joining there", ["3", "3"], ["3"]);
  await sessionAction(window.evaluate, "end-group", shown);
  await sizesAre("no group after ending it there", [], []);
  if ((await members()).length > 0)
    fail(`the main window still shows members: ${json(await members())}`);
  step(
    "leaving, joining and ending the group in the session window showed in both windows",
  );

  // Dock back from its menu: a tab here, the window gone.
  await sessionAction(window.evaluate, "dock", shown);
  window.devtools.close();
  await gone("session-1");
  let back = await until("the session back in the main window", () =>
    movingTab(),
  );
  await show(back.id);
  if (!(await ranAnywhere(evaluate, slot(back.id), "winmoved")))
    fail("Dock back lost the scrollback");
  step("Dock back returned it to a tab here, with its scrollback");

  // By the shortcut, while a listing arrives: unbroken in the window.
  await focusTerminal(evaluate, slot(back.id));
  await main.type("lines 9000\r");
  await moveShortcut(main);
  window = await openWindow("session-2");
  await until("the whole listing in the session window", async () =>
    (await lines(window.evaluate, shown)).includes("line 9000"),
  );
  const listed = (await lines(window.evaluate, shown))
    .map((line) => /^line (\d+)$/.exec(line))
    .filter(Boolean)
    .map((match) => Number(match[1]));
  const run = listed.slice(listed.lastIndexOf(1));
  if (run.length !== 9000 || run.some((n, index) => n !== index + 1))
    fail(
      `the listing moved mid-output is broken: ${run.length} lines, ${run[0]} to ${run.at(-1)}`,
    );
  step("a 9,000-line listing moved by Ctrl+Shift+N arrived once and in order");

  // A full-screen program moves back with its screen and modes.
  const moved = keysOn(window.devtools);
  await moved.type("mouse on\r");
  await moved.type("program\r");
  const held = await until("the program on screen", async () => {
    const now = await modes(window.evaluate, shown);
    return now?.buffer === "alternate" && now.bracketedPaste && now;
  });
  await moveShortcut(moved);
  window.devtools.close();
  await gone("session-2");
  back = await until("the session docked back", () => movingTab());
  await show(back.id);
  const kept = await until("the program's screen here", async () => {
    const now = await modes(evaluate, slot(back.id));
    return now?.screen.includes("full screen program") && now;
  });
  for (const key of [
    "buffer",
    "bracketedPaste",
    "applicationCursor",
    "mouse",
    "encoding",
  ]) {
    if (kept[key] !== held[key])
      fail(`docking back lost ${key}: ${json(kept)} after ${json(held)}`);
  }
  await focusTerminal(evaluate, slot(back.id));
  await main.type("program off\r");
  await main.type("mouse off\r");
  await until(
    "the main screen again",
    async () => (await modes(evaluate, slot(back.id)))?.buffer === "normal",
  );
  if (!(await lines(evaluate, slot(back.id))).includes("line 9000"))
    fail("docking back lost the listing");
  step(
    "Dock back (Ctrl+Shift+N) kept the program's alternate screen, bracketed paste, cursor keys and mouse reporting, and the scrollback",
  );

  // Not while a question is open: a multi-line paste waits for its yes.
  const savedClipboard = readClipboard();
  try {
    writeClipboard("show clock\nshow version\n");
    await focusTerminal(evaluate, slot(back.id));
    await main.press("V", "KeyV", 86, CTRL | SHIFT);
    await until("the paste question", () =>
      exists(`${slot(back.id)} [data-paste-confirm]`),
    );
    const menu = `${slot(back.id)} [data-session-menu]`;
    await clickOn(menu);
    const refused = await until("the menu's Open in window", () =>
      evaluate(`(() => {
        const entry = document.querySelector("[data-session-actions] [data-session-action=open-in-window]");
        return entry && { disabled: entry.disabled, title: entry.title };
      })()`),
    );
    await clickOn(menu);
    if (!refused.disabled || !/question/.test(refused.title))
      fail(`Open in window was offered with a question open: ${json(refused)}`);
    await moveShortcut(main);
    await sleep(500);
    if (await windowOpen("session-3"))
      fail("the shortcut moved a session with a question open");
    await clickOn(`${slot(back.id)} [data-paste=cancel]`);
  } finally {
    writeClipboard(savedClipboard);
  }
  step(
    "with a paste question open, Open in window was disabled, saying why, and the shortcut did nothing",
  );

  // A closed session moves, and reconnects there with Enter.
  await focusTerminal(evaluate, slot(back.id));
  await main.type("exit\r");
  await until("the session closed", async () =>
    (await tabs()).some(
      (tab) => tab.id === back.id && tab.state === "disconnected",
    ),
  );
  await moveShortcut(main);
  window = await openWindow("session-3");
  await until("the closed session in its window", () =>
    window.evaluate(
      `!!document.querySelector("[data-tab-state=disconnected]") && !!document.querySelector("[data-session-ended]")`,
    ),
  );
  await keysOn(window.devtools).press("Enter", "Enter", 13);
  await until("reconnected in the session window", () =>
    window.evaluate(`!!document.querySelector("[data-tab-state=connected]")`),
  );
  step("a closed session moved, and Enter reconnected it in its window");

  // A reload keeps the session, saying its earlier output was lost.
  await window.devtools.send("Page.reload");
  window.devtools.close();
  await sleep(500);
  window = await openWindow("session-3");
  await until("the lost-output line", async () =>
    (await screenOf(window.evaluate, shown)).includes(
      "earlier output was lost",
    ),
  );
  await focusTerminal(window.evaluate, shown);
  await keysOn(window.devtools).type("reloaded\r");
  // (Its prompt went with the lost output: the echo has none before it.)
  const echoed = async (target, selector) =>
    (await lines(target, selector)).some((line) => /^reloaded\s*$/.test(line));
  await until("typing after the reload", () =>
    echoed(window.evaluate, shown),
  ).catch(async (error) => {
    const seen = await window.evaluate(`({
      status: document.querySelector("[role=status]")?.textContent ?? null,
      error: document.querySelector("[data-session-window-error]")?.textContent ?? null,
      focus: document.hasFocus(),
      active: document.activeElement?.className ?? null,
    })`);
    fail(
      `${error.message}; window ${json(seen)}; screen ${json(await screenOf(window.evaluate, shown))}`,
    );
  });
  step(
    "a reloaded session window took its session again, connected, saying earlier output was lost",
  );

  // Its Close button asks (#88): Dock back returns the session.
  await mouseClick(window.evaluate, `[data-window-control=close]`, {
    mayClose: true,
  });
  await until("the close question", () =>
    window.evaluate(`!!document.querySelector("[data-confirm-close-window]")`),
  );
  await mouseClick(window.evaluate, `[data-close-window=dock]`, {
    mayClose: true,
  });
  window.devtools.close();
  await gone("session-3");
  // Connecting until its terminal has attached: then connected.
  back = await until(
    "the session docked back on close, connected",
    async () => {
      const tab = await movingTab();
      return tab?.state === "connected" && tab;
    },
  ).catch(async (error) =>
    fail(
      `${error.message}: closing the window left ${JSON.stringify(await movingTab())}`,
    ),
  );
  await show(back.id);
  if (!(await echoed(evaluate, slot(back.id))))
    fail("closing the window lost the session's scrollback");
  step(
    "closing the session window asked, and Dock back returned its session, with its scrollback",
  );

  // The group as multiSend left it: ended (in the session window, above).

  await severalInAWindow({ ...several, windows: [4, 5] });
}

/**
 * Several sessions in one session window (#85): moved in from the main
 * window and from another session window (which then closes), the tab
 * shortcuts on that window's tabs only, Duplicate there, a split with
 * typing in each pane, search, copy, paste and highlighting there, zoom
 * and theme from the main window, the title following the active tab,
 * and Dock all back. `windows`: the numbers of the two session windows it
 * opens (4 and 5 after those sessionWindows used, session-1 to -3; 1 and 2
 * when run alone).
 */
async function severalInAWindow({
  windows: [first, second],
  evaluate,
  tabs,
  slot,
  show,
  keysOn,
  main,
  focusTerminal,
  lines,
  ran,
  ranAnywhere,
  openWindow,
  gone,
  step,
  screenshot,
}) {
  const json = (value) => JSON.stringify(value);
  const CTRL = 2;
  const SHIFT = 8;
  const connected = (await tabs()).filter((tab) => tab.state === "connected");
  if (connected.length < 3)
    fail(`severalInAWindow needs three connected sessions: ${json(connected)}`);
  const [x, y, z] = connected;
  const known = new Set((await tabs()).map((tab) => tab.id));
  const windowTabs = (window) =>
    window.evaluate(`[...document.querySelectorAll("[role=tab]")].map((tab) => ({
      id: Number(tab.dataset.tab),
      state: tab.dataset.tabState,
      name: tab.querySelector("[data-tab-name]")?.textContent ?? "",
    }))`);
  const shownTab = (target) =>
    target(
      `Number(document.querySelector("${SHOWN_SESSION}")?.dataset.tab ?? -1)`,
    );
  const titleOf = (window) => window.evaluate("document.title");

  // A new window from the main window, then a second session into it.
  await show(x.id);
  await sessionAction(evaluate, "open-in-window", slot(x.id));
  const window = await openWindow(`session-${first}`);
  await show(y.id);
  await sessionAction(evaluate, `open-in-window-${first}`, slot(y.id));
  await until("two tabs in the session window", async () => {
    const shown = await windowTabs(window);
    return (
      shown.length === 2 && shown.every((tab) => tab.state === "connected")
    );
  });
  if ((await tabs()).some((tab) => tab.id === x.id || tab.id === y.id))
    fail("the moved sessions' tabs are still in the main window");
  await until(`the title naming ${y.name}`, async () =>
    (await titleOf(window)).startsWith(y.name),
  );
  step(
    "Open in window offered the open session window, and moving into it added a tab there, titled after it",
  );

  // From another session window: that window, emptied, closes.
  await show(z.id);
  await sessionAction(evaluate, "open-in-window", slot(z.id));
  const other = await openWindow(`session-${second}`);
  await sessionAction(other.evaluate, `open-in-window-${first}`);
  await gone(`session-${second}`);
  await until(
    "three tabs in the first session window",
    async () => (await windowTabs(window)).length === 3,
  );
  // In the order they came (each at the end of the active pane).
  const [xTab, yTab] = (await windowTabs(window)).map((tab) => tab.id);
  const xSlot = `.slot[data-tab="${xTab}"]`;
  const ySlot = `.slot[data-tab="${yTab}"]`;
  step(
    "a session moved from one session window into another; the emptied window closed itself",
  );

  // Tab shortcuts act on this window's tabs only.
  const keys = keysOn(window.devtools);
  const mainShown = await shownTab(evaluate);
  const before = await shownTab(window.evaluate);
  await focusTerminal(window.evaluate, SHOWN_SESSION);
  await keys.press("Tab", "Tab", 9, CTRL);
  await until("the next tab in the session window", async () => {
    const now = await shownTab(window.evaluate);
    return now !== before && now !== -1;
  });
  if ((await shownTab(evaluate)) !== mainShown)
    fail("Ctrl+Tab in the session window changed the main window's tab");
  const order = async () => (await windowTabs(window)).map((tab) => tab.id);
  const orderBefore = await order();
  await keys.press("PageDown", "PageDown", 34, CTRL | SHIFT);
  await until(
    "the tab moved right (it is the first)",
    async () => json(await order()) !== json(orderBefore),
  );
  await keys.press("PageUp", "PageUp", 33, CTRL | SHIFT);
  await until(
    "the tab moved back",
    async () => json(await order()) === json(orderBefore),
  );
  await until("the title following the active tab", async () => {
    const tab = await shownTab(window.evaluate);
    const name = (await windowTabs(window)).find(
      (each) => each.id === tab,
    )?.name;
    return !!name && (await titleOf(window)).startsWith(name);
  });
  step(
    "Ctrl+Tab and Ctrl+Shift+PageUp/PageDown acted on the session window's tabs only; its title followed the active tab",
  );

  // Duplicate opens beside the original, in this window.
  await sessionAction(window.evaluate, "duplicate", xSlot);
  const duplicated = await until(
    "the duplicate connected beside it",
    async () => {
      const shown = await windowTabs(window);
      const at = shown.findIndex((tab) => tab.id === xTab);
      const next = shown[at + 1];
      return (
        shown.length === 4 &&
        next &&
        !orderBefore.includes(next.id) &&
        next.name === x.name &&
        next.state === "connected" &&
        next
      );
    },
  );
  await mouseClick(
    window.evaluate,
    `[data-tab="${duplicated.id}"] [data-tab-close]`,
  );
  await until("the close question", () =>
    window.evaluate(`!!document.querySelector("[data-confirm=close]")`),
  );
  await mouseClick(window.evaluate, `[data-confirm=close]`);
  await until(
    "three tabs again",
    async () => (await windowTabs(window)).length === 3,
  );
  step("Duplicate in the session window opened beside the original there");

  // Split: x and y side by side, each typing to its own device.
  await mouseClick(window.evaluate, `[data-tab="${xTab}"]`);
  await until("x shown", () =>
    window.evaluate(`!!document.querySelector('${xSlot}[data-active]')`),
  );
  await paneTool(
    window.evaluate,
    `document.querySelector('[data-tab="${xTab}"]').closest("[data-tab-bar]")`,
    "split-right",
    "the session window's pane",
  );
  await until("two panes", () =>
    window.evaluate(`document.querySelectorAll("[data-tab-bar]").length === 2`),
  );
  // At its narrowest, two panes side by side (#120).
  {
    const least = await narrowFrame(
      window.evaluate,
      "a session window with two panes side by side",
      420,
    );
    step(
      `at its narrowest (${least} px) with two panes side by side, a session window's three buttons stayed whole inside it at its top right, over nothing, beside an empty part that drags it, and Maximize and Restore answered`,
    );
  }
  // Narrow, it folds its top-right pane's tools too (#100): no new tab or
  // multi-send there.
  {
    const { windowId, bounds } = await window.devtools.send(
      "Browser.getWindowForTarget",
    );
    await window.devtools.send("Browser.setWindowBounds", {
      windowId,
      bounds: { width: 560, height: bounds.height },
    });
    const topRight = `document.querySelector(".bar.framed")`;
    await until("the narrow session window's tools folded", () =>
      window.evaluate(
        `!!(${topRight})?.querySelector("[data-pane-tools-more]")`,
      ),
    );
    const narrow = await foldedToolEntries(
      window.evaluate,
      topRight,
      "the narrow session window",
    );
    if (
      JSON.stringify(narrow) !==
      JSON.stringify(["split-right", "split-down", "close-pane"])
    )
      fail(`the session window's folded actions: ${JSON.stringify(narrow)}`);
    // Wide, they come back; then the window's own size again.
    await window.devtools.send("Browser.setWindowBounds", {
      windowId,
      bounds: { width: 1200, height: bounds.height },
    });
    await until(
      "the session window's tools back",
      async () =>
        !(await window.evaluate(
          `!!document.querySelector("[data-pane-tools-more]")`,
        )),
    );
    await reachable(window.evaluate, ".bar.framed [data-split=down]");
    await window.devtools.send("Browser.setWindowBounds", {
      windowId,
      bounds: { width: bounds.width, height: bounds.height },
    });
    step(
      "a narrow session window folded its top-right pane's tools into a menu (split right and down, close pane); at 1200 px they were back in the bar",
    );
  }
  const visible = (selector) =>
    window.evaluate(
      `(() => { const slot = document.querySelector('${selector}'); return !!slot && !slot.hidden; })()`,
    );
  const tail = async (slot) =>
    (await lines(window.evaluate, slot)).filter(Boolean).slice(-3);
  /** The tab whose slot has the keys (or the element's tag). */
  const keysWith = () =>
    window.evaluate(
      `document.activeElement?.closest(".slot")?.dataset.tab ?? document.activeElement?.tagName ?? null`,
    );
  const activeTab = () =>
    window.evaluate(
      `document.querySelector(${json(SHOWN_SESSION)})?.dataset.tab ?? null`,
    );
  /** Where the typed lines went and who has the keys, for a failure. */
  const whereTyped = async () =>
    `x (tab ${xTab}) ends ${json(await tail(xSlot))}; y (tab ${yTab}) ends ${json(await tail(ySlot))}; the keys are with ${json(await keysWith())}; the active tab is ${json(await activeTab())}`;
  // The split took the shown tab (x) into the new pane; the first shows y
  // or z: show y there. Then x is focused and typed in, then y (#141: once
  // in five runs on a loaded computer, both lines went to x). Done
  // SMOKE_SPLIT_TYPING_REPEAT times over, to repeat it under load.
  for (let round = 1; round <= SPLIT_TYPING_REPEAT; round++) {
    const [left, right] =
      round === 1
        ? ["inleft", "inright"]
        : [`inleft${round}`, `inright${round}`];
    await mouseClick(window.evaluate, `[data-tab="${yTab}"]`);
    await until(
      "both sessions side by side",
      async () => (await visible(xSlot)) && (await visible(ySlot)),
    );
    // The tab clicked hands its terminal the keys a moment later: once it
    // has, focusing x is not undone.
    await until("y's terminal given the keys", () =>
      window.evaluate(`!!document.activeElement?.closest(${json(ySlot)})`),
    );
    await focusTerminal(window.evaluate, xSlot);
    await keys.type(`${left}\r`);
    await focusTerminal(window.evaluate, ySlot);
    await keys.type(`${right}\r`);
    try {
      await until(
        "each typed line echoed in its own pane",
        async () =>
          (await ran(window.evaluate, xSlot, left)) &&
          (await ran(window.evaluate, ySlot, right)),
      );
    } catch (error) {
      fail(
        `${error.message} (round ${round} of ${SPLIT_TYPING_REPEAT}); ${await whereTyped()}`,
      );
    }
    if (
      (await ranAnywhere(window.evaluate, xSlot, right)) ||
      (await ranAnywhere(window.evaluate, ySlot, left))
    )
      fail(`typing in one pane reached the other's device (round ${round})`);
  }
  step(
    `split in the session window: two sessions side by side, each typing to its own device${SPLIT_TYPING_REPEAT > 1 ? ` (${SPLIT_TYPING_REPEAT} times)` : ""}`,
  );

  // The same with the core's layout answers held back (#141), as on a
  // computer too busy to answer: a focus given a moment later never takes
  // the keys from where they went meanwhile.
  const holdAnswers = () =>
    window.evaluate(`(() => {
      window.__smokeLayoutInFlight = 0;
      window.__smokeHoldLayoutAnswers = new Promise((resolve) => (window.__smokeReleaseLayoutAnswers = resolve));
    })()`);
  const releaseAnswers = () =>
    window.evaluate(`(() => {
      const release = window.__smokeReleaseLayoutAnswers;
      delete window.__smokeHoldLayoutAnswers;
      delete window.__smokeReleaseLayoutAnswers;
      release?.();
    })()`);
  /** Every held answer given, then `tab`'s pane active with the keys. */
  const settledWith = (what, tab) =>
    until(what, async () =>
      window.evaluate(`(() => {
        if ((window.__smokeLayoutInFlight ?? 0) !== 0) return false;
        const slot = document.querySelector(${json(`.slot[data-tab="${tab}"]`)});
        return !!slot && slot.matches("[data-active]") && !!document.activeElement && slot.contains(document.activeElement);
      })()`),
    ).catch(async (error) => fail(`${error.message}; ${await whereTyped()}`));
  // (1) Focus moved to another pane while the first pane's activation is
  // on its way: the first pane, made active by its answer, takes nothing.
  try {
    await holdAnswers();
    await focusTerminal(window.evaluate, xSlot);
    await keys.type("heldleft\r");
    await focusTerminal(window.evaluate, ySlot);
    if ((await keysWith()) !== String(yTab))
      fail(`focusing y left the keys with ${json(await keysWith())}`);
  } finally {
    await releaseAnswers();
  }
  await settledWith("y's pane active with the keys, the answers given", yTab);
  await keys.type("heldright\r");
  await until(
    "each line typed while the answers were held echoed in its own pane",
    async () =>
      (await ran(window.evaluate, xSlot, "heldleft")) &&
      (await ran(window.evaluate, ySlot, "heldright")),
  ).catch(async (error) => fail(`${error.message}; ${await whereTyped()}`));
  if (
    (await ranAnywhere(window.evaluate, xSlot, "heldright")) ||
    (await ranAnywhere(window.evaluate, ySlot, "heldleft"))
  )
    fail("typing in one pane reached the other's device (answers held)");
  // (2) A tab clicked (its terminal to take the keys once shown), then a
  // click on the terminal that has the keys before the answer: the keys
  // stay there, and its pane is made active again.
  const zTab = (await windowTabs(window))
    .map((tab) => tab.id)
    .find((id) => id !== xTab && id !== yTab);
  await focusTerminal(window.evaluate, xSlot);
  await settledWith("x's pane active with the keys", xTab);
  try {
    await holdAnswers();
    await mouseClick(window.evaluate, `[data-tab="${zTab}"]`);
    await mouseClick(window.evaluate, `${xSlot} .terminal`);
  } finally {
    await releaseAnswers();
  }
  await settledWith(
    "x's pane active again with the keys, the tab clicked before shown",
    xTab,
  );
  await keys.type("afterclick\r");
  await until("the line typed after the click echoed in x", () =>
    ran(window.evaluate, xSlot, "afterclick"),
  ).catch(async (error) => fail(`${error.message}; ${await whereTyped()}`));
  if (await ranAnywhere(window.evaluate, slot(zTab), "afterclick"))
    fail("the line typed after the click went to the tab clicked before");
  // y shown again in its pane, with the keys, for what follows.
  await mouseClick(window.evaluate, `[data-tab="${yTab}"]`);
  await until(
    "both sessions side by side again",
    async () => (await visible(xSlot)) && (await visible(ySlot)),
  );
  await until("y's terminal given the keys again", () =>
    window.evaluate(`!!document.activeElement?.closest(${json(ySlot)})`),
  );
  step(
    "with the core's layout answers held back, a pane made active late took no keys from the pane focused meanwhile, nor from a terminal clicked meanwhile",
  );
  if (screenshot) await screenshot("session-window-split");

  // Search, copy and paste, and highlighting there.
  await focusTerminal(window.evaluate, ySlot);
  await keys.press("F", "KeyF", 70, CTRL | SHIFT);
  await until("the search bar", () =>
    window.evaluate(
      `document.activeElement?.matches(${json(`${ySlot} [data-terminal-search] input[name=search]`)}) ?? false`,
    ),
  );
  await keys.type("inright");
  await until("a match", async () =>
    /^\d+ of [1-9]/.test(
      await window.evaluate(
        `document.querySelector(${json(`${ySlot} [data-search-results]`)}).textContent`,
      ),
    ),
  );
  await keys.press("Escape", "Escape", 27);
  const savedClipboard = readClipboard();
  try {
    await window.evaluate(`(() => {
      const terminal = document.querySelector('${ySlot} .terminal').xterm;
      const buffer = terminal.buffer.active;
      for (let y = buffer.length - 1; y >= 0; y--) {
        if (buffer.getLine(y).translateToString(true).includes("$ inright")) {
          terminal.selectLines(y, y);
          return;
        }
      }
    })()`);
    await focusTerminal(window.evaluate, ySlot);
    await keys.press("C", "KeyC", 67, CTRL | SHIFT);
    await until("the selection copied", () =>
      readClipboard().includes("inright"),
    );
    writeClipboard("pastedhere");
    await keys.press("V", "KeyV", 86, CTRL | SHIFT);
    // The paste goes through its plan first: Enter only once it is in.
    await until("the pasted text on the line", async () =>
      (await screenOf(window.evaluate, ySlot)).includes("$ pastedhere"),
    );
    await keys.type("\r");
    await until("the paste echoed", () =>
      ran(window.evaluate, ySlot, "pastedhere"),
    );
  } finally {
    writeClipboard(savedClipboard);
  }
  await keys.type("down\r");
  await until("down echoed", () => ran(window.evaluate, ySlot, "down")).catch(
    async (error) =>
      fail(
        `${error.message}; focus in ${await window.evaluate("document.activeElement?.outerHTML.slice(0, 120) ?? null")}; screen ${json(await screenOf(window.evaluate, ySlot))}`,
      ),
  );
  await until("the built-in rules colouring down", () =>
    window.evaluate(`(() => {
      const slot = document.querySelector('${ySlot} .terminal');
      const buffer = slot.xterm.buffer.active;
      return slot.highlights.shown().some((cell) =>
        buffer.getLine(cell.line)?.translateToString(true).slice(cell.col, cell.col + cell.width) === "down");
    })()`),
  ).catch(async (error) => {
    const seen = await window.evaluate(`(() => {
      const slot = document.querySelector('${ySlot} .terminal');
      const buffer = slot.xterm.buffer.active;
      const lines = [];
      for (let y = 0; y < buffer.length; y++) {
        const text = buffer.getLine(y).translateToString(true);
        if (text.trim()) lines.push([y, text]);
      }
      return {
        focus: document.hasFocus(),
        visibility: document.visibilityState,
        shown: slot.highlights.shown(),
        lines,
        viewport: buffer.viewportY,
        rows: slot.xterm.rows,
      };
    })()`);
    fail(`${error.message}; window ${json(seen)}`);
  });
  step(
    "search, copy, paste and keyword highlighting worked in the session window",
  );

  // Zoom and theme from the main window reach it.
  const fontSize = () =>
    window.evaluate(
      `document.querySelector('${ySlot} .terminal').xterm.options.fontSize`,
    );
  const size = await fontSize();
  await main.press("=", "Equal", 187, CTRL);
  await until(
    "the larger font there too",
    async () => (await fontSize()) === size + 1,
  );
  await main.press("0", "Digit0", 48, CTRL);
  await until(
    "the default font there too",
    async () => (await fontSize()) !== size + 1,
  );
  const theme = () => window.evaluate("document.documentElement.dataset.theme");
  const themeBefore = await theme();
  await openPreferences(evaluate);
  const choose = (value) =>
    evaluate(`(() => {
      const field = document.querySelector("[data-view=preferences] [name=preference-theme]");
      const before = field.value;
      field.value = ${json(value)};
      field.dispatchEvent(new Event("change", { bubbles: true }));
      return before;
    })()`);
  const chosenBefore = await choose(themeBefore === "dark" ? "light" : "dark");
  await until(
    "the other theme there too",
    async () => (await theme()) !== themeBefore,
  );
  await choose(chosenBefore);
  await until(
    "the theme back there too",
    async () => (await theme()) === themeBefore,
  );
  await closePreferences(evaluate);
  step(
    "a zoom and a theme change in the main window reached the session window",
  );

  // Dock all back: every session, connected, with its scrollback.
  await sessionAction(window.evaluate, "dock-all", ySlot).catch(
    async (error) => {
      await mouseClick(window.evaluate, `${ySlot} [data-session-menu]`);
      const entry = await until("the menu", () =>
        window.evaluate(`(() => {
          const entry = document.querySelector("[data-session-actions] [data-session-action=dock-all]");
          return entry ? { title: entry.title, disabled: entry.disabled } : document.querySelector("[data-session-actions]") && "none";
        })()`),
      );
      fail(`${error.message}: ${json(entry)}`);
    },
  );
  window.devtools.close();
  await gone(`session-${first}`);
  const docked = await until("the three sessions back", async () => {
    const back = (await tabs()).filter((tab) => !known.has(tab.id));
    return back.length === 3 && back;
  });
  // Each tab says connecting until its terminal has attached (as after
  // Dock back above): then connected. Read at once, a loaded computer
  // showed them still connecting (#141, not a focus fault).
  await until(
    "the three sessions back connected",
    async () =>
      (await tabs()).filter(
        (tab) => !known.has(tab.id) && tab.state === "connected",
      ).length === 3,
  ).catch(async (error) =>
    fail(
      `${error.message}: Dock all back left ${json((await tabs()).filter((tab) => !known.has(tab.id)))}`,
    ),
  );
  for (const word of ["inleft", "inright"]) {
    let found = false;
    for (const tab of docked) {
      await show(tab.id);
      if (await ranAnywhere(evaluate, slot(tab.id), word)) found = true;
    }
    if (!found) fail(`Dock all back lost the scrollback with ${word}`);
  }
  step(
    "Dock all back returned every session to the main window, connected, with its scrollback, and closed the window",
  );
}

let exitCode = 0;
try {
  await main();
  if (contrastFailures.size > 0) {
    const listed = [...contrastFailures].map(
      ([failure, where]) => `  ${failure} (at ${where})`,
    );
    fail(
      [`text under the contrast minimum (${listed.length}):`, ...listed].join(
        String.fromCharCode(10),
      ),
    );
  }
  console.log(
    "ok - every screen checked met the contrast minimum in both themes",
  );
  console.log("smoke test passed");
} catch (error) {
  console.error(`smoke test FAILED: ${error.message}`);
  exitCode = 1;
} finally {
  for (const child of children) child.kill();
  connections?.stop();
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
