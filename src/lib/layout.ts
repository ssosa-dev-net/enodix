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

export interface Layout {
  root: LayoutNode;
  activePane: PaneId;
  /** Raised by every change: an older layout is out of date. */
  revision: number;
}

export type LayoutEdit =
  | { kind: "openTab" }
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
  | { kind: "closePane"; pane: PaneId };

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

/** Every pane, in reading order. */
export function panesOf(node: LayoutNode): PaneNode[] {
  return node.kind === "pane" ? [node] : node.children.flatMap(panesOf);
}
