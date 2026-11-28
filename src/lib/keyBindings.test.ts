// Which letter a shortcut press is (`typedLetter`), and the chords a press
// can be bound by (`chordsOf`, `bindingOf`), on the layouts users have. A
// press carries the character it prints on the user's layout (`key`) and
// where the key sits, named as on a US QWERTY keyboard (`code`).

import { describe, expect, test } from "vitest";
import { bindingOf, chordOf, chordsOf, typedLetter } from "./keyBindings";

/** A press of the key at `code` printing `key`, with modifiers. */
function press(
  key: string,
  code: string,
  modifiers: { ctrl?: boolean; shift?: boolean; alt?: boolean } = {},
) {
  return new KeyboardEvent("keydown", {
    key,
    code,
    ctrlKey: modifiers.ctrl ?? false,
    shiftKey: modifiers.shift ?? false,
    altKey: modifiers.alt ?? false,
  });
}

// The keys for copy, paste and search on three layouts: the character
// printed and the position pressed.
const LAYOUTS = {
  qwerty: { c: "KeyC", v: "KeyV", f: "KeyF", k: "KeyK" },
  // French AZERTY: A and Q, Z and W swapped; M where QWERTY has ;.
  azerty: { c: "KeyC", v: "KeyV", f: "KeyF", k: "KeyK" },
  // Dvorak: every letter elsewhere.
  dvorak: { c: "KeyI", v: "Period", f: "KeyY", k: "KeyV" },
} as const;

describe("typedLetter", () => {
  for (const [layout, keys] of Object.entries(LAYOUTS)) {
    test(`on ${layout}, each letter is the key that prints it`, () => {
      for (const [letter, code] of Object.entries(keys)) {
        expect(typedLetter(press(letter, code, { ctrl: true }), letter)).toBe(
          true,
        );
        // Shifted, the key prints the capital.
        expect(
          typedLetter(
            press(letter.toUpperCase(), code, { ctrl: true, shift: true }),
            letter,
          ),
        ).toBe(true);
      }
    });
  }

  test("on AZERTY, the key where QWERTY has W is Z, not W", () => {
    expect(typedLetter(press("z", "KeyW"), "w")).toBe(false);
    expect(typedLetter(press("z", "KeyW"), "z")).toBe(true);
    expect(typedLetter(press("a", "KeyQ"), "q")).toBe(false);
    expect(typedLetter(press("a", "KeyQ"), "a")).toBe(true);
  });

  test("on Dvorak, the key where QWERTY has V is K: Ctrl+it does not paste", () => {
    expect(typedLetter(press("k", "KeyV", { ctrl: true }), "v")).toBe(false);
    // And Ctrl+the key printing V does, wherever it is.
    expect(typedLetter(press("v", "Period", { ctrl: true }), "v")).toBe(true);
    // The key where QWERTY has C prints J.
    expect(typedLetter(press("j", "KeyC", { ctrl: true }), "c")).toBe(false);
  });

  test("a layout printing no Latin letter goes by the key's position", () => {
    // Russian: the key where QWERTY has V prints м, C prints с.
    expect(typedLetter(press("м", "KeyV", { ctrl: true }), "v")).toBe(true);
    expect(typedLetter(press("с", "KeyC", { ctrl: true }), "c")).toBe(true);
    expect(typedLetter(press("а", "KeyF", { ctrl: true }), "v")).toBe(false);
  });

  test("a press printing a symbol goes by the key's position too", () => {
    // AZERTY prints a comma where QWERTY has M, and é on the 2 key.
    expect(typedLetter(press(",", "KeyM", { ctrl: true }), "m")).toBe(true);
    expect(typedLetter(press(",", "KeyM", { ctrl: true }), "v")).toBe(false);
    expect(typedLetter(press("é", "Digit2", { ctrl: true }), "v")).toBe(false);
  });
});

describe("chordsOf", () => {
  test("a press is the printed key first, then its QWERTY position", () => {
    expect(chordsOf(press("k", "KeyK", { ctrl: true }))).toEqual(["Ctrl+K"]);
    // Dvorak's K sits where QWERTY has V.
    expect(chordsOf(press("k", "KeyV", { ctrl: true }))).toEqual([
      "Ctrl+K",
      "Ctrl+V",
    ]);
    // AZERTY's W sits where QWERTY has Z.
    expect(chordsOf(press("W", "KeyZ", { ctrl: true, shift: true }))).toEqual([
      "Ctrl+Shift+W",
      "Ctrl+Shift+Z",
    ]);
    expect(chordOf(press("k", "KeyV", { ctrl: true }))).toBe("Ctrl+K");
  });

  test("plain typing and the Windows key are no chord; F keys are", () => {
    expect(chordsOf(press("k", "KeyK"))).toEqual([]);
    expect(chordsOf(press("K", "KeyK", { shift: true }))).toEqual([]);
    expect(
      chordsOf(
        new KeyboardEvent("keydown", { key: "k", code: "KeyK", metaKey: true }),
      ),
    ).toEqual([]);
    expect(chordsOf(press("F5", "F5"))).toEqual(["F5"]);
    expect(chordsOf(press("F5", "F5", { shift: true }))).toEqual(["Shift+F5"]);
  });
});

describe("bindingOf", () => {
  const reserved = ["Ctrl+V", "Ctrl+C", "Ctrl+Shift+V", "Ctrl+Shift+C"];

  test("a key bound to a snippet sends it, on each layout", () => {
    const snippetKeys = { "Ctrl+Alt+B": "backup" };
    for (const code of ["KeyB", "KeyX", "KeyN"]) {
      // B prints from KeyB on QWERTY and AZERTY, KeyN on Dvorak; KeyX
      // stands for any layout putting it elsewhere.
      expect(
        bindingOf(
          press("b", code, { ctrl: true, alt: true }),
          {},
          snippetKeys,
          reserved,
        ),
      ).toEqual({ action: null, snippet: "backup" });
    }
  });

  test("the printed key wins over the position", () => {
    // Dvorak: Ctrl+K (palette) on the key where QWERTY has V (paste).
    expect(
      bindingOf(press("k", "KeyV", { ctrl: true }), {}, {}, reserved),
    ).toEqual({ action: "quick-open", snippet: null });
  });

  test("the position is not tried past a key the app keeps", () => {
    // Dvorak's V sits where QWERTY has the period: Ctrl+V is paste (kept),
    // and Ctrl+. bound to a snippet must not run as well.
    expect(
      bindingOf(
        press("v", "Period", { ctrl: true }),
        {},
        { "Ctrl+.": "show-run" },
        reserved,
      ),
    ).toEqual({ action: null, snippet: null });
  });

  test("a shortcut the user moved answers on its new key only", () => {
    const moved = { "quick-open": "Ctrl+P" };
    expect(
      bindingOf(press("p", "KeyP", { ctrl: true }), moved, {}, reserved),
    ).toEqual({ action: "quick-open", snippet: null });
    expect(
      bindingOf(press("k", "KeyK", { ctrl: true }), moved, {}, reserved),
    ).toEqual({ action: null, snippet: null });
  });
});
