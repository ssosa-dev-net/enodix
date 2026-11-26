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

/**
 * The chord a key press is, in the written form the core keeps, or null for
 * a press that is no chord the core accepts (a modifier alone, plain typing,
 * the Windows key). Letters and digits go by position, so a chord works on
 * any keyboard layout.
 */
export function chordOf(event: KeyboardEvent): string | null {
  if (event.metaKey) return null;
  let key: string | null = null;
  if (/^Key[A-Z]$/.test(event.code)) key = event.code.slice(3);
  else if (/^Digit[0-9]$/.test(event.code)) key = event.code.slice(5);
  else if (event.code in CODE_KEYS) key = CODE_KEYS[event.code];
  else if (/^F([1-9]|1[0-9]|2[0-4])$/.test(event.key)) key = event.key;
  else if (event.key === " ") key = "Space";
  else if (NAMED_KEYS.includes(event.key)) key = event.key;
  if (key === null) return null;
  const functionKey = /^F\d+$/.test(key);
  if (!event.ctrlKey && !event.altKey && !functionKey) return null;
  return [
    event.ctrlKey ? "Ctrl" : "",
    event.altKey ? "Alt" : "",
    event.shiftKey ? "Shift" : "",
    key,
  ]
    .filter(Boolean)
    .join("+");
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
