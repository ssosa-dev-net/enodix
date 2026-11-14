// The interface's view of the core's session interface: bytes in, bytes out,
// resize, close and state changes. Everything here is a call into the Tauri
// shell, which forwards to the core.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { FlowControl, Parity } from "./workspace";

export interface QuickConnectTarget {
  host: string;
  port: number;
  username: string;
  password: string;
}

/** Serial line parameters; the default is 9600 8N1, no flow control. */
export interface SerialParameters {
  baudRate: number;
  dataBits: number;
  parity: Parity;
  stopBits: number;
  flowControl: FlowControl;
}

export const DEFAULT_SERIAL_PARAMETERS: SerialParameters = {
  baudRate: 9600,
  dataBits: 8,
  parity: "none",
  stopBits: 1,
  flowControl: "none",
};

/** A serial port this computer has. */
export interface SerialPortInfo {
  /** What to open it by, such as `COM3`. */
  name: string;
  /** The adapter, as its driver names it; may be empty. */
  description: string;
}

/** The serial ports there are now, in port order. */
export function serialPorts(): Promise<SerialPortInfo[]> {
  return invoke<SerialPortInfo[]>("serial_ports");
}

export type Transport = "ssh" | "telnet" | "serial";

/** What a session connects to. */
export type ConnectTarget =
  | ({ kind: "quick" } & QuickConnectTarget)
  | {
      kind: "host";
      /** The host's id in the open workspace. */
      id: string;
      /** What the user typed; null logs in with the host's profile. */
      credentials: { username: string; password: string } | null;
    }
  /** A host of the open workspace by telnet, whatever its protocol. */
  | { kind: "telnet"; id: string }
  /** A host of the open workspace on its serial console. */
  | { kind: "serial"; id: string }
  /** Any serial port, without an inventory host. */
  | { kind: "quickSerial"; port: string; parameters: SerialParameters };

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
      /** The algorithm negotiation failed; null for other failures. */
      negotiation: NegotiationFailure | null;
    };

export type AlgorithmKind = "keyExchange" | "hostKey" | "cipher" | "mac";

/** Algorithms per kind, as named on the wire. */
export interface AlgorithmLists {
  keyExchange: string[];
  hostKey: string[];
  cipher: string[];
  mac: string[];
}

/** The device had no algorithm in common with the session for some kind. */
export interface NegotiationFailure {
  /** The jump host that failed; null for the device. */
  jumpHost: string | null;
  /** The first kind nothing was in common for. */
  kind: AlgorithmKind;
  /** What the device (or jump host) offered. */
  offered: AlgorithmLists;
  /** Whether the session had the legacy-crypto switch on. */
  legacyCrypto: boolean;
  /** Turning the legacy-crypto switch on would help. */
  legacyCryptoWouldHelp: boolean;
  /** The kind nothing is supported for even with the switch on. */
  unsupported: AlgorithmKind | null;
}

/** The algorithms one connection of a session agreed on. */
export interface NegotiatedAlgorithms {
  /** The jump host this connection went to; null for the device. */
  jumpHost: string | null;
  keyExchange: string;
  hostKey: string;
  cipher: string;
  mac: string;
  /** The outdated (legacy-crypto) algorithms among these. */
  legacy: string[];
}

/** Questions the server asks while logging in (keyboard-interactive). */
export interface LoginPrompt {
  /** A title; often empty. */
  name: string;
  /** What to tell the user first; often empty. */
  instructions: string;
  questions: { text: string; echo: boolean }[];
}

/** A host key as the user sees it. */
export interface HostKey {
  /** The key type, such as `ssh-ed25519`. */
  algorithm: string;
  /** `SHA256:` and base64, as OpenSSH shows it. */
  fingerprint: string;
}

/** The server presented a host key that is not trusted for it. */
export interface HostKeyQuestion {
  host: string;
  port: number;
  key: HostKey;
  /** The keys trusted until now; empty on the first connection, otherwise
   * the key has changed. */
  stored: HostKey[];
}

export type HostKeyDecision = "trustAndRemember" | "trustOnce" | "cancel";

export interface Session {
  /** The core's id for the session, for duplicating it. */
  readonly id: number;
  /** Whether its traffic is encrypted on the network (telnet's is not). */
  readonly encrypted: boolean;
  readonly transport: Transport;
  write(data: Uint8Array): void;
  /** Sends a break on the line (serial sessions only). */
  sendBreak(): void;
  /** Answers the last login prompt, one answer per question. */
  answer(answers: string[]): void;
  /** Decides the last host-key question. */
  decideHostKey(decision: HostKeyDecision): void;
  resize(size: TerminalSize): void;
  close(): void;
  /**
   * Opens the session again once it has closed: the same target, settings
   * and login. Its events go to the same handlers, starting with
   * `connecting`; rejects if it is still open.
   */
  reconnect(size: TerminalSize): Promise<void>;
  /** Forgets the session (closing it if open), when its terminal goes. */
  discard(): void;
}

/** What the interface does with a session's events. */
export interface SessionHandlers {
  /**
   * Must call `consumed` once the bytes have been processed (for xterm.js,
   * from the write callback): flow control in the core stops reading from
   * the device while too much output is unconsumed.
   */
  onOutput: (bytes: Uint8Array, consumed: () => void) => void;
  onState: (state: SessionState) => void;
  /**
   * The server's login questions; answer them with `Session.answer`, or
   * close the session to refuse.
   */
  onPrompt: (prompt: LoginPrompt) => void;
  /**
   * A host key to decide on (`Session.decideHostKey`); the connection
   * waits for the decision.
   */
  onHostKey: (question: HostKeyQuestion) => void;
  /**
   * What each connection agreed on (jump hosts first), just before the
   * session is connected.
   */
  onAlgorithms: (algorithms: NegotiatedAlgorithms[]) => void;
}

/** Where a new session comes from. */
export type SessionSource =
  | { kind: "target"; target: ConnectTarget }
  /** A second connection like the open session with this id. */
  | { kind: "duplicate"; of: number };

export function appName(): Promise<string> {
  return invoke<string>("app_name");
}

/**
 * The channels a session's events arrive on, and how to acknowledge its
 * output. Output can be consumed before the session id is known; it is
 * acknowledged once `ready` gives the id.
 */
function channels(handlers: SessionHandlers) {
  let id: number | null = null;
  let consumedEarly = 0;
  const acknowledge = (bytes: number) => {
    if (id === null) consumedEarly += bytes;
    else send(id, "session_acknowledge", { bytes });
  };
  return {
    // Output travels on its own channel as raw bytes, not as JSON.
    onOutput: new Channel<ArrayBuffer>((buffer) =>
      handlers.onOutput(new Uint8Array(buffer), () =>
        acknowledge(buffer.byteLength),
      ),
    ),
    onState: new Channel<SessionState>(handlers.onState),
    onPrompt: new Channel<LoginPrompt>(handlers.onPrompt),
    onHostKey: new Channel<HostKeyQuestion>(handlers.onHostKey),
    onAlgorithms: new Channel<NegotiatedAlgorithms[]>(handlers.onAlgorithms),
    ready(known: number) {
      id = known;
      if (consumedEarly > 0) acknowledge(consumedEarly);
    },
  };
}

/**
 * Calls a command on the session `id`. A call can lose a race with the
 * session closing; the closed state has already told the interface, so
 * that failure is not reported again.
 */
function send(id: number, command: string, args: Record<string, unknown>) {
  invoke(command, { id, ...args }).catch(() => {});
}

/** A session just opened, as the shell returns it. */
interface Opened {
  id: number;
  encrypted: boolean;
  transport: Transport;
}

/** Opens a session, to a target or as a copy of another session. */
export async function openSession(
  source: SessionSource,
  size: TerminalSize,
  handlers: SessionHandlers,
): Promise<Session> {
  const { ready, ...events } = channels(handlers);
  const { id, encrypted, transport } =
    source.kind === "target"
      ? await invoke<Opened>("session_open", {
          target: source.target,
          size,
          ...events,
        })
      : await invoke<Opened>("session_duplicate", {
          id: source.of,
          size,
          ...events,
        });
  ready(id);

  return {
    id,
    encrypted,
    transport,
    write: (data) => send(id, "session_write", { data: Array.from(data) }),
    sendBreak: () => send(id, "session_break", {}),
    answer: (answers) => send(id, "session_answer", { answers }),
    decideHostKey: (decision) =>
      send(id, "session_decide_host_key", { decision }),
    resize: (size) => send(id, "session_resize", { size }),
    close: () => send(id, "session_close", {}),
    async reconnect(size) {
      // New channels: the old ones ended with the old connection.
      const { ready, ...events } = channels(handlers);
      await invoke("session_reconnect", { id, size, ...events });
      ready(id);
    },
    discard: () => send(id, "session_discard", {}),
  };
}
