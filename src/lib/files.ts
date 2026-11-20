// The Files tab's IPC (#103): a browser of one host's files over SFTP, in
// the core (`netterm_core::files::FileBrowsers`). Its events arrive on a
// Channel; the shell closes it with `files_close` or with the window.

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
  | { kind: "noSftp"; reason: string }
  | { kind: "failed"; reason: string }
  | { kind: "lost"; reason: string };

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
