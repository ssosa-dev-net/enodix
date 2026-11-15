// The open session windows (#85), which every window offers as places a
// session can move to.

import type { SessionMove } from "./SessionMenu.svelte";
import {
  listSessionWindows,
  onSessionWindows,
  thisSessionWindow,
  type ListedWindow,
} from "./windows";

/** The open session windows, now and as they change. */
export const sessionWindows = $state<{ list: ListedWindow[] }>({ list: [] });

/** Keeps `sessionWindows` up to date, for as long as the page lives. */
export function trackSessionWindows() {
  void onSessionWindows((list) => (sessionWindows.list = list));
  listSessionWindows().then(
    (list) => (sessionWindows.list = list),
    () => {
      // Told again with the next change.
    },
  );
}

/**
 * The moves a window offers for a session: into a new window, and into
 * each other open session window, named by the session it shows. `open`
 * makes the move (`into` null: a new window).
 */
export function windowMoves(
  open: (into: number | null) => void,
  newShortcut?: string,
): SessionMove[] {
  const own = thisSessionWindow();
  return [
    {
      id: "open-in-window",
      label: "Open in new window",
      title: "Move this session to a window of its own",
      shortcut: newShortcut,
      run: () => open(null),
    },
    ...sessionWindows.list
      .filter((each) => each.window !== own)
      .map((each) => ({
        id: `open-in-window-${each.window}`,
        label: each.name
          ? `Open in the window of ${each.name}`
          : `Open in session window ${each.window}`,
        title: "Move this session to a tab of that window",
        attributes: { "data-into-window": String(each.window) },
        run: () => open(each.window),
      })),
  ];
}
