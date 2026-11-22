// The window's tabs and panes, kept by the core (`netterm_core::layout`).
// Each edit returns the whole layout; the interface renders it and keeps
// what each tab holds (its session) itself, by tab id.

import { invoke } from "@tauri-apps/api/core";

export type TabId = number;
export type PaneId = number;
/** Where a split puts the new pane: on the right, or below. */
export type SplitDirection = "right" | "down";

export interface PaneNode {
  kind: "pane";
  id: PaneId;
  tabs: TabId[];
  /** The tab shown; null shows the connect view. */
  active: TabId | null;
}

export interface SplitNode {
  kind: "split";
  direction: SplitDirection;
  children: LayoutNode[];
}

export type LayoutNode = PaneNode | SplitNode;

/**
 * A view shown in a tab (#68): which view it is and what it is about (a
 * host id, a run id), if anything. A view is open in one tab at most.
 */
export interface ViewRef {
  view: ViewName;
  subject: string | null;
}

/** The views that open as tabs. Later views add their name here. */
export type ViewName =
  | "preferences"
  | "job"
  | "run"
  | "configs"
  | "csv-import"
  | "nornir-import"
  | "platforms"
  | "highlights"
  | "enrich"
  | "python-setup"
  | "files";

/** What a tab holds: a session's terminal, or a view. */
export type TabKind = { kind: "session" } | ({ kind: "view" } & ViewRef);

export interface Layout {
  root: LayoutNode;
  activePane: PaneId;
  /**
   * This window's tabs in the app's multi-send group, in the order they
   * joined: what is typed in one of them reaches every member, in any
   * window (the shell routes it); a paste, as a group paste.
   */
  group: TabId[];
  /** How many members the group has, in every window (#87). */
  groupSize: number;
  /** Raised by every change to the group's members (#123): a group
   * paste's question asked before is out of date. */
  groupRevision: number;
  /** What each tab holds, by tab id. */
  kinds: Record<string, TabKind>;
  /** Raised by every change: an older layout is out of date. */
  revision: number;
}

export type LayoutEdit =
  | { kind: "openTab" }
  /** Shows the view's tab if it is open, else opens one. */
  | { kind: "openView"; view: ViewName; subject: string | null }
  /** The view about `from` is about `to` now (its host moved, #99). */
  | { kind: "retargetView"; view: ViewName; from: string; to: string }
  | { kind: "openTabBeside"; tab: TabId }
  | { kind: "activate"; tab: TabId }
  | { kind: "activatePane"; pane: PaneId }
  | { kind: "showConnectView"; pane: PaneId }
  | { kind: "nextTab" }
  | { kind: "previousTab" }
  | { kind: "closeTab"; tab: TabId }
  | { kind: "dismissTab"; tab: TabId }
  | { kind: "moveTabBy"; tab: TabId; offset: number }
  /** `index` counts the target pane's other tabs. */
  | { kind: "moveTab"; tab: TabId; pane: PaneId; index: number }
  | { kind: "split"; pane: PaneId; direction: SplitDirection }
  | { kind: "closePane"; pane: PaneId }
  /** `confirmed`: the user answered the join question; a production
   * session joins only then (the shell refuses it otherwise, #123). */
  | { kind: "joinGroup"; tab: TabId; confirmed: boolean }
  /** Every tab of the pane not yet in the group joins it. */
  | { kind: "joinPane"; pane: PaneId; confirmed: boolean }
  | { kind: "leaveGroup"; tab: TabId }
  | { kind: "endGroup" };

export interface LayoutChanged {
  layout: Layout;
  /** The tab (openTab*) or pane (split) the edit created. */
  created: number | null;
}

/** A fresh layout: one empty pane. */
export function resetLayout(): Promise<Layout> {
  return invoke<Layout>("layout_reset");
}

/**
 * For the end-to-end test only (#141); nothing in the app sets these.
 * While the page holds a promise in `__smokeHoldLayoutAnswers`, the core's
 * answers wait for it, as on a computer too busy to answer at once; once
 * the page has set `__smokeLayoutInFlight` (to 0), it counts the edits sent
 * and not yet answered.
 */
const smoke = globalThis as {
  __smokeHoldLayoutAnswers?: Promise<void>;
  __smokeLayoutInFlight?: number;
};

export async function editLayout(edit: LayoutEdit): Promise<LayoutChanged> {
  const counted = smoke.__smokeLayoutInFlight !== undefined;
  if (counted)
    smoke.__smokeLayoutInFlight = (smoke.__smokeLayoutInFlight ?? 0) + 1;
  try {
    const changed = await invoke<LayoutChanged>("layout_edit", { edit });
    if (smoke.__smokeHoldLayoutAnswers) await smoke.__smokeHoldLayoutAnswers;
    return changed;
  } finally {
    if (counted)
      smoke.__smokeLayoutInFlight = (smoke.__smokeLayoutInFlight ?? 1) - 1;
  }
}

/** The view `tab` shows, or null for a session tab (or an unknown one). */
export function viewOf(layout: Layout, tab: TabId): ViewRef | null {
  const kind = layout.kinds[String(tab)];
  return kind?.kind === "view"
    ? { view: kind.view, subject: kind.subject }
    : null;
}

/** Every pane, in reading order. */
export function panesOf(node: LayoutNode): PaneNode[] {
  return node.kind === "pane" ? [node] : node.children.flatMap(panesOf);
}
