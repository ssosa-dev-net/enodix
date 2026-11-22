// The Files tab's IPC (#103, #106): a browser of one host's files over
// SFTP (or, without SFTP, transfers by path over SCP), in the core
// (`enodix_core::files::FileBrowsers`), and its transfers. Its events
// arrive on a Channel; each call adding transfers gives another for their
// states. The shell closes them with `files_close_host` or with the window.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { HostKeyDecision, HostKeyQuestion, LoginPrompt } from "./session";

export type EntryKind = "file" | "folder" | "link" | "other";

/** A file or folder as a listing gives it; a device may leave any detail
 * out. */
export interface RemoteEntry {
  name: string;
  kind: EntryKind;
  /** In bytes. */
  size: number | null;
  /** Seconds since 1970. */
  modified: number | null;
  /** As `ls` shows them: `rwxr-xr-x`. */
  permissions: string | null;
}

/** One folder: folders first, then files, each by name. */
export interface Listing {
  folder: string;
  /** Null at the top. */
  parent: string | null;
  entries: RemoteEntry[];
}

export type BrowserEvent =
  | { kind: "hostKey"; question: HostKeyQuestion }
  | { kind: "prompt"; prompt: LoginPrompt }
  | { kind: "connected"; listing: Listing; notice: string | null }
  | {
      kind: "transferOnly";
      reason: string;
      hint: string | null;
      folder: string | null;
    }
  | { kind: "failed"; reason: string }
  | { kind: "lost"; reason: string }
  /** A transfer's question went unanswered (the transfer was cancelled,
   * or its connection gave up): it comes off the tab. */
  | { kind: "questionClosed" };

/** Opens a browser on the host `host` (its stored profile, or the typed
 * `credentials`), starting in `folder` or the host's platform file folder
 * or the login's folder. Returns its id. */
export function openFiles(
  host: string,
  credentials: { username: string; password: string } | null,
  folder: string | null,
  onevent: (event: BrowserEvent) => void,
) {
  return invoke<number>("files_open", {
    host,
    credentials,
    folder,
    onEvent: new Channel<BrowserEvent>(onevent),
  });
}

export const answerFiles = (id: number, answers: string[]) =>
  invoke<void>("files_answer", { id, answers });

export const decideFilesHostKey = (id: number, decision: HostKeyDecision) =>
  invoke<void>("files_decide_host_key", { id, decision });

export const cancelFiles = (id: number) => invoke<void>("files_cancel", { id });

export const closeFiles = (id: number) => invoke<void>("files_close", { id });

/** `path` as typed while `folder` was shown ("" = `folder` again). */
export const listFiles = (id: number, folder: string, path: string) =>
  invoke<Listing>("files_list", { id, folder, path });

export const renameFile = (
  id: number,
  folder: string,
  from: string,
  to: string,
) => invoke<Listing>("files_rename", { id, folder, from, to });

export const deleteFile = (
  id: number,
  folder: string,
  name: string,
  kind: EntryKind,
) => invoke<Listing>("files_delete", { id, folder, name, kind });

export const makeFolder = (id: number, folder: string, name: string) =>
  invoke<Listing>("files_make_folder", { id, folder, name });

/** `bytes` as a person reads it. */
export function sizeText(bytes: number | null): string {
  if (bytes === null) return "";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value < 10 ? 1 : 0)} ${units[unit]}`;
}

/** A modification time in the computer's own format. */
export function timeText(seconds: number | null): string {
  return seconds === null ? "" : new Date(seconds * 1000).toLocaleString();
}

export type Direction = "upload" | "download";

/** A transfer the tab added. */
export interface Added {
  id: number;
  direction: Direction;
  /** The file's name. */
  name: string;
  /** Its path on the device. */
  remote: string;
  /** The local file. */
  local: string;
}

/** Where a transfer is, in the core's words. */
export type TransferState =
  | { kind: "waiting" }
  | { kind: "connecting" }
  | { kind: "running"; done: number; total: number | null }
  | {
      kind: "done";
      bytes: number;
      hashes: { md5: string; sha256: string } | null;
      size: string;
      checked: boolean;
    }
  | { kind: "failed"; reason: string; hint: string | null; partial: string }
  | { kind: "cancelled"; partial: string };

export interface TransferUpdate {
  id: number;
  state: TransferState;
}

const updates = (onupdate: (update: TransferUpdate) => void) =>
  new Channel<TransferUpdate>(onupdate);

/** Uploads the local files `paths` into `folder` on the device. */
export const uploadFiles = (
  id: number,
  folder: string,
  paths: string[],
  onupdate: (update: TransferUpdate) => void,
) =>
  invoke<Added[]>("files_upload", {
    id,
    folder,
    paths,
    onUpdate: updates(onupdate),
  });

/** Downloads the files `names` in `folder` into the local folder `into`. */
export const downloadFiles = (
  id: number,
  folder: string,
  names: string[],
  into: string,
  onupdate: (update: TransferUpdate) => void,
) =>
  invoke<Added[]>("files_download", {
    id,
    folder,
    names,
    into,
    onUpdate: updates(onupdate),
  });

/** Sends the local file `local` to the device path `remote` (SCP form). */
export const sendFile = (
  id: number,
  local: string,
  remote: string,
  onupdate: (update: TransferUpdate) => void,
) =>
  invoke<Added>("files_send", {
    id,
    local,
    remote,
    onUpdate: updates(onupdate),
  });

/** Fetches the device path `remote` into the local folder `into`. */
export const fetchFile = (
  id: number,
  remote: string,
  into: string,
  onupdate: (update: TransferUpdate) => void,
) =>
  invoke<Added>("files_fetch", {
    id,
    remote,
    into,
    onUpdate: updates(onupdate),
  });

/** Which of `names` are in `folder` on the device (none over SCP). */
export const presentFiles = (id: number, folder: string, names: string[]) =>
  invoke<string[]>("files_present", { id, folder, names });

/** Which of `names` are in the local folder `folder`. */
export const existingLocally = (folder: string, names: string[]) =>
  invoke<string[]>("files_existing_locally", { folder, names });

export const cancelTransfer = (transfer: number) =>
  invoke<boolean>("files_cancel_transfer", { transfer });

/** The host's Files tab closed: its browsers and transfers go. */
export const closeFilesHost = (host: string) =>
  invoke<void>("files_close_host", { host });

/** `name` in `folder`, as the device writes paths (the core's
 * `join_path`). */
export function joinPath(folder: string, name: string): string {
  return folder.endsWith("/") || folder.endsWith(":")
    ? `${folder}${name}`
    : `${folder}/${name}`;
}

/** A local path's last part. */
export function baseName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
