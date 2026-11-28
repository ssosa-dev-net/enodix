// A terminal tab (`TerminalSession.svelte`) as the user drives it, mounted
// on the page with the real xterm.js; the shell on the other side of the
// IPC is `src/test/shell.ts`. Two things the core cannot check live here:
// every paste goes through the one paste path, asked first where the core
// says, and the terminal's input is not lost while the session opens.

import { flushSync, mount, unmount } from "svelte";
import { afterEach, beforeEach, describe, expect, test } from "vitest";
import TerminalSession from "./TerminalSession.svelte";
import { preferences } from "./preferences.svelte";
import type { PasteSafety } from "./workspace";
import {
  fakeShell,
  opened,
  pending,
  sentText,
  sessionEvents,
  settle,
  type FakeShell,
  type SessionChannels,
} from "../test/shell";

const ID = 7;
const SAFE: PasteSafety = { environment: null, lineDelayMs: 0 };

let shell: FakeShell;
let component: ReturnType<typeof mount> | null = null;
/** What the clipboard holds, as the webview reads it. */
let clipboard: string;

beforeEach(() => {
  shell = fakeShell();
  // As the core plans a paste (`enodix_core::paste`): a line per line,
  // asked first when there is more than one.
  shell.answer("paste_plan", (args) => {
    const lines = (args.text as string).split(/(?<=\n)/);
    return {
      lines,
      confirm: lines.length > 1,
      lineDelayMs: (args.safety as PasteSafety).lineDelayMs,
    };
  });
  clipboard = "";
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: {
      readText: async () => clipboard,
      writeText: async () => {},
    },
  });
});

afterEach(() => {
  if (component) unmount(component);
  component = null;
  preferences.current.rightClickPaste = false;
});

/** Exported by the component, as the window calls them. */
interface Terminal {
  sendText(text: string): boolean;
  sessionId(): number | null;
}

/**
 * A terminal tab opening a session: the shell's answer to `session_open`
 * waits for `answerOpen` (or `refuseOpen`), as a slow login does; the
 * session's channels are there before it, as in the app.
 */
async function openTab(
  props: {
    groupSize?: number | null;
    ongrouppaste?: (text: string) => void;
  } = {},
) {
  const open = pending<unknown>();
  let channels: SessionChannels | null = null;
  shell.answer("session_open", (args) => {
    channels = args as unknown as SessionChannels;
    return open.promise;
  });
  const target = document.body.appendChild(document.createElement("div"));
  component = mount(TerminalSession, {
    target,
    props: {
      source: {
        kind: "target",
        target: { kind: "host", id: "edge-1", credentials: null },
        name: "edge-1",
        title: "admin@edge-1",
      },
      tab: 1,
      groupSize: props.groupSize ?? null,
      ongrouppaste: props.ongrouppaste ?? (() => {}),
      title: "admin@edge-1",
      address: null,
      jumpHost: null,
      role: null,
      safety: SAFE,
      name: "edge-1",
      shown: true,
      focused: true,
      ondismiss: () => {},
    },
  });
  flushSync();
  await settle();
  if (!channels) throw new Error("the tab did not open its session");
  const events = sessionEvents(channels);
  const textarea = target.querySelector("textarea")!;
  return {
    events,
    terminal: component as unknown as Terminal,
    textarea,
    async answerOpen() {
      open.resolve(opened(ID));
      await settle();
    },
    async refuseOpen(reason: string) {
      open.reject(reason);
      await settle();
    },
    /** The session opened and logged in. */
    async connect() {
      open.resolve(opened(ID));
      await settle();
      events.connected();
      await settle();
    },
    /** What the device sends, once the terminal has parsed it. */
    async output(text: string) {
      events.output(text);
      await settle();
    },
    press(init: KeyboardEventInit) {
      textarea.dispatchEvent(
        new KeyboardEvent("keydown", {
          bubbles: true,
          cancelable: true,
          ...init,
        }),
      );
    },
  };
}

/** Each input the session was sent, in order: by `session_write` (before
 * it connected) or `session_input` (with its origin). */
function sent() {
  return shell.calls
    .filter(
      (call) =>
        call.command === "session_write" || call.command === "session_input",
    )
    .map((call) => ({
      id: call.args.id,
      text: sentText(call),
      origin: call.command === "session_write" ? "write" : call.args.origin,
    }));
}

const pasted = () => sent().filter((each) => each.origin === "pasted");
const question = () => document.querySelector("[data-paste-confirm]");

async function answerQuestion(button: "send" | "cancel") {
  document
    .querySelector<HTMLButtonElement>(`[data-paste="${button}"]`)!
    .click();
  await settle();
}

describe("input before the session's open is answered", () => {
  test("the terminal's reply to early output is sent once it is", async () => {
    const tab = await openTab();
    // A local shell's ConPTY asks for the cursor position first, and the
    // output reaches the page before `session_open` answers (0.8.0's
    // local shell that stayed empty).
    await tab.output("\x1b[6n");
    expect(sent()).toEqual([]);

    await tab.answerOpen();
    expect(sent()).toEqual([{ id: ID, text: "\x1b[1;1R", origin: "write" }]);
  });

  test("early output is kept and acknowledged once the session is known", async () => {
    const tab = await openTab();
    await tab.output("Microsoft Windows\r\n");
    await tab.output("\x1b[6n");
    expect(shell.called("session_acknowledge")).toEqual([]);

    await tab.answerOpen();
    const acknowledged = shell
      .called("session_acknowledge")
      .map((call) => call.args);
    expect(acknowledged).toEqual([{ id: ID, consumer: 1, bytes: 23 }]);
    // The reply shows the output was parsed in order: the cursor stood
    // on the second line.
    expect(sent()).toEqual([{ id: ID, text: "\x1b[2;1R", origin: "write" }]);
  });

  test("several inputs before it go in the order they were made", async () => {
    const tab = await openTab();
    await tab.output("\x1b[6n");
    tab.press({ key: "Enter", code: "Enter", keyCode: 13 });
    await tab.output("\x1b[c");
    await tab.answerOpen();
    expect(sent().map((each) => each.text)).toEqual([
      "\x1b[1;1R",
      "\r",
      "\x1b[?1;2c",
    ]);
  });

  test("an open refused sends none of it", async () => {
    const tab = await openTab();
    await tab.output("\x1b[6n");
    await tab.refuseOpen("connection refused");
    expect(sent()).toEqual([]);
  });

  test("once connected, input goes with its origin", async () => {
    const tab = await openTab();
    await tab.connect();
    await tab.output("\x1b[6n");
    tab.press({ key: "Enter", code: "Enter", keyCode: 13 });
    expect(sent()).toEqual([
      { id: ID, text: "\x1b[1;1R", origin: "reply" },
      { id: ID, text: "\r", origin: "typed" },
    ]);
  });
});

/** Every way the user starts a paste of the clipboard into the terminal. */
const CLIPBOARD_PASTES: {
  name: string;
  start: (tab: Awaited<ReturnType<typeof openTab>>) => void;
}[] = [
  {
    name: "Ctrl+V",
    start: (tab) => tab.press({ key: "v", code: "KeyV", ctrlKey: true }),
  },
  {
    name: "Ctrl+Shift+V",
    start: (tab) =>
      tab.press({ key: "V", code: "KeyV", ctrlKey: true, shiftKey: true }),
  },
  {
    name: "Ctrl+V on Dvorak (the key where QWERTY has the period)",
    start: (tab) => tab.press({ key: "v", code: "Period", ctrlKey: true }),
  },
  {
    name: "Ctrl+V on a Russian layout (м)",
    start: (tab) => tab.press({ key: "м", code: "KeyV", ctrlKey: true }),
  },
  {
    name: "Shift+Insert",
    start: (tab) =>
      tab.press({ key: "Insert", code: "Insert", shiftKey: true }),
  },
  {
    name: "a right click, when it pastes",
    start: (tab) => {
      preferences.current.rightClickPaste = true;
      tab.textarea.dispatchEvent(
        new MouseEvent("contextmenu", {
          bubbles: true,
          cancelable: true,
          button: 2,
        }),
      );
    },
  },
  {
    name: "the browser's own paste (its context menu)",
    start: (tab) => {
      const event = new Event("paste", { bubbles: true, cancelable: true });
      Object.defineProperty(event, "clipboardData", {
        value: {
          getData: (type: string) => (type === "text/plain" ? clipboard : ""),
        },
      });
      tab.textarea.dispatchEvent(event);
    },
  },
];

const TWO_LINES = "show version\nshow clock\n";

describe("every paste goes through the paste path", () => {
  for (const { name, start } of CLIPBOARD_PASTES) {
    test(`${name}: asked first, nothing sent before the answer`, async () => {
      const tab = await openTab();
      await tab.connect();
      clipboard = TWO_LINES;
      start(tab);
      await settle();
      expect(shell.called("paste_plan").map((call) => call.args)).toEqual([
        { text: TWO_LINES, safety: SAFE },
      ]);
      expect(question()).not.toBeNull();
      expect(pasted()).toEqual([]);

      await answerQuestion("send");
      expect(question()).toBeNull();
      expect(pasted().map((each) => each.text)).toEqual([
        "show version\r",
        "show clock\r",
      ]);
      // One paste, one id: the core takes all of it or none.
      const ids = shell.called("session_input").map((call) => call.args.paste);
      expect(new Set(ids).size).toBe(1);
      expect(ids[0]).not.toBeNull();
    });
  }

  test("a snippet sent to the tab (by click, palette or key) is asked first too", async () => {
    // The Snippets view, the palette and a snippet's key all end in the
    // tab's `sendText` (App's `sendSnippet`).
    const tab = await openTab();
    await tab.connect();
    expect(tab.terminal.sendText("conf t\nend\n")).toBe(true);
    await settle();
    expect(shell.called("paste_plan")).toHaveLength(1);
    expect(question()).not.toBeNull();
    expect(pasted()).toEqual([]);
    await answerQuestion("send");
    expect(pasted().map((each) => each.text)).toEqual(["conf t\r", "end\r"]);
  });

  test("answered Cancel, nothing is sent", async () => {
    const tab = await openTab();
    await tab.connect();
    clipboard = TWO_LINES;
    tab.press({ key: "v", code: "KeyV", ctrlKey: true });
    await settle();
    await answerQuestion("cancel");
    expect(question()).toBeNull();
    expect(pasted()).toEqual([]);
  });

  test("a session that closes while asked gets nothing", async () => {
    const tab = await openTab();
    await tab.connect();
    clipboard = TWO_LINES;
    tab.press({ key: "v", code: "KeyV", ctrlKey: true });
    await settle();
    tab.events.closed();
    await settle();
    expect(question()).toBeNull();
    expect(pasted()).toEqual([]);
  });

  test("one line the core does not ask about goes straight out", async () => {
    const tab = await openTab();
    await tab.connect();
    clipboard = "show version";
    tab.press({ key: "v", code: "KeyV", ctrlKey: true });
    await settle();
    expect(question()).toBeNull();
    expect(pasted().map((each) => each.text)).toEqual(["show version"]);
  });

  test("a paste the core cannot plan is not sent", async () => {
    const tab = await openTab();
    await tab.connect();
    shell.answer("paste_plan", () => {
      throw "the preferences cannot be read";
    });
    clipboard = TWO_LINES;
    tab.press({ key: "v", code: "KeyV", ctrlKey: true });
    await settle();
    expect(question()).toBeNull();
    expect(pasted()).toEqual([]);
  });

  test("Dvorak's Ctrl+K, where QWERTY has V, does not paste", async () => {
    const tab = await openTab();
    await tab.connect();
    clipboard = TWO_LINES;
    tab.press({ key: "k", code: "KeyV", ctrlKey: true, keyCode: 75 });
    await settle();
    expect(shell.called("paste_plan")).toEqual([]);
    // It reached the device as the control character it is.
    expect(sent()).toEqual([{ id: ID, text: "\x0b", origin: "typed" }]);
  });

  test("in the multi-send group a paste is the group's, not sent here", async () => {
    const groupPastes: string[] = [];
    const tab = await openTab({
      groupSize: 2,
      ongrouppaste: (text) => groupPastes.push(text),
    });
    await tab.connect();
    clipboard = TWO_LINES;
    tab.press({ key: "v", code: "KeyV", ctrlKey: true });
    await settle();
    expect(groupPastes).toEqual([TWO_LINES]);
    expect(shell.called("paste_plan")).toEqual([]);
    expect(pasted()).toEqual([]);
  });

  test("a snippet begun outside the group stays the tab's", async () => {
    const groupPastes: string[] = [];
    const tab = await openTab({
      groupSize: 2,
      ongrouppaste: (text) => groupPastes.push(text),
    });
    await tab.connect();
    tab.terminal.sendText("show run\n");
    await settle();
    expect(groupPastes).toEqual([]);
    expect(pasted().map((each) => each.text)).toEqual(["show run\r"]);
  });
});
