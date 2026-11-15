// Views that open as tabs beside the terminals (#68). The core's layout
// keeps which tab shows which view (`netterm_core::layout::View`); this
// module names the views and lets any part of the interface open one.
// `App.svelte` renders each view tab's content over its pane's body, as it
// does terminals.
//
// Adding a view: its name in `ViewName` (`layout.ts`), an entry in `VIEWS`
// (title, icon), and a branch in `App.svelte`'s view slot.

import Settings from "@lucide/svelte/icons/settings";
import type { Component } from "svelte";
import type { ViewName, ViewRef } from "./layout";

export interface ViewInfo {
  /** The tab's name. */
  title: (subject: string | null) => string;
  /** The tab's icon (in place of a session's connection dot). */
  icon: Component<{ size?: number; "aria-hidden"?: boolean | "true" }>;
}

export const VIEWS: Record<ViewName, ViewInfo> = {
  preferences: { title: () => "Preferences", icon: Settings },
};

let opener: ((view: ViewRef) => void) | null = null;

/** Called once by the window, which owns the layout. */
export function setViewOpener(open: (view: ViewRef) => void) {
  opener = open;
}

/** Shows `view` in a tab: its own if it is open, else a new one in the
 * active pane. */
export function openView(view: ViewName, subject: string | null = null) {
  opener?.({ view, subject });
}
