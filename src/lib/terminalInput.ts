import type { IDisposable, Terminal } from "@xterm/xterm";

/**
 * Where a piece of terminal input came from. Only "typed" and "pasted" are
 * the user's own input (what multi-send may share); "reply" is the
 * terminal answering the device by itself (cursor-position and status
 * reports, device attributes, focus reports, ...), "mouse" is a mouse
 * report (or another mouse-driven sequence: wheel arrows in a full-screen
 * program, alt-click cursor moves), which depends on this terminal's screen.
 */
export type InputOrigin = "typed" | "pasted" | "reply" | "mouse";

/** One piece of input, as xterm.js produced it. */
export interface Input {
  data: string;
  /** Sent byte per character (some mouse reports), not as UTF-8. */
  binary: boolean;
  origin: InputOrigin;
}

export interface TerminalInput extends IDisposable {
  /** Pastes through xterm.js (bracketed paste applies); counts as "pasted". */
  paste(text: string): void;
  /**
   * For the smoke test: from `record()` on, the last inputs are kept
   * (oldest first, from `recent()`). Off by default, as typed input
   * includes passwords answered in the terminal.
   */
  record(): void;
  recent(): Input[];
}

const RECENT = 64;

/** The part of xterm.js's internals this relies on. */
interface Internals {
  _core?: {
    coreService?: { onUserInput?: (listener: () => void) => IDisposable };
  };
}

/**
 * Reports every piece of input `terminal` produces, with its origin.
 *
 * xterm.js sends keystrokes and its own replies through the same `onData`
 * event. Internally each is triggered with a `wasUserInput` flag, and its
 * core service fires `onUserInput` synchronously just before `onData` for
 * user input only (keys, IME composition, pastes, mouse reports): that is
 * what separates replies. Mouse reports are user input to xterm.js, so they
 * are recognised by the DOM event being dispatched (`window.event`), as
 * xterm.js sends them from its mouse and wheel handlers. If xterm.js's
 * internals change and that hook is gone, this throws: typed keys could
 * not be told from replies (they would never reach the multi-send group,
 * nor stop a snippet's steps). The interface's tests
 * (`terminalInput.test.ts`) run against the xterm.js installed, and the
 * smoke test checks the origins in the app.
 */
export function terminalInput(
  terminal: Terminal,
  onInput: (input: Input) => void,
): TerminalInput {
  const recent: Input[] = [];
  let recording = false;
  const report = (input: Input) => {
    if (recording) {
      recent.push(input);
      if (recent.length > RECENT) recent.shift();
    }
    onInput(input);
  };

  let userInput = false;
  let pasting = false;
  const coreService = (terminal as unknown as Internals)._core?.coreService;
  const watch = coreService?.onUserInput?.call(coreService, () => {
    userInput = true;
  });
  if (!watch) {
    throw new Error(
      "xterm.js internals changed: typed input cannot be told from replies",
    );
  }

  const origin = (): InputOrigin => {
    if (pasting) return "pasted";
    if (!userInput) return "reply";
    return window.event instanceof MouseEvent ? "mouse" : "typed";
  };
  const dataListener = terminal.onData((text) => {
    const from = origin();
    userInput = false;
    report({ data: text, binary: false, origin: from });
  });
  // Only mouse reports (X10 encoding) are sent as binary.
  const binaryListener = terminal.onBinary((text) =>
    report({ data: text, binary: true, origin: "mouse" }),
  );

  return {
    paste(text) {
      pasting = true;
      try {
        terminal.paste(text);
      } finally {
        pasting = false;
      }
    },
    record() {
      recording = true;
    },
    recent: () => [...recent],
    dispose() {
      watch?.dispose();
      dataListener.dispose();
      binaryListener.dispose();
    },
  };
}
