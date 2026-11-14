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

/** What a session connects to. */
export type ConnectTarget =
  | ({ kind: "quick" } & QuickConnectTarget)
  | {
      kind: "host";
      /** The host's id in the open workspace. */
      id: string;
      /** What the user typed; null logs in with the host's profile. */
      credentials: { username: string; password: string } | null;
    };

export interface TerminalSize {
  cols: number;
  rows: number;
}

export type SessionState =
  | { state: "connecting" }
  | { state: "connected" }
  | {
      state: "closed";
      reason: string;
      /** Logging in with a password might work (the SSH agent did not). */
      offersPasswordLogin: boolean;
    };

/** Questions the server asks while logging in (keyboard-interactive). */
export interface LoginPrompt {
  /** A title; often empty. */
  name: string;
  /** What to tell the user first; often empty. */
  instructions: string;
  questions: { text: string; echo: boolean }[];
}

export interface Session {
  write(data: Uint8Array): void;
  /** Answers the last login prompt, one answer per question. */
  answer(answers: string[]): void;
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
 * `onPrompt` receives the server's login questions; answer them with
 * `Session.answer`, or close the session to refuse.
 */
export async function openSession(
  target: ConnectTarget,
  size: TerminalSize,
  onOutput: (bytes: Uint8Array, consumed: () => void) => void,
  onState: (state: SessionState) => void,
  onPrompt: (prompt: LoginPrompt) => void,
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
  const promptChannel = new Channel<LoginPrompt>(onPrompt);

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
    onPrompt: promptChannel,
  });
  if (consumedEarly > 0) acknowledge(consumedEarly);

  return {
    write: (data) => send("session_write", { data: Array.from(data) }),
    answer: (answers) => send("session_answer", { answers }),
    resize: (size) => send("session_resize", { size }),
    close: () => send("session_close", {}),
  };
}
