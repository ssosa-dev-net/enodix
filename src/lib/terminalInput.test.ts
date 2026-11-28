// Where a terminal's input came from (`terminalInput.ts`), with the
// xterm.js the app ships, its events played as the browser sends them. The
// origin decides where input goes: typed keys may fan out to the
// multi-send group, a terminal's replies and mouse-driven input never do.

import { Terminal } from "@xterm/xterm";
import { afterEach, describe, expect, test } from "vitest";
import { terminalInput, type Input } from "./terminalInput";

let terminal: Terminal;

afterEach(() => terminal?.dispose());

/** A terminal on the page, and every input it reports. */
function openTerminal() {
  terminal = new Terminal();
  const element = document.createElement("div");
  document.body.append(element);
  terminal.open(element);
  const inputs: Input[] = [];
  const input = terminalInput(terminal, (each) => inputs.push(each));
  return { input, inputs };
}

/** What the device sends, once xterm.js has parsed it. */
function fromDevice(text: string) {
  return new Promise<void>((resolve) => terminal.write(text, resolve));
}

function press(init: KeyboardEventInit) {
  terminal.textarea!.dispatchEvent(
    new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init }),
  );
}

const origins = (inputs: Input[]) =>
  inputs.map(({ data, origin }) => ({ data, origin }));

describe("terminalInput", () => {
  test("keys pressed are typed", () => {
    const { inputs } = openTerminal();
    press({ key: "Enter", code: "Enter", keyCode: 13 });
    press({ key: "ArrowUp", code: "ArrowUp", keyCode: 38 });
    press({ key: "c", code: "KeyC", keyCode: 67, ctrlKey: true });
    expect(origins(inputs)).toEqual([
      { data: "\r", origin: "typed" },
      { data: "\x1b[A", origin: "typed" },
      { data: "\x03", origin: "typed" },
    ]);
  });

  test("the terminal's answers to the device are replies", async () => {
    const { inputs } = openTerminal();
    // A cursor-position report and the device attributes, as a shell (or
    // ConPTY) asks for them.
    await fromDevice("\x1b[6n\x1b[c");
    expect(origins(inputs)).toEqual([
      { data: "\x1b[1;1R", origin: "reply" },
      { data: "\x1b[?1;2c", origin: "reply" },
    ]);
  });

  test("a focus report is a reply, though the user moved the focus", async () => {
    const { inputs } = openTerminal();
    // Turning focus reports on reports where the focus is now (out).
    await fromDevice("\x1b[?1004h");
    terminal.textarea!.dispatchEvent(new FocusEvent("focus"));
    expect(origins(inputs)).toEqual([
      { data: "\x1b[O", origin: "reply" },
      { data: "\x1b[I", origin: "reply" },
    ]);
  });

  test("a key typed right after a reply is typed, and the reply stays one", async () => {
    const { inputs } = openTerminal();
    await fromDevice("\x1b[6n");
    press({ key: "Enter", code: "Enter", keyCode: 13 });
    await fromDevice("\x1b[6n");
    expect(inputs.map((each) => each.origin)).toEqual([
      "reply",
      "typed",
      "reply",
    ]);
  });

  test("the wheel in a full-screen program is mouse input", async () => {
    const { inputs } = openTerminal();
    // The alternate screen without mouse reporting: xterm.js turns the
    // wheel into arrow keys, which depend on this terminal's screen.
    await fromDevice("\x1b[?1049h");
    terminal.element!.dispatchEvent(
      new WheelEvent("wheel", {
        deltaY: 1,
        deltaMode: WheelEvent.DOM_DELTA_LINE,
        bubbles: true,
        cancelable: true,
      }),
    );
    expect(inputs.length).toBeGreaterThan(0);
    for (const each of inputs) expect(each.origin).toBe("mouse");
  });

  test("our own pastes are pasted, with the terminal's line endings", () => {
    const { input, inputs } = openTerminal();
    input.paste("show version\nshow clock");
    expect(origins(inputs)).toEqual([
      { data: "show version\rshow clock", origin: "pasted" },
    ]);
  });

  test("a paste in bracketed paste mode is pasted, brackets and all", async () => {
    const { input, inputs } = openTerminal();
    await fromDevice("\x1b[?2004h");
    input.paste("conf t");
    expect(origins(inputs)).toEqual([
      { data: "\x1b[200~conf t\x1b[201~", origin: "pasted" },
    ]);
  });

  test("input is recorded only once asked, the latest kept", async () => {
    const { input } = openTerminal();
    press({ key: "Enter", code: "Enter", keyCode: 13 });
    expect(input.recent()).toEqual([]);
    input.record();
    for (let count = 0; count < 70; count += 1)
      press({ key: "ArrowUp", code: "ArrowUp", keyCode: 38 });
    await fromDevice("\x1b[6n");
    const recent = input.recent();
    expect(recent).toHaveLength(64);
    expect(recent.at(-1)).toEqual({
      data: "\x1b[1;1R",
      binary: false,
      origin: "reply",
    });
  });

  test("nothing is reported once disposed", async () => {
    const { input, inputs } = openTerminal();
    input.dispose();
    press({ key: "Enter", code: "Enter", keyCode: 13 });
    await fromDevice("\x1b[6n");
    expect(inputs).toEqual([]);
  });

  test("without xterm.js's user-input hook it refuses to start", () => {
    terminal = new Terminal();
    terminal.open(document.body.appendChild(document.createElement("div")));
    // As if an xterm.js update renamed what this relies on.
    const core = (terminal as unknown as { _core: { coreService: object } })
      ._core;
    Object.defineProperty(core.coreService, "onUserInput", {
      value: undefined,
    });
    const inputs: Input[] = [];
    expect(() => terminalInput(terminal, (each) => inputs.push(each))).toThrow(
      /typed input cannot be told from replies/,
    );
    // And no key it saw was taken for a reply.
    press({ key: "Enter", code: "Enter", keyCode: 13 });
    expect(inputs).toEqual([]);
  });
});
