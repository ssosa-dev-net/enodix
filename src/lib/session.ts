// The interface's view of the core's session interface: bytes in, bytes out,
// resize, close and state changes. Everything here is a call into the Tauri
// shell, which forwards to the core.

import { Channel, invoke } from "@tauri-apps/api/core";

export interface QuickConnectTarget {
  host: string;
  port: number;
  username: string;
  password: string;
}

export interface TerminalSize {
  cols: number;
  rows: number;
}

export type SessionState =
  | { state: "connecting" }
  | { state: "connected" }
  | { state: "closed"; reason: string };

export interface Session {
  write(data: Uint8Array): void;
  resize(size: TerminalSize): void;
  close(): void;
}

export function appName(): Promise<string> {
  return invoke<string>("app_name");
}

/**
 * Opens a session. `onOutput` must call `consumed` once the bytes have been
 * processed (for xterm.js, from the write callback): flow control in the
 * core stops reading from the device while too much output is unconsumed.
 */
export async function openSession(
  target: QuickConnectTarget,
  size: TerminalSize,
  onOutput: (bytes: Uint8Array, consumed: () => void) => void,
  onState: (state: SessionState) => void,
): Promise<Session> {
  let id: number | null = null;
  // Output can be consumed before the session id is known.
  let consumedEarly = 0;
  const acknowledge = (bytes: number) => {
    if (id === null) consumedEarly += bytes;
    else send("session_acknowledge", { bytes });
  };

  // Output travels on its own channel as raw bytes, not as JSON.
  const outputChannel = new Channel<ArrayBuffer>((buffer) =>
    onOutput(new Uint8Array(buffer), () => acknowledge(buffer.byteLength)),
  );
  const stateChannel = new Channel<SessionState>(onState);

  // A call can lose a race with the session closing; the closed state has
  // already told the interface, so that failure is not reported again.
  const send = (command: string, args: Record<string, unknown>) => {
    invoke(command, { id, ...args }).catch(() => {});
  };

  id = await invoke<number>("session_open", {
    target,
    size,
    onOutput: outputChannel,
    onState: stateChannel,
  });
  if (consumedEarly > 0) acknowledge(consumedEarly);

  return {
    write: (data) => send("session_write", { data: Array.from(data) }),
    resize: (size) => send("session_resize", { size }),
    close: () => send("session_close", {}),
  };
}
