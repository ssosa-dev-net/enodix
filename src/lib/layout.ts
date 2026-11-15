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
  | "highlights";

/** What a tab holds: a session's terminal, or a view. */
export type TabKind = { kind: "session" } | ({ kind: "view" } & ViewRef);

export interface Layout {
  root: LayoutNode;
  activePane: PaneId;
  /**
   * The multi-send group's tabs, in the order they joined: what is typed
   * or pasted in one of them reaches them all (the shell routes it).
   */
  group: TabId[];
  /** What each tab holds, by tab id. */
  kinds: Record<string, TabKind>;
  /** Raised by every change: an older layout is out of date. */
  revision: number;
}

export type LayoutEdit =
  | { kind: "openTab" }
  /** Shows the view's tab if it is open, else opens one. */
  | { kind: "openView"; view: ViewName; subject: string | null }
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
  | { kind: "joinGroup"; tab: TabId }
  /** Every tab of the pane not yet in the group joins it. */
  | { kind: "joinPane"; pane: PaneId }
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

export function editLayout(edit: LayoutEdit): Promise<LayoutChanged> {
  return invoke<LayoutChanged>("layout_edit", { edit });
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
