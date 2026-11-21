// The open workspace as the interface shows it: the tree, the selected
// item, which folders are expanded, and the dialog in front, plus the user's
// edits. Every edit is made by the core, which writes the files and returns
// the reloaded workspace.

import {
  planSavedJob,
  type JobPlan,
  type JobStep,
  type JobTarget,
  type SavedJob,
} from "./jobs";
import { kindOf } from "./jobTabs.svelte";
import { openJobTab, openView } from "./views";
import * as api from "./workspace";
import type {
  Edited,
  EffectiveSettings,
  FilterView,
  Folder,
  Host,
  Origin,
  PasteSafety,
  Role,
  Settings,
  WorkspaceView,
} from "./workspace";

/** A tree item: a host (by id) or a folder (by path; "" is the root). */
export type Item =
  { kind: "host"; id: string } | { kind: "folder"; path: string };

export interface DialogField {
  name: string;
  label: string;
  value: string;
  /** A choice instead of free text. */
  options?: { value: string; label: string }[];
}

/** A small form in front of the window, for names, moves and confirmations. */
export interface DialogRequest {
  title: string;
  message?: string;
  fields: DialogField[];
  confirm: string;
  danger?: boolean;
  /** Resolves to an error to show, or null to close the dialog. */
  onconfirm: (values: Record<string, string>) => Promise<string | null>;
}

export const inventory = $state({
  workspace: null as WorkspaceView | null,
  /** Why the workspace could not be opened, if it could not. */
  openError: null as string | null,
  /** Why the last edit failed, if it did. */
  editError: null as string | null,
  selected: null as Item | null,
  /**
   * Whether the details sheet is open over the side view (#64): it shows
   * the selected host or folder, following the selection.
   */
  details: false,
  expanded: {} as Record<string, boolean>,
  dialog: null as DialogRequest | null,
  /** While dragging: the folder the item would be dropped into. */
  dropTarget: null as string | null,
  /** Whether the Ctrl+K palette is open. */
  palette: false,
  /** The saved filter the tree is viewed through, by name. */
  activeFilter: null as string | null,
  /** The words typed in the Hosts view's filter field ("" for none). */
  query: "",
  /** What the active filter and the typed words show; null shows
   * everything. */
  filterView: null as { hosts: Set<string>; folders: Set<string> } | null,
  /**
   * The folder the CSV import tab was last opened on (its target while no
   * preview is shown); null until it is opened in this workspace.
   */
  csvImport: null as string | null,
  /** The same for the Nornir import tab. */
  nornirImport: null as string | null,
  /**
   * What the last rename or move could not bring along (#99): history
   * left where it was and references not rewritten; null when it brought
   * everything.
   */
  moveReport: null as MoveReport | null,
});

/** What a rename or move of `item` (its new path) could not bring along. */
export interface MoveReport {
  item: string;
  left: api.LeftHistory[];
  notRewritten: api.Problem[];
}

type HostsMovedListener = (moves: api.HostMove[], root: string) => void;
const hostsMovedListeners = new Set<HostsMovedListener>();

/**
 * Calls `listener` after every rename or move that changed host ids, with
 * the moves and the workspace's root (what holds a host id follows it:
 * session tabs, config history tabs). Returns the unsubscribe function.
 */
export function onHostsMoved(listener: HostsMovedListener): () => void {
  hostsMovedListeners.add(listener);
  return () => hostsMovedListeners.delete(listener);
}

/**
 * Opens a job tab running a snippet on `target`, called `label`; at the
 * pre-flight of `planned` if given (whatever its step).
 */
export function openJob(
  target: JobTarget,
  label: string,
  planned: JobPlan | null = null,
) {
  openJobTab({
    kind: planned ? kindOf(planned) : "snippet",
    target,
    label,
    planned,
  });
}

/**
 * Opens a job tab filling in vendor, model, serial number and OS version
 * of `target`'s hosts from their devices.
 */
export function openEnrich(target: JobTarget, label: string) {
  openJobTab({ kind: "enrich", target, label, planned: null });
}

/** Opens a job tab running a Python script for `target`'s hosts. */
export function openScript(target: JobTarget, label: string) {
  openJobTab({ kind: "script", target, label, planned: null });
}

/** Opens a job tab sending a local file to `target`'s hosts. */
export function openSendFile(target: JobTarget, label: string) {
  openJobTab({ kind: "file", target, label, planned: null });
}

/** Opens a job tab backing up the configs of `target`'s hosts. */
export function openBackup(target: JobTarget, label: string) {
  openJobTab({ kind: "backup", target, label, planned: null });
}

/** What a job tab calls `target` (a folder, a saved filter, a host). */
export function jobTargetLabel(target: JobTarget): string {
  const workspace = inventory.workspace;
  if (target.kind === "folder") {
    if (target.path === "") return workspace?.tree.name ?? "the workspace";
    return target.path.split("/").pop() ?? target.path;
  }
  if (target.kind === "filter") return `the hosts of ${target.name}`;
  if (target.ids.length === 1) {
    const host = workspace ? api.findHost(workspace.tree, target.ids[0]) : null;
    return host?.name ?? target.ids[0];
  }
  return `${target.ids.length} hosts`;
}

/**
 * Saves `step` on `target` as the job `name` in the workspace (a new one;
 * see `renameJob`); rejects with why not.
 */
export async function saveJobAs(
  name: string,
  step: JobStep,
  target: JobTarget,
) {
  apply(await api.saveJob(null, { name, step, target }));
}

/** Asks for a new name for the saved job `job`. */
export function renameJob(job: SavedJob) {
  inventory.dialog = {
    title: "Rename saved job",
    fields: [{ name: "name", label: "Name", value: job.name }],
    confirm: "Rename",
    onconfirm: (values) =>
      inDialog(async () => {
        apply(
          await api.saveJob(job.name, {
            name: values.name,
            step: job.step,
            target: job.target,
          }),
        );
      }),
  };
}

/** Asks before deleting the saved job `name` from the workspace. */
export function removeJob(name: string) {
  inventory.dialog = {
    title: "Delete saved job",
    message: `Delete the saved job ${name} from the workspace? Its past runs stay.`,
    fields: [],
    confirm: "Delete",
    danger: true,
    onconfirm: () =>
      inDialog(async () => {
        apply(await api.deleteJob(name));
      }),
  };
}

/**
 * Plans the saved job `name` and opens a job tab at its pre-flight
 * (or the variables it still needs). Rejects, saying what is gone, if its
 * step or target no longer exists.
 */
export async function runSavedJob(name: string) {
  const { target, plan } = await planSavedJob(name);
  openJob(target, jobTargetLabel(target), plan);
}

/** Opens the config history of the host `id` in its tab (one per host). */
export function openConfigHistory(id: string) {
  openView("configs", id);
}

/** Opens the Files tab of the host `id` (or brings it forward, #103). */
export function openFiles(id: string) {
  openView("files", id);
}

/** Opens the CSV import tab (or brings it forward) with `folder` as its
 * target. */
export function openCsvImport(folder: string) {
  inventory.csvImport = folder;
  openView("csv-import");
}

/**
 * Imports the CSV file read last (see `api.importCsv`) and shows the
 * reloaded workspace; returns the report. Errors propagate.
 */
export async function runCsvImport(
  mapping: api.ColumnTarget[],
  target: string,
  replace: string[],
): Promise<api.ImportReport> {
  return apply(await api.importCsv(mapping, target, replace));
}

/**
 * Fills in what the preview of the "Fill in from devices" run `run`
 * showed (see `api.applyEnrich`) and shows the reloaded workspace; returns
 * the report. Errors propagate.
 */
export async function applyEnrich(
  run: string,
  replace: string[],
): Promise<api.EnrichReport> {
  return apply(await api.applyEnrich(run, replace));
}

/** Opens the Nornir import tab (or brings it forward) with `folder` as
 * its target. */
export function openNornirImport(folder: string) {
  inventory.nornirImport = folder;
  openView("nornir-import");
}

/**
 * Runs the Nornir import the last preview showed (see
 * `api.importNornir`) and shows the reloaded workspace; returns the report.
 * Errors propagate.
 */
export async function runNornirImport(
  replace: string[],
): Promise<api.ImportReport> {
  return apply(await api.importNornir(replace));
}

export function setWorkspace(workspace: WorkspaceView | null) {
  inventory.workspace = workspace;
  inventory.selected = null;
  inventory.expanded = {};
  inventory.editError = null;
  inventory.moveReport = null;
  inventory.activeFilter = null;
  inventory.query = "";
  inventory.filterView = null;
  inventory.csvImport = null;
  inventory.nornirImport = null;
}

// Under a filter, the selected item (one just created, say, or edited out
// of the filter) stays in view with its folders, so it never vanishes
// while the side panel shows it.

/** Whether the tree shows the host `id` under the active filter. */
export function hostShown(id: string): boolean {
  const view = inventory.filterView;
  if (!view || view.hosts.has(id)) return true;
  const selected = inventory.selected;
  return selected?.kind === "host" && selected.id === id;
}

/** Whether the tree shows the folder at `path` under the active filter. */
export function folderShown(path: string): boolean {
  const view = inventory.filterView;
  if (!view || view.folders.has(path)) return true;
  const selected = inventory.selected;
  if (!selected) return false;
  const at = selected.kind === "host" ? selected.id : selected.path;
  return at === path || at.startsWith(`${path}/`);
}

export function selectedHost(): Host | null {
  const { workspace, selected } = inventory;
  if (!workspace || selected?.kind !== "host") return null;
  return api.findHost(workspace.tree, selected.id);
}

export function selectedFolder(): Folder | null {
  const { workspace, selected } = inventory;
  if (!workspace || selected?.kind !== "folder") return null;
  return api.findFolder(workspace.tree, selected.path);
}

/** The role the host `id` shows, if it has one. */
export function hostRole(id: string): Role | null {
  return inventory.workspace?.host_roles[id] ?? null;
}

/** The environment and paste line delay of the host `id`; null if the
 * open workspace has no such host. */
export function hostSafety(id: string): PasteSafety | null {
  return inventory.workspace?.host_safety[id] ?? null;
}

/** The folder new items go into: the selected folder, or the selected host's. */
export function targetFolder(): string {
  const selected = inventory.selected;
  if (selected?.kind === "folder") return selected.path;
  if (selected?.kind === "host") return api.parentPath(selected.id);
  return "";
}

/** Opens the details sheet on `item` (selected, and shown in the tree). */
export function openDetails(item: Item) {
  if (item.kind === "host") revealHost(item.id);
  else {
    reveal(item.path);
    inventory.selected = item;
  }
  inventory.details = true;
}

/** Expands every folder above `path` (and `path` itself, if `self`). */
export function reveal(path: string, self = false) {
  const names = path.split("/");
  const upTo = self ? names.length : names.length - 1;
  for (let i = 1; i <= upTo; i++) {
    inventory.expanded[names.slice(0, i).join("/")] = true;
  }
}

/** Shows the host `id` in the tree: expands its folders and selects it. */
export function revealHost(id: string) {
  if (!hostShown(id)) {
    viewThrough(null);
    inventory.query = "";
  }
  reveal(id);
  inventory.selected = { kind: "host", id };
}

/** Views the tree through the saved filter `name`, or no saved filter
 * (null: everything, or what the typed words show). */
export function viewThrough(name: string | null) {
  inventory.activeFilter = name;
  if (name === null && !inventory.query.trim()) inventory.filterView = null;
}

/**
 * Keeps the view of the active filter and the typed words up to date as the
 * workspace changes (the core matches both: the words as a `text`
 * condition), and expands the folders they show when they change. Call
 * during component setup.
 */
export function trackFilterView() {
  let expandedFor: string | null = null;
  $effect(() => {
    const name = inventory.activeFilter;
    const words = inventory.query.trim();
    const filter = inventory.workspace?.filters.find((f) => f.name === name);
    if (name !== null && !filter) {
      // Deleted or renamed outside the app.
      inventory.activeFilter = null;
    }
    const conditions = [
      ...(filter ? $state.snapshot(filter.conditions) : []),
      ...(words ? [{ field: "text", value: words }] : []),
    ];
    if (conditions.length === 0) {
      inventory.filterView = null;
      expandedFor = null;
      return;
    }
    const shown = `${filter?.name ?? ""}|${words}`;
    let stale = false;
    api
      .applyFilter({ name: filter?.name ?? "", conditions })
      .then((view: FilterView) => {
        if (stale) return;
        inventory.filterView = {
          hosts: new Set(view.hosts),
          folders: new Set(view.folders),
        };
        if (expandedFor !== shown) {
          expandedFor = shown;
          for (const path of view.folders) inventory.expanded[path] = true;
        }
      })
      .catch((error) => {
        if (!stale) inventory.editError = String(error);
      });
    return () => {
      stale = true;
    };
  });
}

/** How a search or filter field is called on screen. */
export function fieldLabel(field: string): string {
  if (field.startsWith("custom.")) return field.slice("custom.".length);
  const known = FILTER_FIELDS.find(([key]) => key === field);
  return known ? known[1] : field;
}

/** The fields a filter can test, besides custom ones. */
const FILTER_FIELDS: [string, string][] = [
  ["tag", "Tag"],
  ["role", "Role"],
  ["name", "Name"],
  ["address", "Management address"],
  ["platform", "Platform"],
  ...api.HOST_TEXT_FIELDS.map(([key, label]): [string, string] => [key, label]),
  ["description", "Description"],
  ["text", "Any field (search words)"],
];

/**
 * Asks for a filter of one condition, saves it and views the tree through
 * it; with `words` (the filter field's), it starts as those words in any
 * field, and the field is cleared once saved.
 */
export function newFilter(words = "") {
  const tree = inventory.workspace?.tree;
  if (!tree) return;
  const custom = [
    ...new Set(
      api.allHosts(tree).flatMap((host) => host.custom.map((f) => f.key)),
    ),
  ].sort();
  const options = [
    ...FILTER_FIELDS,
    ...custom.map((key): [string, string] => [
      `custom.${key}`,
      `${key} (custom)`,
    ]),
  ].map(([value, label]) => ({ value, label }));
  inventory.dialog = {
    title: "Save filter",
    message: "Values match whole, ignoring case; * stands for any text.",
    fields: [
      { name: "filter-name", label: "Name", value: "" },
      {
        name: "field",
        label: "Field",
        value: words ? "text" : options[0].value,
        options,
      },
      { name: "value", label: "Value", value: words },
    ],
    confirm: "Save",
    onconfirm: (values) =>
      inDialog(async () => {
        const name = values["filter-name"].trim();
        apply(
          await api.saveFilter({
            name,
            conditions: [{ field: values.field, value: values.value }],
          }),
        );
        const saved = inventory.workspace?.filters.find(
          (filter) => filter.name.toLowerCase() === name.toLowerCase(),
        );
        if (saved) {
          if (words) inventory.query = "";
          viewThrough(saved.name);
        }
      }),
  };
}

/** Asks before deleting the saved filter `name` from the workspace. */
export function removeFilter(name: string) {
  inventory.dialog = {
    title: "Delete filter",
    message: `Delete the saved filter ${name}? Its hosts are not touched.`,
    fields: [],
    confirm: "Delete",
    danger: true,
    onconfirm: () =>
      inDialog(async () => {
        apply(await api.deleteFilter(name));
        if (inventory.activeFilter === name) viewThrough(null);
      }),
  };
}

/** Saves a highlight rule (in place of `previous`); rejects with why not. */
export async function saveHighlightRule(
  previous: string | null,
  rule: api.HighlightRule,
) {
  apply(await api.saveHighlightRule(previous, rule));
}

/** Deletes a workspace highlight rule, or resets a built-in one. */
export async function deleteHighlightRule(name: string) {
  apply(await api.deleteHighlightRule(name));
}

/** Saves a platform definition (in place of `previous`); rejects with why not. */
export async function savePlatform(
  previous: string | null,
  definition: api.PlatformDefinition,
) {
  apply(await api.savePlatform(previous, definition));
}

/** Deletes a workspace platform definition, or resets a built-in one. */
export async function deletePlatform(name: string) {
  apply(await api.deletePlatform(name));
}

/**
 * The definition a platform setting of `text` finds (by name or other
 * name, ignoring case; `usable` false when it has a bad pattern), as the
 * core listed it in `platform_names`; null if none.
 */
export function platformDefinition(
  text: string,
): { definition: string; usable: boolean } | null {
  const names = inventory.workspace?.platform_names ?? {};
  return names[text.trim().toLowerCase()] ?? null;
}

/** Saves a snippet (in place of `previous`); rejects with why not. */
export async function saveSnippet(
  previous: string | null,
  snippet: api.Snippet,
) {
  apply(await api.saveSnippet(previous, snippet));
}

/** Asks before deleting the snippet `name` from the workspace. */
export function removeSnippet(name: string) {
  inventory.dialog = {
    title: "Delete snippet",
    message: `Delete the snippet ${name} from the workspace?`,
    fields: [],
    confirm: "Delete",
    danger: true,
    onconfirm: () =>
      inDialog(async () => {
        apply(await api.deleteSnippet(name));
      }),
  };
}

function itemPath(item: Item) {
  return item.kind === "host" ? item.id : item.path;
}

function itemName(item: Item) {
  const path = itemPath(item);
  const host =
    item.kind === "host" && inventory.workspace
      ? api.findHost(inventory.workspace.tree, item.id)
      : null;
  return host?.name ?? path.slice(path.lastIndexOf("/") + 1);
}

/**
 * Shows `workspace` unless a newer view is already shown (a reload from
 * disk can overtake an edit's answer, and already includes the edit).
 * Returns whether it is shown.
 */
function adopt(workspace: WorkspaceView): boolean {
  const shown = inventory.workspace;
  if (shown?.root === workspace.root && workspace.revision < shown.revision) {
    return false;
  }
  inventory.workspace = workspace;
  return true;
}

/** Applies an edit's answer; returns its result. Errors propagate. */
function apply<T>(edited: Edited<T>): T {
  adopt(edited.workspace);
  inventory.editError = null;
  return edited.result;
}

/**
 * Shows the workspace as reloaded after files changed outside the app.
 * The selection and expanded folders stay; a selected item that no longer
 * exists is deselected. Open sessions are not touched.
 */
export function workspaceChanged(workspace: WorkspaceView) {
  // A late notice about a workspace no longer open, or an outdated one.
  if (inventory.workspace?.root !== workspace.root || !adopt(workspace)) {
    return;
  }
  const selected = inventory.selected;
  if (
    selected &&
    !(selected.kind === "host"
      ? api.findHost(workspace.tree, selected.id)
      : api.findFolder(workspace.tree, selected.path))
  ) {
    inventory.selected = null;
  }
}

/** Runs an edit outside a dialog, showing a failure in the panel. */
async function attempt(edit: () => Promise<void>) {
  try {
    await edit();
  } catch (error) {
    inventory.editError = String(error);
  }
}

/** Runs a dialog's edit: a failure is shown in the dialog. */
async function inDialog(edit: () => Promise<void>): Promise<string | null> {
  try {
    await edit();
    return null;
  } catch (error) {
    return String(error);
  }
}

/** Something at `from` now lives at `to`: keep the selection and expansion. */
function moved(from: string, to: string) {
  if (from === to) return;
  const rebase = (path: string) =>
    path === from
      ? to
      : path.startsWith(`${from}/`)
        ? to + path.slice(from.length)
        : null;
  const selected = inventory.selected;
  if (selected) {
    const path = rebase(itemPath(selected));
    if (path !== null) {
      inventory.selected =
        selected.kind === "host"
          ? { kind: "host", id: path }
          : { kind: "folder", path };
    }
  }
  const expanded: Record<string, boolean> = {};
  for (const [path, open] of Object.entries(inventory.expanded)) {
    expanded[rebase(path) ?? path] = open;
  }
  inventory.expanded = expanded;
  reveal(to);
}

export function newFolder(parent: string) {
  inventory.dialog = {
    title: "New folder",
    message: parent ? `In ${parent}` : "At the top of the workspace",
    fields: [{ name: "name", label: "Name", value: "" }],
    confirm: "Create",
    onconfirm: ({ name }) =>
      inDialog(async () => {
        const path = apply(await api.createFolder(parent, name));
        reveal(path);
        inventory.selected = { kind: "folder", path };
      }),
  };
}

export function newHost(folder: string) {
  inventory.dialog = {
    title: "New host",
    message: folder ? `In ${folder}` : "At the top of the workspace",
    fields: [
      { name: "name", label: "Name", value: "" },
      { name: "address", label: "Management address", value: "" },
    ],
    confirm: "Create",
    onconfirm: ({ name, address }) =>
      inDialog(async () => {
        const id = apply(await api.createHost(folder, name, address));
        reveal(id);
        inventory.selected = { kind: "host", id };
      }),
  };
}

/** Takes in a rename or move: the workspace, what follows the hosts that
 * moved, and the report of what did not. */
function relocated(from: string, relocation: api.Relocated<string>) {
  const to = apply(relocation);
  moved(from, to);
  const root = relocation.workspace.root;
  if (relocation.moved.length > 0) {
    for (const listener of hostsMovedListeners)
      listener(relocation.moved, root);
  }
  inventory.moveReport =
    relocation.left.length > 0 || relocation.notRewritten.length > 0
      ? {
          item: to,
          left: relocation.left,
          notRewritten: relocation.notRewritten,
        }
      : null;
}

async function renameItem(item: Item, name: string) {
  relocated(
    itemPath(item),
    item.kind === "host"
      ? await api.renameHost(item.id, name)
      : await api.renameFolder(item.path, name),
  );
}

async function moveItem(item: Item, folder: string) {
  relocated(
    itemPath(item),
    item.kind === "host"
      ? await api.moveHost(item.id, folder)
      : await api.moveFolder(item.path, folder),
  );
}

/** Renames without a dialog (from the side panel). */
export function renameTo(item: Item, name: string) {
  return attempt(() => renameItem(item, name));
}

export function rename(item: Item) {
  inventory.dialog = {
    title: item.kind === "host" ? "Rename host" : "Rename folder",
    fields: [{ name: "name", label: "Name", value: itemName(item) }],
    confirm: "Rename",
    onconfirm: ({ name }) => inDialog(() => renameItem(item, name)),
  };
}

/** Moves an item into a folder (drag and drop, or the move dialog). */
export function moveInto(item: Item, folder: string) {
  return attempt(() => moveItem(item, folder));
}

/** Whether `item` may be dropped into the folder at `folder`. */
export function canMoveInto(item: Item, folder: string) {
  const path = itemPath(item);
  if (api.parentPath(path) === folder) return false;
  return (
    item.kind === "host" ||
    (item.path !== "" && folder !== path && !folder.startsWith(`${path}/`))
  );
}

export function move(item: Item) {
  const tree = inventory.workspace?.tree;
  if (!tree) return;
  const options = api
    .allFolders(tree)
    .filter((folder) => canMoveInto(item, folder.path))
    .map((folder) => ({
      value: folder.path,
      label: folder.path === "" ? `${tree.name} (top)` : folder.path,
    }));
  if (options.length === 0) {
    inventory.editError = `There is no other folder to move ${itemName(item)} to.`;
    return;
  }
  inventory.dialog = {
    title: `Move ${itemName(item)}`,
    fields: [
      { name: "folder", label: "To folder", value: options[0].value, options },
    ],
    confirm: "Move",
    onconfirm: ({ folder }) => inDialog(() => moveItem(item, folder)),
  };
}

function afterDelete(path: string) {
  const selected = inventory.selected;
  if (
    selected &&
    (itemPath(selected) === path || itemPath(selected).startsWith(`${path}/`))
  ) {
    inventory.selected = null;
  }
}

export function remove(item: Item) {
  const name = itemName(item);
  if (item.kind === "host") {
    inventory.dialog = {
      title: "Delete host",
      message: `Delete ${name}? Its file ${item.id} is deleted.`,
      fields: [],
      confirm: "Delete",
      danger: true,
      onconfirm: () =>
        inDialog(async () => {
          apply(await api.deleteHost(item.id));
          afterDelete(item.id);
        }),
    };
    return;
  }
  // A folder without hosts goes at once; one with hosts asks first.
  attempt(async () => {
    const answer = await api.deleteFolder(item.path, false);
    if (answer.kind === "deleted") {
      apply({ workspace: answer.workspace, result: null });
      afterDelete(item.path);
      return;
    }
    const hosts = answer.hosts === 1 ? "1 host" : `${answer.hosts} hosts`;
    inventory.dialog = {
      title: "Delete folder",
      message: `${name} contains ${hosts}. Delete the folder and everything in it?`,
      fields: [],
      confirm: `Delete ${hosts}`,
      danger: true,
      onconfirm: () =>
        inDialog(async () => {
          const confirmed = await api.deleteFolder(item.path, true);
          if (confirmed.kind === "deleted") {
            apply({ workspace: confirmed.workspace, result: null });
            afterDelete(item.path);
          }
        }),
    };
  });
}

// Host saves run one after another, each on the host as the last one left
// it, so a quick second edit never sends a copy that undoes the first.
let saving: Promise<void> = Promise.resolve();

/**
 * Saves some fields of the host `id`, given as values or as a function of
 * the host as last saved; resolves once saved (or failed).
 */
export function saveHost(
  id: string,
  changes: Partial<Host> | ((current: Host) => Partial<Host>),
) {
  saving = saving.then(() =>
    attempt(async () => {
      const tree = inventory.workspace?.tree;
      const found = tree ? api.findHost(tree, id) : null;
      if (!found) throw new Error(`${id} is no longer in the workspace`);
      const current = $state.snapshot(found);
      const changed =
        typeof changes === "function" ? changes(current) : changes;
      apply(await api.saveHost({ ...current, ...changed }));
    }),
  );
  return saving;
}

export function setFolderDescription(path: string, description: string) {
  return attempt(async () => {
    apply(await api.setFolderDescription(path, description));
  });
}

/**
 * Saves the settings of the folder at `path`, as a function of what it
 * sets as last saved; queued with the host saves.
 */
export function saveFolderSettings(
  path: string,
  change: (current: Settings) => Settings,
) {
  saving = saving.then(() =>
    attempt(async () => {
      const tree = inventory.workspace?.tree;
      const found = tree ? api.findFolder(tree, path) : null;
      if (!found) throw new Error(`${path} is no longer in the workspace`);
      apply(
        await api.saveFolderSettings(
          path,
          change($state.snapshot(found.settings)),
        ),
      );
    }),
  );
  return saving;
}

/** Saves the tags of the folder at `path`, as a function of its last tags. */
export function setFolderTags(
  path: string,
  change: (current: string[]) => string[],
) {
  saving = saving.then(() =>
    attempt(async () => {
      const tree = inventory.workspace?.tree;
      const found = tree ? api.findFolder(tree, path) : null;
      if (!found) throw new Error(`${path} is no longer in the workspace`);
      apply(await api.setFolderTags(path, change($state.snapshot(found.tags))));
    }),
  );
  return saving;
}

/**
 * The settings that apply to `item`, kept up to date as the workspace
 * changes; null until the core has answered. Call during component setup.
 */
export function effectiveSettings(item: () => Item) {
  const state = $state({ current: null as EffectiveSettings | null });
  $effect(() => {
    const target = item();
    // Any edit can change what is inherited.
    void inventory.workspace;
    let stale = false;
    api
      .resolveSettings(target)
      .then((settings) => {
        if (!stale) state.current = settings;
      })
      .catch((error) => {
        if (!stale) inventory.editError = String(error);
      });
    return () => {
      stale = true;
    };
  });
  return state;
}

/** Where a setting of `item` comes from, in words. */
export function describeOrigin(origin: Origin, item: Item): string {
  if (origin.kind === "default") return "default";
  if (origin.kind === "host") return "set on this host";
  if (item.kind === "folder" && item.path === origin.path) {
    return "set on this folder";
  }
  return origin.path === "" ? "from the workspace root" : `from ${origin.path}`;
}

/** Whether `origin` is `item` itself. */
export function isOwn(origin: Origin, item: Item): boolean {
  return item.kind === "host"
    ? origin.kind === "host"
    : origin.kind === "folder" && origin.path === item.path;
}
