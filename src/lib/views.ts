// Views that open as tabs beside the terminals (#68). The core's layout
// keeps which tab shows which view (`netterm_core::layout::View`); this
// module names the views and lets any part of the interface open one.
// `App.svelte` renders each view tab's content over its pane's body, as it
// does terminals.
//
// Adding a view: its name in `ViewName` (`layout.ts`), an entry in `VIEWS`
// (title, icon), and a branch in `App.svelte`'s view slot.

import Archive from "@lucide/svelte/icons/archive";
import FileClock from "@lucide/svelte/icons/file-clock";
import ListChecks from "@lucide/svelte/icons/list-checks";
import Settings from "@lucide/svelte/icons/settings";
import type { Component } from "svelte";
import { inventory } from "./inventory.svelte";
import type { ViewName, ViewRef } from "./layout";
import { findHost } from "./workspace";
import {
  addDraft,
  draftOfRun,
  draftTitle,
  runTitle,
  type JobDraft,
} from "./jobTabs.svelte";

export interface ViewInfo {
  /** The tab's name. */
  title: (subject: string | null) => string;
  /** The tab's icon (in place of a session's connection dot). */
  icon: Component<{ size?: number; "aria-hidden"?: boolean | "true" }>;
}

export const VIEWS: Record<ViewName, ViewInfo> = {
  preferences: { title: () => "Preferences", icon: Settings },
  /** A job from its start: choice, pre-flight, running, results (#69). */
  job: { title: draftTitle, icon: ListChecks },
  /** A kept run: output, search, compare, save, delete, run again (#69). */
  run: { title: runTitle, icon: FileClock },
  /** A host's config history: versions, text, diff, save (#70). */
  configs: {
    title: (host) => `Config ${hostName(host)}`,
    icon: Archive,
  },
};

/** The open workspace's name for the host `id` (else the id). */
function hostName(id: string | null): string {
  if (!id) return "";
  const tree = inventory.workspace?.tree;
  return (tree && findHost(tree, id)?.name) ?? id;
}

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

/** Opens a new job tab for `draft`'s job. */
export function openJobTab(draft: Omit<JobDraft, "run" | "opened">) {
  openView("job", addDraft(draft));
}

/** Shows the run `run`: the job tab that started it if it is open, else
 * the run's own tab. */
export function openRunTab(run: string) {
  const draft = draftOfRun(run);
  if (draft) openView("job", draft);
  else openView("run", run);
}
