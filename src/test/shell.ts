// A stand-in for the app's shell (`src-tauri`) on the other side of the
// IPC, for the interface's tests: the page runs as it does in the app, and
// what it asks of the shell is recorded here, as the shell would receive
// it. A test answers the commands it is about (`answer`); every other
// command is recorded and answered with nothing. Events (`listen`) are
// Tauri's own mocks; `emit` sends one to the page.

import type { Channel } from "@tauri-apps/api/core";
import { emit } from "@tauri-apps/api/event";
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";

/** One command the page called, with its arguments. */
export interface Call {
  command: string;
  args: Record<string, unknown>;
}

type Answer = (args: Record<string, unknown>) => unknown;

/** A command's answer that the test gives later (`resolve`, `reject`). */
export interface Pending<T> {
  promise: Promise<T>;
  resolve(value: T): void;
  reject(reason: unknown): void;
}

export function pending<T>(): Pending<T> {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

export interface FakeShell {
  /** Every command called so far, oldest first. */
  calls: Call[];
  /** The calls of `command`, oldest first. */
  called(command: string): Call[];
  /** Answers `command` with what `answer` returns (or resolves to, or
   * throws: an error the page sees as the command's refusal). */
  answer(command: string, answer: Answer): void;
  /** Sends the event `event` to the page (every window listening). */
  emit(event: string, payload: unknown): Promise<void>;
}

/**
 * What the shell answers by itself to the commands a page calls on its own
 * (a terminal's highlights, the forwards marker): nothing to show.
 */
const DEFAULTS: Record<string, Answer> = {
  forwards_list: () => [],
  highlight_lines: (args) => (args.lines as string[]).map(() => []),
};

/** Puts a shell in place for this test, in the window `label` (`main`, or
 * `session-<n>` for a session window). */
export function fakeShell(label = "main"): FakeShell {
  const calls: Call[] = [];
  const answers = new Map<string, Answer>();
  mockWindows(label);
  mockIPC(
    (command, args) => {
      const given = (args ?? {}) as Record<string, unknown>;
      calls.push({ command, args: given });
      return (answers.get(command) ?? DEFAULTS[command])?.(given);
    },
    { shouldMockEvents: true },
  );
  return {
    calls,
    called: (command) => calls.filter((call) => call.command === command),
    answer: (command, answer) => void answers.set(command, answer),
    emit: (event, payload) => emit(event, payload),
  };
}

/** The channels a session's events go to, as `session_open` gets them. */
export interface SessionChannels {
  onOutput: Channel<ArrayBuffer>;
  onState: Channel<unknown>;
  onPrompt: Channel<unknown>;
  onHostKey: Channel<unknown>;
  onAgentForwarding: Channel<unknown>;
  onAlgorithms: Channel<unknown>;
  onLogFailure: Channel<unknown>;
  onBackup: Channel<unknown>;
}

/** What the shell sends on a session's channels, as the shell does. */
export function sessionEvents(channels: SessionChannels) {
  const encoder = new TextEncoder();
  return {
    /** Output from the device: raw bytes on the output channel. */
    output(text: string) {
      channels.onOutput.onmessage(encoder.encode(text).buffer as ArrayBuffer);
    },
    connected() {
      channels.onState.onmessage({ state: "connected" });
    },
    closed(reason = "the connection closed") {
      channels.onState.onmessage({
        state: "closed",
        reason,
        offersPasswordLogin: false,
        negotiation: null,
      });
    },
  };
}

/** The bytes of a `session_input` or `session_write` call, as text. */
export function sentText(call: Call): string {
  return new TextDecoder().decode(Uint8Array.from(call.args.data as number[]));
}

/** What `session_open` answers for a session `id` that opened. */
export function opened(id: number) {
  return {
    id,
    consumer: 1,
    encrypted: true,
    agentForwarded: false,
    transport: "ssh",
    record: {
      name: "edge-1",
      title: "admin@edge-1",
      address: null,
      jumpHost: null,
      host: null,
      workspace: null,
      safety: { environment: null, lineDelayMs: 0 },
      role: null,
      insecure: null,
    },
  };
}

/** Lets everything already queued run: promises, then a macrotask. */
export async function settle() {
  for (let round = 0; round < 3; round += 1)
    await new Promise<void>((resolve) => setTimeout(resolve, 0));
}
