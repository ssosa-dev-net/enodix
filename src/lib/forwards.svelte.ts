// Port forwards of the open sessions (#21): local ones (`ssh -L`) and
// SOCKS5 proxies (`ssh -D`), set per host in the workspace. The core opens
// them with each SSH session; this keeps the window's view of every open
// one, asked again whenever any session's forwards change.

import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** A forward of an open session, as the core reports it. */
export type ForwardStatus = {
  name: string;
  /** As written: `L 8443:10.0.0.1:443`, `D 1080`. */
  spec: string;
  /** Why the last connection through it could not be carried, if one. */
  lastFailure: string | null;
} & (
  | { state: "listening"; address: string; port: number }
  | { state: "failed"; reason: string }
  | { state: "stopped" }
);

/** One open session's forwards. */
export interface SessionForwards {
  id: number;
  name: string | null;
  title: string | null;
  forwards: ForwardStatus[];
}

/** Every open session that has forwards, kept current. */
export const forwards = $state<{ sessions: SessionForwards[] }>({
  sessions: [],
});

let started: Promise<void> | null = null;

async function reload() {
  try {
    forwards.sessions = await invoke<SessionForwards[]>("forwards_list");
  } catch {
    // Kept as it was: the next change asks again.
  }
}

/** Follows the forwards of every session; once per window. */
export function followForwards(): Promise<void> {
  started ??= (async () => {
    await listen("forwards-changed", () => void reload());
    await reload();
  })();
  return started;
}

/** The forwards of the session `id`, as last heard. */
export function forwardsOf(id: number | null): ForwardStatus[] {
  if (id === null) return [];
  return forwards.sessions.find((s) => s.id === id)?.forwards ?? [];
}

/** Stops one forward of a session; the session goes on. */
export async function stopForward(id: number, name: string): Promise<void> {
  await invoke<boolean>("forward_stop", { id, name });
  await reload();
}

/** Where a listening forward is reached, as a program is pointed at it. */
export function forwardAddress(status: ForwardStatus): string | null {
  if (status.state !== "listening") return null;
  const address = status.address.includes(":")
    ? `[${status.address}]`
    : status.address;
  return `${address}:${status.port}`;
}
