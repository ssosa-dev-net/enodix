// The interface's view of the core's session interface: bytes in, bytes out,
// resize, close and state changes. Everything here is a call into the Tauri
// shell, which forwards to the core.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { TabId } from "./layout";
import type { InputOrigin } from "./terminalInput";
import type { FlowControl, Parity, PasteSafety, Role } from "./workspace";

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

/** A shell on this computer is the one session that is not a device. */
export type Transport = "ssh" | "telnet" | "serial" | "local";

/** A shell this computer can open as a local session. */
export interface LocalShell {
  /** What the preference and `ConnectTarget` name it by. */
  id: string;
  /** What to show: `PowerShell`, `Ubuntu (WSL)`. */
  name: string;
}

/** The shells this computer has; the first is what an unset preference
 * opens. */
export function localShells(): Promise<LocalShell[]> {
  return invoke<LocalShell[]>("local_shell_list");
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
    }
  /** A host of the open workspace by telnet, whatever its protocol. */
  | { kind: "telnet"; id: string }
  /** A host of the open workspace on its serial console. */
  | { kind: "serial"; id: string }
  /** Any serial port, without an inventory host. */
  | { kind: "quickSerial"; port: string; parameters: SerialParameters }
  /** A shell on this computer: `shell` is one of `localShells`' ids, null
   * the one the preference names. */
  | { kind: "local"; shell: string | null }
  /** A host of the open workspace through the system's ssh client in a
   * local shell: for a device the app's own SSH cannot negotiate with.
   * Marked insecure in the header. */
  | { kind: "systemSsh"; id: string };

export interface TerminalSize {
  cols: number;
  rows: number;
}

export type SessionState =
  | { state: "connecting" }
  | {
      state: "connected";
      /** Whether this connection forwards the SSH agent (settled before it
       * connected). */
      agentForwarded: boolean;
    }
  | {
      state: "closed";
      reason: string;
      /** Logging in with a password might work (the SSH agent did not). */
      offersPasswordLogin: boolean;
      /** The algorithm negotiation failed; null for other failures. */
      negotiation: NegotiationFailure | null;
      /** What to check next, from the core (#116); null when the reason
       * says it, or for a session that ended rather than failed. */
      advice?: string | null;
      /** The next automatic try, from the core (#24): wait, reconnect. */
      retry?: { seconds: number; attempt: number; of: number } | null;
      /** The automatic tries ran out. */
      gaveUp?: boolean;
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
  /** Why the host certificate the server presented was not accepted, if
   * it presented one: the question is then about the key inside it. */
  certificate: string | null;
}

export type HostKeyDecision = "trustAndRemember" | "trustOnce" | "cancel";

/**
 * A workspace host whose settings ask to forward the SSH agent, and that the
 * user has not approved it for: nothing reaches the device until answered.
 */
export interface AgentForwardingQuestion {
  /** The host's name in the workspace. */
  name: string;
  /** The address and port connected to. */
  host: string;
  port: number;
}

/**
 * The answer: connect without forwarding (the default, and what closing the
 * question means), forward for this tab until it is closed, or always
 * forward to this host at this address (kept on this computer).
 */
export type AgentForwardingDecision =
  "withoutForwarding" | "forwardThisSession" | "alwaysForward";

/** A new id for one paste: every line of it carries the id, so the core can
 * refuse the rest of a paste whose first line stopped a snippet's steps. */
export const newPasteId = (): number => Math.floor(Math.random() * 2 ** 47) + 1;

/** Where input came from, as the shell routes it (`InputOrigin` in the
 * core): a terminal's origins (`terminalInput`), and a group paste's line. */
export type SessionInputOrigin = InputOrigin | "group-paste";

export interface Session {
  /** The core's id for the session, for duplicating it. */
  readonly id: number;
  /** Whether its traffic is encrypted on the network (telnet's is not). */
  readonly encrypted: boolean;
  /** Whether it forwarded the SSH agent to the device when opened; each
   * connected state says it again (`agentForwarded`). */
  readonly agentForwarded: boolean;
  readonly transport: Transport;
  /**
   * Input in this session's terminal, with where it came from (#123):
   * typed, the shell sends it to every member of the multi-send group if
   * this session's tab is in it; a group paste's line only to this session
   * and only while its tab is a member; anything else to this session
   * only.
   */
  input(data: Uint8Array, origin: SessionInputOrigin, paste?: number): void;
  /** Input for this session alone (the terminal's replies, mouse reports). */
  write(data: Uint8Array): void;
  /** Sends a break on the line (serial sessions only). */
  sendBreak(): void;
  /** Answers the last login prompt, one answer per question. */
  answer(answers: string[]): void;
  /** Decides the last host-key question. */
  decideHostKey(decision: HostKeyDecision): void;
  /** Answers `onAgentForwarding`'s question. */
  decideAgentForwarding(decision: AgentForwardingDecision): void;
  resize(size: TerminalSize): void;
  close(): void;
  /**
   * Opens the session again once it has closed. A session to a workspace
   * host connects as the host's settings and credential profile are now
   * (#130); a quick connection as it was made. Its events go to the same
   * handlers, starting with `connecting`. Resolves to a note for the
   * terminal when it logs in otherwise than before (null: none); rejects
   * with a `ReconnectFailure` when the host is gone or can no longer be
   * reached that way, or the session is still open.
   */
  reconnect(size: TerminalSize): Promise<string | null>;
  /** Forgets the session (closing it if open), when its terminal goes. */
  discard(): void;
  /**
   * This window gives the session up, to move it to another (#84).
   * Resolves once every byte this window was given has reached `onOutput`;
   * nothing more arrives here after.
   */
  release(): Promise<void>;
  /** Takes the session back after a release, when the move failed. */
  reattach(): Promise<void>;
  /** What the app keeps of the session, as any window shows it. */
  readonly record: SessionRecord;
  /**
   * For a session taken over from another window: what it had reported
   * to the windows before (#86). Null for a session opened here.
   */
  readonly taken: Reported | null;
}

/**
 * What a session reported to the windows shown it before this one (#86):
 * reported once, as events, so a window taking it over starts from these.
 * The events that waited for it are newer.
 */
export interface Reported {
  /** Where it stood (null: nothing shown yet). */
  state: SessionState | null;
  algorithms: NegotiatedAlgorithms[] | null;
  /** Why its log could not be written. */
  logFailure: string | null;
  backup: LoginBackupOutcome | null;
}

/**
 * What the app keeps of each session (#84), whichever window shows it:
 * its name, host and header facts and its paste safety, as they were when
 * it opened.
 */
export interface SessionRecord {
  name: string;
  title: string;
  address: string | null;
  jumpHost: string | null;
  host: string | null;
  workspace: string | null;
  safety: PasteSafety;
  role: Role | null;
  /** Why the session is not safe, when it is the system's ssh client with
   * outdated algorithms rather than an SSH session of the app; null
   * otherwise. */
  insecure: string | null;
}

/** Why a reconnect did not start (`Session.reconnect` rejects with it):
 * the reason and what to do about it. */
export interface ReconnectFailure {
  reason: string;
  advice: string | null;
}

/** A reconnect's failure as the closed state shows it. */
export function reconnectFailure(error: unknown): {
  reason: string;
  advice: string | null;
} {
  if (typeof error === "object" && error !== null && "reason" in error) {
    const failure = error as ReconnectFailure;
    return { reason: String(failure.reason), advice: failure.advice ?? null };
  }
  return { reason: String(error), advice: null };
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
   * Whether to forward the SSH agent (`Session.decideAgentForwarding`),
   * asked before connecting; nothing reaches the device meanwhile.
   */
  onAgentForwarding: (question: AgentForwardingQuestion) => void;
  /**
   * What each connection agreed on (jump hosts first), just before the
   * session is connected.
   */
  onAlgorithms: (algorithms: NegotiatedAlgorithms[]) => void;
  /**
   * The session's log could not be written (why, as a sentence); the
   * session goes on without it.
   */
  onLogFailure: (reason: string) => void;
  /**
   * How the host's backup on login ended (each connection, when the
   * host's `backup_on_login` setting is on); the terminal never waits
   * for it.
   */
  onBackup: (outcome: LoginBackupOutcome) => void;
}

/** How a backup on login ended. */
export type LoginBackupOutcome =
  | { kind: "saved"; version: string }
  | { kind: "unchanged"; version: string }
  | { kind: "failed"; reason: string };

/** Where a new session comes from. */
export type SessionSource =
  /** `name` and `title`: what its tab and header show (its record's). */
  | { kind: "target"; target: ConnectTarget; name: string; title: string }
  /** A second connection like the open session with this id. */
  | { kind: "duplicate"; of: number }
  /**
   * The open session `id`, moved here from another window (#84):
   * `snapshot` is its terminal as that window showed it, written before
   * its output from then on; null when that window died and it is lost.
   */
  | { kind: "adopt"; id: number; snapshot: string | null };

/** Tells the core whether the session's host reconnects by itself. */
export function enableAutoReconnect(id: number, enabled: boolean) {
  return invoke<void>("session_auto_reconnect_enable", { id, enabled });
}

/** A key was pressed in the closed session: no more automatic tries. */
export function stopAutoReconnect(id: number) {
  return invoke<void>("session_auto_reconnect_stop", { id });
}

/** The user reconnects by hand: the automatic tries start afresh. */
export function restartAutoReconnect(id: number) {
  return invoke<void>("session_auto_reconnect_restart", { id });
}

export function appName(): Promise<string> {
  return invoke<string>("app_name");
}

/**
 * The channels a session's events arrive on, and how to acknowledge its
 * output. Output can be consumed before the session id and this window's
 * consumer key are known; it is acknowledged once `ready` gives them. The
 * key ties acknowledgements to these channels: once the session has new
 * ones (a reconnect), the shell ignores acknowledgements for the old.
 */
function channels(handlers: SessionHandlers) {
  let known: { id: number; consumer: number } | null = null;
  let consumedEarly = 0;
  /** Output received on these channels (bytes), from `offset` on. */
  let received = 0;
  let offset = 0;
  let waiting: { upTo: number; resolve: () => void }[] = [];
  const acknowledge = (bytes: number) => {
    if (known === null) consumedEarly += bytes;
    else
      send(known.id, "session_acknowledge", {
        consumer: known.consumer,
        bytes,
      });
  };
  return {
    // Output travels on its own channel as raw bytes, not as JSON.
    onOutput: new Channel<ArrayBuffer>((buffer) => {
      received += buffer.byteLength;
      handlers.onOutput(new Uint8Array(buffer), () =>
        acknowledge(buffer.byteLength),
      );
      const reached = offset + received;
      for (const wait of waiting.filter((each) => reached >= each.upTo))
        wait.resolve();
      waiting = waiting.filter((each) => reached < each.upTo);
    }),
    onState: new Channel<SessionState>(handlers.onState),
    onPrompt: new Channel<LoginPrompt>(handlers.onPrompt),
    onHostKey: new Channel<HostKeyQuestion>(handlers.onHostKey),
    onAgentForwarding: new Channel<AgentForwardingQuestion>(
      handlers.onAgentForwarding,
    ),
    onAlgorithms: new Channel<NegotiatedAlgorithms[]>(handlers.onAlgorithms),
    onLogFailure: new Channel<string>(handlers.onLogFailure),
    onBackup: new Channel<LoginBackupOutcome>(handlers.onBackup),
    /** `startsAt`: the output (bytes) other windows were given first. */
    ready(id: number, consumer: number, startsAt = 0) {
      known = { id, consumer };
      offset = startsAt;
      if (consumedEarly > 0) acknowledge(consumedEarly);
      consumedEarly = 0;
    },
    /** This window's key, once known. */
    key: () => known?.consumer ?? null,
    /**
     * Resolves once the output up to `upTo` (bytes since the relay
     * started) has reached `onOutput`. Channel messages can arrive after
     * a command's answer, so a release waits on this.
     */
    receivedUpTo(upTo: number): Promise<void> {
      if (offset + received >= upTo) return Promise.resolve();
      return new Promise((resolve) => waiting.push({ upTo, resolve }));
    },
  };
}

/** The channels alone, as a command takes them. */
function eventsOf(current: ReturnType<typeof channels>) {
  const { onOutput, onState, onPrompt, onHostKey, onAlgorithms } = current;
  const { onAgentForwarding, onLogFailure, onBackup } = current;
  return {
    onOutput,
    onState,
    onPrompt,
    onHostKey,
    onAgentForwarding,
    onAlgorithms,
    onLogFailure,
    onBackup,
  };
}

/** A session taken over from another window, as the shell returns it. */
interface Attached extends Reported {
  consumer: number;
  offset: number;
  encrypted: boolean;
  agentForwarded: boolean;
  transport: Transport;
}

/** Shows the session `id` here: see `SessionSource`'s `adopt`. */
async function attach(id: number, handlers: SessionHandlers) {
  const current = channels(handlers);
  const attached = await invoke<Attached>("session_attach", {
    id,
    ...eventsOf(current),
  });
  current.ready(id, attached.consumer, attached.offset);
  return { current, attached };
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
  /** The key this window's acknowledgements name. */
  consumer: number;
  encrypted: boolean;
  agentForwarded: boolean;
  transport: Transport;
  record: SessionRecord;
}

/**
 * Opens a session, to a target or as a copy of another session, for the
 * layout's tab `tab` (so the shell knows where its input goes).
 */
export async function openSession(
  source: SessionSource,
  tab: TabId,
  size: TerminalSize,
  handlers: SessionHandlers,
): Promise<Session> {
  let current: ReturnType<typeof channels>;
  let id: number;
  let encrypted: boolean;
  let agentForwarded: boolean;
  let transport: Transport;
  let record: SessionRecord;
  let taken: Reported | null = null;
  if (source.kind === "adopt") {
    const attached = await attach(source.id, handlers);
    current = attached.current;
    ({ encrypted, agentForwarded, transport } = attached.attached);
    id = source.id;
    const { state, algorithms, logFailure, backup } = attached.attached;
    taken = { state, algorithms, logFailure, backup };
    record = (await sessionRecord(id)) ?? NO_RECORD;
  } else {
    current = channels(handlers);
    const opened =
      source.kind === "target"
        ? await invoke<Opened>("session_open", {
            target: source.target,
            tab,
            name: source.name,
            title: source.title,
            size,
            ...eventsOf(current),
          })
        : await invoke<Opened>("session_duplicate", {
            id: source.of,
            tab,
            size,
            ...eventsOf(current),
          });
    ({ id, encrypted, agentForwarded, transport, record } = opened);
    current.ready(id, opened.consumer);
  }

  return {
    id,
    encrypted,
    agentForwarded,
    transport,
    record,
    taken,
    async release() {
      const upTo = await invoke<number>("session_release", {
        id,
        consumer: current.key(),
      });
      await current.receivedUpTo(upTo);
    },
    async reattach() {
      current = (await attach(id, handlers)).current;
    },
    input: (data, origin, paste) =>
      send(id, "session_input", {
        data: Array.from(data),
        origin,
        paste: paste ?? null,
      }),
    write: (data) => send(id, "session_write", { data: Array.from(data) }),
    sendBreak: () => send(id, "session_break", {}),
    answer: (answers) => send(id, "session_answer", { answers }),
    decideHostKey: (decision) =>
      send(id, "session_decide_host_key", { decision }),
    decideAgentForwarding: (decision) =>
      send(id, "session_decide_agent_forwarding", { decision }),
    resize: (size) => send(id, "session_resize", { size }),
    close: () => send(id, "session_close", {}),
    async reconnect(size) {
      // New channels: the old ones ended with the old connection.
      current = channels(handlers);
      const reconnected = await invoke<{
        consumer: number;
        note: string | null;
      }>("session_reconnect", {
        id,
        size,
        ...eventsOf(current),
      });
      current.ready(id, reconnected.consumer);
      return reconnected.note;
    },
    discard: () => send(id, "session_discard", {}),
  };
}

/** Forgets the session `id` (closing it if open): a terminal that was to
 * show it went without it. */
export function discardSession(id: number) {
  send(id, "session_discard", {});
}

/** What the app keeps of the session `id`; null: none (discarded). */
export function sessionRecord(id: number): Promise<SessionRecord | null> {
  return invoke<SessionRecord | null>("session_record", { id });
}

/** A session the app keeps no record of (it should not happen). */
const NO_RECORD: SessionRecord = {
  name: "",
  title: "",
  address: null,
  jumpHost: null,
  host: null,
  workspace: null,
  safety: { environment: null, lineDelayMs: 0 },
  role: null,
  insecure: null,
};
