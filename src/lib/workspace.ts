// The interface's view of the core's workspace: the tree it loaded and the
// files it could not load. Everything here is a call into the Tauri shell.

import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

export interface Host {
  /** The host file's path in the workspace; identifies the host. */
  id: string;
  name: string;
  address: string;
  port: number | null;
}

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
