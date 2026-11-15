// Real windows (#77, #78): a run or a host's config history can leave the
// main window for a window of its own (on any monitor) and dock back as a
// tab. The shell opens and closes them; a detached window's label says
// what it shows (`main.ts`): `run-<run id>`, or `config-<host id in hex>`
// (a label takes no `.`). A session can too (#84): its window is
// `session-<n>`, n being the core's id for the window.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Layout, TabId } from "./layout";
import type { SessionRecord } from "./session";

const RUN_WINDOW_PREFIX = "run-";
const CONFIG_WINDOW_PREFIX = "config-";
const SESSION_WINDOW_PREFIX = "session-";

/** Whether this window is a session window (#84). */
export function isSessionWindow(): boolean {
  const label = getCurrentWindow().label;
  return (
    label.startsWith(SESSION_WINDOW_PREFIX) &&
    /^\d+$/.test(label.slice(SESSION_WINDOW_PREFIX.length))
  );
}

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

/**
 * Moves the main window's tab `tab`, whose session `id` this window has
 * given up, to a session window of its own, which shows `snapshot` first;
 * `size` is its inner size (CSS pixels) at which the terminal keeps its
 * columns and rows. Resolves to the main window's layout.
 */
export function openSessionWindow(
  tab: TabId,
  id: number,
  snapshot: string,
  size: { width: number; height: number },
) {
  return invoke<Layout>("window_open_session", {
    tab,
    id,
    snapshot,
    ...size,
  });
}

/** What a session window starts from. */
export interface TakenSession {
  session: number;
  tab: TabId;
  /** Null: the page was reloaded, and what it showed is lost. */
  snapshot: string | null;
  record: SessionRecord | null;
  layout: Layout;
}

/** This session window's session, once moved here. */
export function takeSession() {
  return invoke<TakenSession>("session_window_take");
}

/** Docks this session window's tab back into the main window (its
 * session given up, `snapshot` its terminal); this window closes. */
export function dockSession(tab: TabId, id: number, snapshot: string) {
  return invoke<void>("session_dock", { tab, id, snapshot });
}

/** Closes this session window and its session (its tab was closed). */
export function closeSessionWindow() {
  return invoke<void>("session_window_close");
}

/** A session the main window is given (docked back, or from a session
 * window that died: `snapshot` null). */
export interface ArrivedSession {
  session: number;
  tab: TabId;
  snapshot: string | null;
  record: SessionRecord | null;
  layout: Layout;
}

/** Calls `handler` with each session the main window is given. */
export function onSessionArrived(
  handler: (arrived: ArrivedSession) => void,
): Promise<UnlistenFn> {
  return listen<ArrivedSession>("session-arrived", (event) =>
    handler(event.payload),
  );
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
