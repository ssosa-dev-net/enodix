// The open workspace as the interface shows it: the tree, the selected
// item, which folders are expanded, and the dialog in front, plus the user's
// edits. Every edit is made by the core, which writes the files and returns
// the reloaded workspace.

import * as api from "./workspace";
import type {
  Edited,
  EffectiveSettings,
  Folder,
  Host,
  Origin,
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
  expanded: {} as Record<string, boolean>,
  dialog: null as DialogRequest | null,
  /** While dragging: the folder the item would be dropped into. */
  dropTarget: null as string | null,
});

export function setWorkspace(workspace: WorkspaceView | null) {
  inventory.workspace = workspace;
  inventory.selected = null;
  inventory.expanded = {};
  inventory.editError = null;
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

/** The folder new items go into: the selected folder, or the selected host's. */
export function targetFolder(): string {
  const selected = inventory.selected;
  if (selected?.kind === "folder") return selected.path;
  if (selected?.kind === "host") return api.parentPath(selected.id);
  return "";
}

/** Expands every folder above `path` (and `path` itself, if `self`). */
export function reveal(path: string, self = false) {
  const names = path.split("/");
  const upTo = self ? names.length : names.length - 1;
  for (let i = 1; i <= upTo; i++) {
    inventory.expanded[names.slice(0, i).join("/")] = true;
  }
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

/** Applies an edit's answer; returns its result. Errors propagate. */
function apply<T>(edited: Edited<T>): T {
  inventory.workspace = edited.workspace;
  inventory.editError = null;
  return edited.result;
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

async function renameItem(item: Item, name: string) {
  const from = itemPath(item);
  const to = apply(
    item.kind === "host"
      ? await api.renameHost(item.id, name)
      : await api.renameFolder(item.path, name),
  );
  moved(from, to);
}

async function moveItem(item: Item, folder: string) {
  const from = itemPath(item);
  const to = apply(
    item.kind === "host"
      ? await api.moveHost(item.id, folder)
      : await api.moveFolder(item.path, folder),
  );
  moved(from, to);
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
