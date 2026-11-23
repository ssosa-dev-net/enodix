// Views that open as tabs beside the terminals (#68). The core's layout
// keeps which tab shows which view (`enodix_core::layout::View`); this
// module names the views and lets any part of the interface open one.
// `App.svelte` renders each view tab's content over its pane's body, as it
// does terminals.
//
// Adding a view: its name in `ViewName` (`layout.ts`), an entry in `VIEWS`
// (title, icon), and a branch in `App.svelte`'s view slot.

import Archive from "@lucide/svelte/icons/archive";
import ClipboardCheck from "@lucide/svelte/icons/clipboard-check";
import Cpu from "@lucide/svelte/icons/cpu";
import FileClock from "@lucide/svelte/icons/file-clock";
import FileInput from "@lucide/svelte/icons/file-input";
import FileSpreadsheet from "@lucide/svelte/icons/file-spreadsheet";
import FolderOpen from "@lucide/svelte/icons/folder-open";
import Highlighter from "@lucide/svelte/icons/highlighter";
import ListChecks from "@lucide/svelte/icons/list-checks";
import Package from "@lucide/svelte/icons/package";
import Settings from "@lucide/svelte/icons/settings";
import SquarePen from "@lucide/svelte/icons/square-pen";
import type { Component } from "svelte";
import { following, unsaved } from "./hostDrafts.svelte";
import { inventory, selectedHost } from "./inventory.svelte";
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
  /** The CSV import: file, mapping, preview, report (#71; one tab). */
  "csv-import": { title: () => "Import CSV", icon: FileSpreadsheet },
  /** The Nornir import: files, preview, report (#71; one tab). */
  "nornir-import": { title: () => "Import Nornir", icon: FileInput },
  /** The open workspace's platform definitions (#72; one tab). */
  platforms: { title: () => "Platforms", icon: Cpu },
  /** The highlight rules and the switch (#72; one tab). */
  highlights: { title: () => "Highlight rules", icon: Highlighter },
  /** The review of a "Fill in from devices" run (#73; subject: the run). */
  enrich: { title: () => "Fill in from devices", icon: ClipboardCheck },
  /** Setting up the app's Python environment (#73; one tab). */
  "python-setup": { title: () => "Python setup", icon: Package },
  /** A host's files over SFTP (#103; subject: the host; one per host,
   * main window only). */
  files: { title: (host) => `Files ${hostName(host)}`, icon: FolderOpen },
  /** The host editor (#10): subject null follows the selected host;
   * a host's id keeps the tab on that host (pinned). */
  host: {
    title: (host) => {
      const id = host ?? following.host ?? selectedHost()?.id ?? null;
      if (!id) return "Edit host";
      // Unsaved edits show in the tab, as an editor's would.
      const count = unsaved(id);
      return `Edit ${hostName(id)}${count > 0 ? ` (${count})` : ""}`;
    },
    icon: SquarePen,
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

/** Shows the host editor tab that follows the selection (#10). */
export function openHostEditor() {
  openView("host", null);
}

/** Opens a new job tab for `draft`'s job. */
export function openJobTab(draft: Omit<JobDraft, "run" | "opened" | "shown">) {
  openView("job", addDraft(draft));
}

/** Shows the run `run`: the job tab that started it if it is open, else
 * the run's own tab. */
export function openRunTab(run: string) {
  const draft = draftOfRun(run);
  if (draft) openView("job", draft);
  else openView("run", run);
}
