// XMODEM and YMODEM transfers on a serial session (#47). The core runs the
// protocol over the session's line (`enodix_core::session::modem`); this
// starts and cancels one, and follows its progress, which the shell tells
// every window as `session-transfer` events (each window keeps those of
// the sessions it shows).

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";

export type ModemProtocol = "xmodem" | "xmodemCrc" | "xmodem1k" | "ymodem";

/** The protocols, as the transfer form offers them. */
export const MODEM_PROTOCOLS: {
  id: ModemProtocol;
  name: string;
  hint: string;
}[] = [
  {
    id: "xmodem1k",
    name: "XMODEM-1K",
    hint: "1 KiB blocks with a CRC: what most boot loaders take",
  },
  {
    id: "xmodemCrc",
    name: "XMODEM-CRC",
    hint: "128-byte blocks with a CRC",
  },
  {
    id: "xmodem",
    name: "XMODEM",
    hint: "128-byte blocks with a checksum, for old devices",
  },
  {
    id: "ymodem",
    name: "YMODEM",
    hint: "Sends each file's name and size; several files at once",
  },
];

export const protocolName = (id: ModemProtocol) =>
  MODEM_PROTOCOLS.find((protocol) => protocol.id === id)?.name ?? id;

export type TransferAction =
  { kind: "send"; path: string[] } | { kind: "receive"; path: string };

export type TransferState =
  | { kind: "waiting" }
  | { kind: "running" }
  | { kind: "done" }
  | { kind: "cancelled"; byDevice: boolean }
  | { kind: "failed"; reason: string };

/** How a transfer stands, as the core reports it. */
export interface TransferStatus {
  direction: "send" | "receive";
  protocol: ModemProtocol;
  state: TransferState;
  /** The file being moved now (its name). */
  file: string | null;
  fileBytes: number;
  fileSize: number | null;
  filesDone: number;
  /** Files to send in all (sending only). */
  filesTotal: number | null;
  bytes: number;
  blocks: number;
  retries: number;
  /** The files written, receiving. */
  saved: string[];
}

export const transferEnded = (status: TransferStatus) =>
  status.state.kind !== "waiting" && status.state.kind !== "running";

/** Starts a transfer on the serial session `id`; rejects with the reason
 * it could not start (a file that cannot be read, say). */
export function startTransfer(
  id: number,
  protocol: ModemProtocol,
  action: TransferAction,
): Promise<void> {
  return invoke("session_transfer_start", {
    id,
    request: { protocol, action },
  });
}

/** Stops the transfer running on the session `id`. */
export function cancelTransfer(id: number): Promise<void> {
  return invoke("session_transfer_cancel", { id });
}

/** Calls `handler` with each status of the transfers on the session `id`. */
export function followTransfers(
  id: number,
  handler: (status: TransferStatus) => void,
): Promise<UnlistenFn> {
  return listen<{ id: number; status: TransferStatus }>(
    "session-transfer",
    (event) => {
      if (event.payload.id === id) handler(event.payload.status);
    },
  );
}

/** Asks for the files to send: one, or several for YMODEM. */
export async function pickFilesToSend(several: boolean): Promise<string[]> {
  const picked = await open({
    title: several ? "Files to send" : "File to send",
    multiple: several,
  });
  if (picked === null) return [];
  return Array.isArray(picked) ? picked : [picked];
}

/** Asks where to receive into: a file for XMODEM (it sends no name), a
 * folder for YMODEM. */
export async function pickReceiveTarget(
  protocol: ModemProtocol,
): Promise<string | null> {
  if (protocol === "ymodem") {
    const folder = await open({
      directory: true,
      title: "Folder to receive the files into",
    });
    return typeof folder === "string" ? folder : null;
  }
  return save({ title: "Save the received file as" });
}

/** The last part of a path, as the form shows a picked file. */
export function baseName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}
