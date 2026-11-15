// Real windows (#77, #78): a run or a host's config history can leave the
// main window for a window of its own (on any monitor) and dock back as a
// tab. The shell opens and closes them; a detached window's label says
// what it shows (`main.ts`): `run-<run id>`, or `config-<host id in hex>`
// (a label takes no `.`).

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

const RUN_WINDOW_PREFIX = "run-";
const CONFIG_WINDOW_PREFIX = "config-";

/** What a detached window shows. */
export type DetachedView =
  { kind: "run"; run: string } | { kind: "configs"; host: string };

/** The host id a config history window's label names, or null. */
function hostOfLabel(hex: string): string | null {
  if (hex.length === 0 || hex.length % 2 !== 0 || /[^0-9a-f]/.test(hex))
    return null;
  const bytes = new Uint8Array(hex.length / 2);
  for (let at = 0; at < bytes.length; at++)
    bytes[at] = parseInt(hex.slice(at * 2, at * 2 + 2), 16);
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return null;
  }
}

/** What this window shows on its own, or null in the main window. */
export function detachedView(): DetachedView | null {
  const label = getCurrentWindow().label;
  if (label.startsWith(RUN_WINDOW_PREFIX))
    return { kind: "run", run: label.slice(RUN_WINDOW_PREFIX.length) };
  if (label.startsWith(CONFIG_WINDOW_PREFIX)) {
    const host = hostOfLabel(label.slice(CONFIG_WINDOW_PREFIX.length));
    if (host !== null) return { kind: "configs", host };
  }
  return null;
}

/** Shows the run `run` in a window of its own (its window if open). */
export function detachRun(run: string) {
  return invoke<void>("window_detach_run", { run });
}

/** Shows the host `host`'s config history in a window of its own (its
 * window if open). */
export function detachConfigs(host: string) {
  return invoke<void>("window_detach_configs", { host });
}

/** Docks this detached window back: what it shows opens as a tab in the
 * main window, and this window closes. */
export function dockBack() {
  return invoke<void>("window_dock");
}

/** From a detached config history: the main window opens a backup of its
 * host and comes forward. */
export function backUpInMainWindow() {
  return invoke<void>("window_back_up");
}

/** Closes every detached window (their jobs go on). */
export function closeDetachedWindows() {
  return invoke<void>("windows_close_detached");
}

/** Calls `handler` with each run docked back into the main window. */
export function onDockRun(handler: (run: string) => void): Promise<UnlistenFn> {
  return listen<string>("dock-run", (event) => handler(event.payload));
}

/** Calls `handler` with each host whose config history docked back. */
export function onDockConfigs(
  handler: (host: string) => void,
): Promise<UnlistenFn> {
  return listen<string>("dock-configs", (event) => handler(event.payload));
}

/** Calls `handler` with each host a detached config history asks to back
 * up. */
export function onBackUpHost(
  handler: (host: string) => void,
): Promise<UnlistenFn> {
  return listen<string>("back-up-host", (event) => handler(event.payload));
}
