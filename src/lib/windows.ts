// Real windows (#77): a run can leave the main window for a window of its
// own (on any monitor) and dock back as a tab. The shell opens and closes
// them; a detached window's label is `run-<run id>`, which is how its page
// knows what to show (`main.ts`).

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

const RUN_WINDOW_PREFIX = "run-";

/** The run this window shows on its own, or null in the main window. */
export function detachedRun(): string | null {
  const label = getCurrentWindow().label;
  return label.startsWith(RUN_WINDOW_PREFIX)
    ? label.slice(RUN_WINDOW_PREFIX.length)
    : null;
}

/** Shows the run `run` in a window of its own (its window if open). */
export function detachRun(run: string) {
  return invoke<void>("window_detach_run", { run });
}

/** Docks this detached window back: its run opens as a tab in the main
 * window, and this window closes. */
export function dockBack() {
  return invoke<void>("window_dock");
}

/** Closes every detached window (their jobs go on). */
export function closeDetachedWindows() {
  return invoke<void>("windows_close_detached");
}

/** Calls `handler` with each run docked back into the main window. */
export function onDockRun(handler: (run: string) => void): Promise<UnlistenFn> {
  return listen<string>("dock-run", (event) => handler(event.payload));
}
