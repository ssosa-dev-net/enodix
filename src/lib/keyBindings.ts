// Key bindings (#24): the window's shortcuts the user can move, and keys
// that send a snippet. The core keeps and checks them
// (`enodix_core::preferences`); this turns a key press into the same written
// chord (`Ctrl+Shift+K`) so the two can be compared.

export interface ShortcutAction {
  id: string;
  name: string;
  /** The key it has unless the user moved it. */
  chord: string;
}

/** In the core's order (`SHORTCUT_ACTIONS`): on a clash the first keeps
 * the key. */
export const SHORTCUT_ACTIONS: ShortcutAction[] = [
  {
    id: "quick-open",
    name: "Search hosts, snippets and commands",
    chord: "Ctrl+K",
  },
  { id: "switch-tab", name: "Switch workspace", chord: "Ctrl+Shift+O" },
  { id: "close-tab", name: "Close tab", chord: "Ctrl+Shift+W" },
  {
    id: "toggle-group",
    name: "Join or leave the multi-send group",
    chord: "Ctrl+Shift+M",
  },
  {
    id: "open-in-window",
    name: "Open tab in its own window",
    chord: "Ctrl+Shift+N",
  },
  {
    id: "toggle-side-view",
    name: "Fold or show the side view",
    chord: "Ctrl+Shift+B",
  },
  { id: "preferences", name: "Preferences", chord: "Ctrl+," },
  { id: "edit-host", name: "Edit the selected host", chord: "Ctrl+Shift+E" },
  { id: "details", name: "Details of the selection", chord: "Ctrl+Shift+D" },
];

const CODE_KEYS: Record<string, string> = {
  Comma: ",",
  Period: ".",
  Slash: "/",
  Semicolon: ";",
  Quote: "'",
  BracketLeft: "[",
  BracketRight: "]",
  Backslash: "\\",
  Minus: "-",
  Equal: "=",
  Backquote: "`",
};

const NAMED_KEYS = [
  "Tab",
  "PageUp",
  "PageDown",
  "Home",
  "End",
  "Insert",
  "Delete",
];

const PRINTED_PUNCTUATION = ",./;'[]\\-=`+";

/** The key a press prints on the user's layout (`W`, `7`, `,`), or null when
 * it prints none we name (a Cyrillic letter, a shifted symbol, a dead key). */
function printedKey(event: KeyboardEvent): string | null {
  const key = event.key;
  if (key.length !== 1) return null;
  if (/^[a-zA-Z0-9]$/.test(key)) return key.toUpperCase();
  return PRINTED_PUNCTUATION.includes(key) ? key : null;
}

/** The key where the press sits, as on a QWERTY keyboard. */
function positionKey(event: KeyboardEvent): string | null {
  if (/^Key[A-Z]$/.test(event.code)) return event.code.slice(3);
  if (/^Digit[0-9]$/.test(event.code)) return event.code.slice(5);
  return CODE_KEYS[event.code] ?? null;
}

/** Whether a press is the letter `name` for what the terminal does with it
 * (copy, paste, search): the key that prints it on the user's layout, and
 * only when the press prints no Latin letter, the key at that position. The
 * app's own shortcuts go by the same rule, so a press is one or the other. */
export function typedLetter(event: KeyboardEvent, name: string): boolean {
  const upper = name.toUpperCase();
  if (/^[a-zA-Z]$/.test(event.key)) return event.key.toUpperCase() === upper;
  return event.code === `Key${upper}`;
}

/**
 * The chords a key press can be, in the written form the core keeps, best
 * first; empty for a press that is no chord the core accepts (a modifier
 * alone, plain typing, the Windows key). The first is the key that prints the
 * letter on the user's layout (AZERTY's W, Dvorak's K); the second, when it
 * differs, is the key at that position on a QWERTY keyboard, which is how
 * the shortcuts also answered before they could be moved.
 */
export function chordsOf(event: KeyboardEvent): string[] {
  if (event.metaKey) return [];
  const keys: string[] = [];
  const functionKey = /^F([1-9]|1[0-9]|2[0-4])$/.test(event.key);
  if (functionKey) keys.push(event.key);
  else if (event.key === " ") keys.push("Space");
  else if (NAMED_KEYS.includes(event.key)) keys.push(event.key);
  else {
    for (const key of [printedKey(event), positionKey(event)]) {
      if (key !== null && !keys.includes(key)) keys.push(key);
    }
  }
  if (!event.ctrlKey && !event.altKey && !functionKey) return [];
  return keys.map((key) =>
    [
      event.ctrlKey ? "Ctrl" : "",
      event.altKey ? "Alt" : "",
      event.shiftKey ? "Shift" : "",
      key,
    ]
      .filter(Boolean)
      .join("+"),
  );
}

/** The chord to record for a press: the one it will later be matched by
 * first, the printed key. */
export function chordOf(event: KeyboardEvent): string | null {
  return chordsOf(event)[0] ?? null;
}

/** What a press is bound to: an action or a snippet, or neither. The printed
 * key wins; the key's position is tried only when the printed chord is bound
 * to nothing and is not one the app keeps (`reserved`), so one press never
 * runs two things, and copy and paste stay where the user sees them. */
export function bindingOf(
  event: KeyboardEvent,
  moved: Record<string, string>,
  snippetKeys: Record<string, string>,
  reserved: string[],
): { action: string | null; snippet: string | null } {
  for (const chord of chordsOf(event)) {
    const action = actionFor(chord, moved);
    const snippet = snippetKeys[chord] ?? null;
    if (action !== null || snippet !== null) return { action, snippet };
    if (reserved.includes(chord)) break;
  }
  return { action: null, snippet: null };
}

/** The action a chord runs now: the first in `SHORTCUT_ACTIONS` order whose
 * key (moved or default) it is. */
export function actionFor(
  chord: string,
  moved: Record<string, string>,
): string | null {
  for (const action of SHORTCUT_ACTIONS) {
    if ((moved[action.id] ?? action.chord) === chord) return action.id;
  }
  return null;
}

/** The key an action has now. */
export function chordFor(id: string, moved: Record<string, string>): string {
  return (
    moved[id] ??
    SHORTCUT_ACTIONS.find((action) => action.id === id)?.chord ??
    ""
  );
}
