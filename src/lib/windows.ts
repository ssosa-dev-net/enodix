// Real windows (#77, #78): a run or a host's config history can leave the
// main window for a window of its own (on any monitor) and dock back as a
// tab. The shell opens and closes them; a detached window's label says
// what it shows (`main.ts`): `run-<run id>`, or `config-<host id in hex>`
// (a label takes no `.`). A session can too (#84): its window is
// `session-<n>`, n being the core's id for the window.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
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

/** This session window's id in the core (`session-<n>`), or null. */
export function thisSessionWindow(): number | null {
  return isSessionWindow()
    ? Number(getCurrentWindow().label.slice(SESSION_WINDOW_PREFIX.length))
    : null;
}

/**
 * Moves this window's tab `tab`, whose session `id` this window has given
 * up, to another window, which shows `snapshot` first: into the open
 * session window `into` (#85), or, when null, into a new session window,
 * whose inner size (CSS pixels) `size` keeps the terminal's columns and
 * rows. Resolves to this window's layout; null once this session window
 * closed, its last tab gone.
 */
export function openSessionWindow(
  tab: TabId,
  id: number,
  snapshot: string,
  size: { width: number; height: number },
  into: number | null = null,
) {
  return invoke<Layout | null>("window_open_session", {
    tab,
    id,
    snapshot,
    ...size,
    into,
  });
}

/** A session a session window takes. */
export interface TakenSession {
  session: number;
  tab: TabId;
  /** Null: the page was reloaded, and what it showed is lost. */
  snapshot: string | null;
  record: SessionRecord | null;
}

/** This session window's sessions, once one moved here, and its layout. */
export function takeSessions() {
  return invoke<{ sessions: TakenSession[]; layout: Layout }>(
    "session_window_take",
  );
}

/** The session of the tab `tab` moved into this open session window
 * (#85), and the window's layout. */
export function takeTab(tab: TabId) {
  return invoke<{ session: TakenSession; layout: Layout }>(
    "session_window_take_tab",
    { tab },
  );
}

/** Calls `handler` with each tab moved into this open session window. */
export function onTabArrived(
  handler: (tab: TabId) => void,
): Promise<UnlistenFn> {
  // This window's own: a plain `listen` hears every window's.
  return getCurrentWebviewWindow().listen<{ tab: TabId }>(
    "session-tab-arrived",
    (event) => handler(event.payload.tab),
  );
}

/** A session tab given back to the main window. */
export interface Docking {
  tab: TabId;
  /** Its session, given up. */
  id: number;
  snapshot: string;
}

/** Docks this session window's tab back into the main window (its
 * session given up, `snapshot` its terminal). Resolves to this window's
 * layout; null once it closed, that being its last tab. */
export function dockSession(tab: TabId, id: number, snapshot: string) {
  return invoke<Layout | null>("session_dock", { tab, id, snapshot });
}

/** Docks every tab given back into the main window, in order (#85). */
export function dockAll(docking: Docking[]) {
  return invoke<Layout | null>("session_dock_all", { docking });
}

/** Closes this session window and its sessions (its last tab was
 * closed). */
export function closeSessionWindow() {
  return invoke<void>("session_window_close");
}

/** An open session window, named by the session its active tab shows. */
export interface ListedWindow {
  window: number;
  /** Empty while that is not known. */
  name: string;
}

/** The open session windows (#85). */
export function listSessionWindows() {
  return invoke<ListedWindow[]>("session_windows_list");
}

/** Calls `handler` with the open session windows whenever they change. */
export function onSessionWindows(
  handler: (list: ListedWindow[]) => void,
): Promise<UnlistenFn> {
  return listen<ListedWindow[]>("session-windows", (event) =>
    handler(event.payload),
  );
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
