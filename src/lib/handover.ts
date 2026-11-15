// Moving a terminal's contents to another window (#84). The serialize
// addon writes the screen, the scrollback, the alternate screen and most
// modes (application cursor keys and keypad, bracketed paste, insert,
// origin, focus reporting, wraparound, the mouse tracking mode); these
// add what it leaves out and the receiving terminal needs to behave the
// same: the mouse report encoding and a hidden cursor. Still lost: the
// scroll region, character sets, the cursor's style and saved positions.
import type { Terminal } from "@xterm/xterm";

/** The parts of xterm.js's internals read here (absent after an upgrade:
 * then nothing is added). */
interface Internals {
  _core?: {
    coreMouseService?: { activeEncoding?: string };
    coreService?: { isCursorHidden?: boolean };
  };
}

/** Escape sequences restoring what `SerializeAddon.serialize` leaves out. */
export function unserializedModes(terminal: Terminal): string {
  const core = (terminal as unknown as Internals)._core;
  let modes = "";
  const encoding = core?.coreMouseService?.activeEncoding;
  if (encoding === "SGR") modes += "\x1b[?1006h";
  else if (encoding === "SGR_PIXELS") modes += "\x1b[?1016h";
  if (core?.coreService?.isCursorHidden) modes += "\x1b[?25l";
  return modes;
}
