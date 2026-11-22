// Who may give a terminal the keyboard a moment later (#141).
//
// The window hands a terminal the keys some time after the user asked: a
// tab clicked is shown once the core's layout answer comes back, a session
// opens after its login, a snippet's terminal gets them back after its
// lines were filled in. On a busy computer that moment can come after the
// user has already put the keyboard somewhere else (clicked another pane
// and typed), and the keys then went back to the terminal of the earlier
// request: what was typed for one device went to another.
//
// The rule: a focus scheduled for later is given only if the keyboard has
// not moved since it was asked for. "Moved" is any focus landing on another
// element than the one that had it (a click, the Tab key, a script's
// `focus()`) and any press of the mouse; focus given through `giveKeys`
// does not count, nor does a window getting back the focus it had. Asking
// (`askForKeys`) is done at the instant of the user's action; work that
// awaits something first takes a `keyMark()` then and asks with it after,
// which asks nothing if the keyboard moved meanwhile.

/** How many times the keyboard moved (not counting `giveKeys`). */
let moves = 0;
/** `moves` when the keys were last asked for. */
let asked = 0;
/** The element the keyboard was last seen on. */
let last: EventTarget | null = null;
/** Set while `giveKeys` focuses: that move is the app's own. */
let giving = false;

if (typeof window !== "undefined") {
  window.addEventListener(
    "focusin",
    (event) => {
      if (giving) return;
      // A window getting the focus back refocuses the element it had.
      if (event.target !== last) moves += 1;
      last = event.target;
    },
    true,
  );
  // A press is the user doing something new, even on the element that has
  // the keys already (which gets no focus event).
  window.addEventListener("pointerdown", () => (moves += 1), true);
}

/** Where the keyboard stands now, for asking later (`askForKeys(mark)`). */
export function keyMark(): number {
  return moves;
}

/**
 * Asks that the window's active terminal take the keys (given by
 * `giveKeys` when it is shown or opens). With `mark`, only if the keyboard
 * has not moved since that mark was taken; returns whether it asked.
 */
export function askForKeys(mark: number = moves): boolean {
  if (mark !== moves) return false;
  asked = moves;
  return true;
}

/** Whether the keys were asked for and have not moved since. */
export function keysAsked(): boolean {
  return asked === moves;
}

/** Whether the focus now landing is `giveKeys`'s (the layout shown already
 * says where it goes), not a move of the keyboard. */
export function givingKeys(): boolean {
  return giving;
}

/**
 * Focuses `target` if the keys were asked for and have not moved since;
 * returns whether it did. For every focus a terminal takes by itself later
 * (not for one given at the instant of a click or key on that terminal).
 */
export function giveKeys(target: { focus(): void } | null | undefined) {
  if (!target || !keysAsked()) return false;
  giving = true;
  try {
    target.focus();
  } finally {
    giving = false;
  }
  last = document.activeElement;
  return true;
}
