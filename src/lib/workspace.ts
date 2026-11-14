// The interface's view of the core's workspace: the tree it loaded, the
// files it could not load, and the edits it can make. Everything here is a
// call into the Tauri shell; the core writes the files and returns the
// reloaded workspace.

import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

export interface CustomField {
  key: string;
  value: string;
}

export interface Host {
  /** The host file's path in the workspace; identifies the host. */
  id: string;
  name: string;
  /** The management address. */
  address: string;
  port: number | null;
  vendor: string | null;
  platform: string | null;
  model: string | null;
  serial_number: string | null;
  site: string | null;
  building: string | null;
  room: string | null;
  rack: string | null;
  rack_unit: string | null;
  description: string | null;
  tags: string[];
  custom: CustomField[];
}

/** The optional built-in text fields of a host, in display order. */
export const HOST_TEXT_FIELDS = [
  ["vendor", "Vendor"],
  ["platform", "Platform"],
  ["model", "Model"],
  ["serial_number", "Serial number"],
  ["site", "Site"],
  ["building", "Building"],
  ["room", "Room"],
  ["rack", "Rack"],
  ["rack_unit", "Rack unit"],
] as const satisfies readonly (readonly [keyof Host, string])[];

export interface Folder {
  name: string;
  /** Path relative to the workspace root; empty for the root. */
  path: string;
  description: string | null;
  folders: Folder[];
  hosts: Host[];
}

export interface Problem {
  file: string;
  message: string;
}

export interface WorkspaceView {
  root: string;
  tree: Folder;
  problems: Problem[];
}

/** The workspace after an edit, and what the edit returned. */
export interface Edited<T> {
  workspace: WorkspaceView;
  result: T;
}

export type FolderDeleted =
  | { kind: "deleted"; workspace: WorkspaceView }
  | { kind: "needsConfirmation"; hosts: number };

/** Asks the user for a workspace folder; null if they cancel. */
export async function pickWorkspaceFolder(): Promise<string | null> {
  const folder = await open({
    directory: true,
    title: "Open workspace folder",
  });
  return typeof folder === "string" ? folder : null;
}

/** Opens a workspace folder; it reopens on the next launch. */
export function openWorkspace(path: string): Promise<WorkspaceView> {
  return invoke<WorkspaceView>("workspace_open", { path });
}

/** Reopens the last opened workspace, if any. */
export function restoreWorkspace(): Promise<WorkspaceView | null> {
  return invoke<WorkspaceView | null>("workspace_restore");
}

/** Returns the new folder's path. */
export function createFolder(parent: string, name: string) {
  return invoke<Edited<string>>("folder_create", { parent, name });
}

/** Returns the folder's new path. */
export function renameFolder(path: string, name: string) {
  return invoke<Edited<string>>("folder_rename", { path, name });
}

/** Returns the folder's new path. */
export function moveFolder(path: string, parent: string) {
  return invoke<Edited<string>>("folder_move", { path, parent });
}

/**
 * Deletes a folder. Unless `includingHosts`, a folder holding hosts is not
 * deleted, and the answer says how many hosts it holds.
 */
export function deleteFolder(path: string, includingHosts: boolean) {
  return invoke<FolderDeleted>("folder_delete", {
    path,
    deletion: includingHosts ? "includingHosts" : "ifNoHosts",
  });
}

export function setFolderDescription(path: string, description: string) {
  return invoke<Edited<null>>("folder_set_description", {
    path,
    description: description.trim() === "" ? null : description,
  });
}

/** Returns the new host's id. */
export function createHost(folder: string, name: string, address: string) {
  return invoke<Edited<string>>("host_create", { folder, name, address });
}

/** Returns the host's new id. */
export function renameHost(id: string, name: string) {
  return invoke<Edited<string>>("host_rename", { id, name });
}

/** Returns the host's new id. */
export function moveHost(id: string, folder: string) {
  return invoke<Edited<string>>("host_move", { id, folder });
}

export function deleteHost(id: string) {
  return invoke<Edited<null>>("host_delete", { id });
}

/** Saves a host's fields (not its name or place: see rename and move). */
export function saveHost(host: Host) {
  return invoke<Edited<null>>("host_save", { host });
}

export function findFolder(tree: Folder, path: string): Folder | null {
  if (path === "") return tree;
  let folder: Folder | undefined = tree;
  for (const name of path.split("/")) {
    folder = folder.folders.find((child) => child.name === name);
    if (!folder) return null;
  }
  return folder;
}

export function findHost(tree: Folder, id: string): Host | null {
  const slash = id.lastIndexOf("/");
  const folder = findFolder(tree, slash < 0 ? "" : id.slice(0, slash));
  return folder?.hosts.find((host) => host.id === id) ?? null;
}

/** The folder path a host or folder is in. */
export function parentPath(path: string): string {
  const slash = path.lastIndexOf("/");
  return slash < 0 ? "" : path.slice(0, slash);
}

/** Every folder of the tree, root first, depth first. */
export function allFolders(tree: Folder): Folder[] {
  return [tree, ...tree.folders.flatMap(allFolders)];
}
