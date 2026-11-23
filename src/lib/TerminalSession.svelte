<script lang="ts">
  import { FitAddon } from "@xterm/addon-fit";
  import { SearchAddon, type ISearchOptions } from "@xterm/addon-search";
  import { SerializeAddon } from "@xterm/addon-serialize";
  import { Terminal } from "@xterm/xterm";
  import { onMount, tick, untrack } from "svelte";
  import { webglRenderer } from "./renderer";
  import { terminalInput, type TerminalInput } from "./terminalInput";
  import { unserializedModes } from "./handover";
  import { giveKeys } from "./keys";
  import { highlighting, highlightTerminal } from "./highlight.svelte";
  import HostKeyCheck from "./HostKeyCheck.svelte";
  import LoginQuestions from "./LoginQuestions.svelte";
  import RoleIcon from "./RoleIcon.svelte";
  import SessionMenu, {
    type SessionAction,
    type SessionMove,
  } from "./SessionMenu.svelte";
  import Ellipsis from "@lucide/svelte/icons/ellipsis";
  import { PROTOCOL_NAMES } from "./ways";
  import NegotiationFailure from "./NegotiationFailure.svelte";
  import PasteConfirm from "./PasteConfirm.svelte";
  import { ENVIRONMENT_NAMES, planPaste } from "./paste";
  import {
    fontStack,
    isDark,
    preferences,
    terminalTheme,
    zoomFont,
    type Preferences,
  } from "./preferences.svelte";
  import {
    discardSession,
    openSession,
    reconnectFailure,
    type HostKeyDecision,
    type HostKeyQuestion,
    type LoginPrompt,
    type LoginBackupOutcome,
    type NegotiatedAlgorithms,
    type Session,
    type SessionSource,
    type SessionState,
  } from "./session";
  import type { TabId } from "./layout";
  import type { Environment, PasteSafety, Role } from "./workspace";

  interface Props {
    source: SessionSource;
    /** The layout's tab this session is shown in. */
    tab: TabId;
    /**
     * The multi-send group's size if this session's tab is in it, else
     * null: what is typed here then reaches every connected member.
     */
    groupSize: number | null;
    /**
     * Adds this session's tab to the multi-send group, or takes it out;
     * without it multi-send is not offered.
     */
    ontogglegroup?: () => void;
    /** Ends the multi-send group. */
    onendgroup?: () => void;
    /**
     * Takes a paste (or snippet text) made here while the tab is in the
     * multi-send group: the group's paste is planned and confirmed once for
     * every member, then sent to each with `pasteLines` (#36).
     */
    ongrouppaste: (text: string) => void;
    /** Shown in the header, such as `user@host:port`. */
    title: string;
    /** The host's address, if the title does not show it; null: none. */
    address: string | null;
    /** The jump host the session goes through; null: none. */
    jumpHost: string | null;
    /** Told the terminal's columns and rows whenever they change. */
    onsize?: (size: { cols: number; rows: number }) => void;
    /** The role of the session's host, if it has one. */
    role: Role | null;
    /** Its host's environment (the border colour) and paste line delay. */
    safety: PasteSafety;
    /** The session's name, as its tab shows it (for the paste question). */
    name: string;
    /** Whether this session is on screen (the tab its pane shows). */
    shown: boolean;
    /** Whether the keyboard goes to it (the session of the active pane). */
    focused: boolean;
    /** Raised to give the focused terminal the keys again (a tab clicked). */
    focusRequest?: number;
    /** Told every state the session goes through. */
    onstate?: (state: SessionState) => void;
    /** Called when the user leaves this session's view. */
    ondismiss: () => void;
    /** Called with this session's id when the user duplicates it. */
    /** Called with this session's id when the user duplicates it; without
     * it (a session window), Duplicate is not offered. */
    onduplicate?: (id: number) => void;
    /**
     * Called when the user asks to log in with a password after the SSH
     * agent did not work; without it, that is not offered.
     */
    onpasswordlogin?: (reason: string) => void;
    /**
     * Opens the details sheet on the session's host; without it (quick
     * connect, or a host no longer in the open workspace) there is none.
     */
    ondetails?: () => void;
    /**
     * Opens the host's Files tab (#103), in the main window; offered for
     * SSH sessions only. Without it (quick connect, a host not in the open
     * workspace) there is none.
     */
    onfiles?: () => void;
    /**
     * Moving the session to another window (#84, #85): into a new window
     * or an open session window, or back to the main window. Offered in
     * the session's menu, each disabled while `moveRefusal` (or its own
     * `refusal`) gives a reason.
     */
    moves?: SessionMove[];
    /**
     * Why the window holds a question about this session (snippet
     * variables, a group paste), which a move would lose; null: none.
     */
    windowQuestion?: string | null;
  }

  let {
    source,
    tab,
    groupSize,
    ontogglegroup,
    onendgroup,
    ongrouppaste,
    title,
    address,
    jumpHost,
    onsize,
    role,
    safety,
    name,
    shown,
    focused,
    focusRequest = 0,
    onstate,
    ondismiss,
    onduplicate,
    onpasswordlogin,
    ondetails,
    onfiles,
    moves = [],
    windowQuestion = null,
  }: Props = $props();

  /** Written first in a terminal whose earlier output is lost (#84). */
  const LOST_LINE =
    "\x1b[2m--- earlier output was lost: the window showing it closed ---\x1b[0m\r\n";

  /** Set once the session has gone to another window: not discarded. */
  let moved = false;
  /** Pastes queued or still being sent, line by line. */
  let pastes = $state(0);

  /**
   * Why the session cannot move to another window now, or null (#84):
   * what lives in this window would be lost or cut in half by a move.
   */
  export function moveRefusal(): string | null {
    if (!session) return "Not until the session has opened";
    if (status.state === "connecting")
      return "Not while the session is connecting";
    if (prompt || hostKey || pasteQuestion)
      return "Not while a question about the session is open";
    if (windowQuestion) return windowQuestion;
    if (pastes > 0) return "Not while a paste is still being sent";
    return null;
  }

  /** Gives the session up for another window (set up on mount). */
  let giveUp: () => Promise<{
    id: number;
    snapshot: string;
  } | null> = async () => null;

  /**
   * Gives the session up to move it to another window (#84): once every
   * byte this window was given is in the terminal, returns the session's
   * id and the terminal's screen and scrollback, with its modes, as
   * escape sequences. Null if it cannot move now (`moveRefusal`). From
   * then on the terminal shows nothing more, and going away leaves the
   * session open; `takeBack` undoes it if the move fails.
   */
  export function handOver() {
    return giveUp();
  }

  /** Shows the session here again after a failed move. */
  export async function takeBack() {
    if (!session) return;
    moved = false;
    await session.reattach();
  }

  /** Output held for a hidden terminal, acknowledged at once (see `deliver`). */
  const HIDDEN_BUFFER_BYTES = 1 << 20;

  let container: HTMLDivElement;
  /** For focusing the terminal again after a login prompt. */
  let xterm: Terminal | null = null;
  let session = $state<Session | null>(null);
  let status = $state<SessionState>({ state: "connecting" });
  /** The server's login questions waiting for answers, with a new key each. */
  let prompt = $state<{ prompt: LoginPrompt; key: number } | null>(null);
  let prompts = 0;
  /** The host key waiting for a decision, with a new key each. */
  let hostKey = $state<{ question: HostKeyQuestion; key: number } | null>(null);
  /** What each connection agreed on, jump hosts first. */
  let algorithms = $state<NegotiatedAlgorithms[]>([]);
  /** Whether this connection showed output before it connected: an SSH
   * login banner (#134), which the login questions leave readable. */
  let bannerShown = $state(false);
  /** Why the session's log could not be written; null: logging works. */
  let logFailure = $state<string | null>(null);
  /** How this connection's backup on login ended; null: none (yet). */
  let backup = $state<LoginBackupOutcome | null>(null);
  /** Whether the failed backup's notice was dismissed. */
  let backupNoticeDismissed = $state(false);
  /** A paste waiting for the user to confirm it; see `pasteText`. */
  let pasteQuestion = $state<{
    text: string;
    lineCount: number;
    environment: Environment | null;
    lineDelayMs: number;
    answer: (send: boolean) => void;
  } | null>(null);

  function answerPaste(send: boolean) {
    pasteQuestion?.answer(send);
    pasteQuestion = null;
    xterm?.focus();
  }

  /** The connections that use outdated algorithms (legacy crypto). */
  const legacy = $derived(algorithms.filter((a) => a.legacy.length > 0));
  const legacyTitle = $derived(
    legacy
      .map(
        (a) =>
          `${a.jumpHost ? `Jump host ${a.jumpHost}` : "This device"}: ${a.legacy.join(", ")}`,
      )
      .join("\n"),
  );

  function decideHostKey(decision: HostKeyDecision) {
    session?.decideHostKey(decision);
    hostKey = null;
    xterm?.focus();
  }

  function answer(answers: string[]) {
    session?.answer(answers);
    prompt = null;
    xterm?.focus();
  }

  function cancelLogin() {
    prompt = null;
    session?.close();
    // Closed, Enter reconnects: the keys come back here at once.
    xterm?.focus();
  }

  const statusText = $derived(
    status.state === "connecting"
      ? "Connecting…"
      : status.state === "connected"
        ? "Connected"
        : `Closed: ${status.reason}`,
  );

  /** The card over the terminal that takes the closed session's keys;
   * the closed bar under its backdrop is left out, as its button could not
   * be clicked (#94). */
  const closedCard = $derived(
    status.state === "closed" &&
      (status.negotiation !== null ||
        (status.offersPasswordLogin && onpasswordlogin !== undefined)),
  );

  /** For reconnecting at the terminal's current size. */
  let currentSize: () => { cols: number; rows: number } = () => ({
    cols: 80,
    rows: 24,
  });
  /** Marks where the old connection's output ends in the scrollback. */
  let markReconnect: () => void = () => {};
  /** Writes the core's note on a reconnect under the marker: the profile it
   * logs in with now, or why it asks for the login (#130). */
  let noteReconnect: (note: string) => void = () => {};

  /** Opens the closed session again in this terminal: a host's session
   * with the host's settings and profile as they are now (#130). */
  function reconnect() {
    if (!session || status.state !== "closed") return;
    const reconnecting = session;
    status = { state: "connecting" };
    prompt = null;
    hostKey = null;
    algorithms = [];
    bannerShown = false;
    logFailure = null;
    backup = null;
    backupNoticeDismissed = false;
    markReconnect();
    const requested = currentSize();
    reconnecting.reconnect(requested).then(
      (note) => {
        if (note) noteReconnect(note);
        // The terminal may have changed size while the shell was
        // reconnecting (the closed notice goes away at once): a resize
        // sent before the new connection existed reached the old one.
        const now = currentSize();
        if (now.cols !== requested.cols || now.rows !== requested.rows) {
          reconnecting.resize(now);
        }
      },
      (error) => {
        // Not tried: the host is gone, or no longer reached this way.
        const { reason, advice } = reconnectFailure(error);
        status = {
          state: "closed",
          reason,
          offersPasswordLogin: false,
          negotiation: null,
          advice,
        };
      },
    );
  }

  /** Whether the scrollback search bar is open. */
  let searching = $state(false);
  let query = $state("");
  let caseSensitive = $state(false);
  let regex = $state(false);
  /** The current match (from 0, -1: none) and how many; null: not run. */
  let results = $state<{ index: number; count: number } | null>(null);
  let searchInput = $state<HTMLInputElement>();
  /** Set up on mount: finds `query` forwards or backwards. */
  let find: (
    direction: "next" | "previous",
    incremental?: boolean,
  ) => void = () => {};
  let clearSearch: () => void = () => {};

  const resultsText = $derived(
    results === null || query === ""
      ? ""
      : results.count === 0
        ? "No results"
        : results.index < 0
          ? `${results.count} found`
          : `${results.index + 1} of ${results.count}`,
  );

  function openSearch() {
    searching = true;
    // Once shown (or again, if already open): the input takes the keys.
    void tick().then(() => {
      searchInput?.focus();
      searchInput?.select();
    });
  }

  function closeSearch() {
    searching = false;
    results = null;
    clearSearch();
    xterm?.focus();
  }

  function onsearchkey(event: KeyboardEvent) {
    if (event.key === "Enter" || event.key === "F3") {
      event.preventDefault();
      find(event.shiftKey ? "previous" : "next");
    }
  }

  /** Escape closes the search from any of its controls: one clicked with
   * the mouse (Aa, say) has the keys then (#94). */
  function onsearchbarkey(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      closeSearch();
    }
  }

  /** The search's arrows leave the keys in its field, so Enter goes on
   * finding after a click (#94). */
  function keepSearchKeys(event: MouseEvent) {
    event.preventDefault();
  }

  // The terminal takes the keys when it comes on screen and when the
  // session closes (Enter reconnects), unless a card or the search bar
  // over it takes them. That comes a moment after the user asked (the
  // core's layout answer, say): never if the keyboard has moved since
  // (`giveKeys`, #141), so keys typed in another pane meanwhile stay there.
  $effect(() => {
    void focusRequest;
    if (
      focused &&
      !prompt &&
      !hostKey &&
      !closedCard &&
      !searching &&
      !pasteQuestion &&
      // A question drawn by the window (closing this tab, joining) keeps
      // the keys: this pane may become active after it took them.
      !document.activeElement?.closest("[role=dialog]")
    )
      giveKeys(xterm);
  });

  $effect(() => onstate?.(status));

  /** Brings the terminal up to date and on screen; set up on mount. */
  let reveal: () => void = () => {};
  /** Keeps the terminal's output aside while it is out of sight. */
  let conceal: () => void = () => {};
  /** Sends text as pasted (set up on mount); see `sendText`. `alone`: the
   * paste was begun for this tab alone (outside the group), whatever the
   * tab is now. */
  let pasteText: (text: string, alone?: boolean) => void = () => {};
  /** Pastes lines into this session alone (set up on mount). */
  let sendLines: (
    lines: string[],
    lineDelayMs: number,
  ) => Promise<void> = async () => {};

  /**
   * Pastes `lines` (each with its line break) into this session only, as
   * one member's part of a multi-send group paste that was already planned
   * and confirmed: `lineDelayMs` apart, stopping if the session ends or
   * its tab leaves the group (#123; the shell refuses such a line too).
   * Resolves once the last line is sent (or it stopped).
   */
  export function pasteLines(
    lines: string[],
    lineDelayMs: number,
  ): Promise<void> {
    return sendLines(lines, lineDelayMs);
  }

  /** The core's id for the session, once it has opened. */
  export function sessionId(): number | null {
    return session?.id ?? null;
  }

  /**
   * Sends `text` to this session alone the way a paste is sent (a
   * snippet's lines, each ending in a line break): for a snippet begun
   * outside the multi-send group, which stays this tab's even if the tab
   * joined while its variables were asked (#123). Only while connected;
   * returns whether it was sent.
   */
  export function sendText(text: string): boolean {
    if (!session || status.state !== "connected") return false;
    pasteText(text, true);
    return true;
  }

  /** The terminal's columns and rows, for the window's status line. */
  let terminalSize = $state({ cols: 80, rows: 24 });
  $effect(() => onsize?.(terminalSize));

  /** Where the header's menu of actions is open; null: closed. */
  let menuAt = $state<{ x: number; y: number } | null>(null);
  let menuButton = $state<HTMLButtonElement>();

  /**
   * The session's actions as things stand, for the header's menu and the
   * tab's context menu (#67). Their shortcuts are the window's (App).
   */
  export function actions(): SessionAction[] {
    const back = () => xterm?.focus();
    const list: SessionAction[] = [];
    if (ondetails) {
      const details = ondetails;
      list.push({
        id: "details",
        label: "Details",
        title: "Show this host's details over the Hosts view",
        attributes: { "data-session-details": "" },
        run: details,
      });
    }
    if (onfiles && session?.transport === "ssh") {
      list.push({
        id: "files",
        label: "Files",
        title:
          "Browse this host's files over SFTP, in a tab of the main window",
        run: onfiles,
      });
    }
    if (onduplicate) {
      const duplicate = onduplicate;
      list.push({
        id: "duplicate",
        label: "Duplicate",
        title: "Open a second session to the same host",
        disabled: !session,
        run: () => session && duplicate(session.id),
      });
    }
    for (const move of moves) {
      const refusal = moveRefusal() ?? move.refusal?.() ?? null;
      list.push({
        id: move.id,
        label: move.label,
        shortcut: move.shortcut,
        title: refusal ?? move.title,
        disabled: refusal !== null,
        attributes: { [`data-${move.id}`]: "", ...move.attributes },
        run: move.run,
      });
    }
    if (status.state === "closed") {
      list.push(
        {
          id: "reconnect",
          label: "Reconnect",
          shortcut: "Enter",
          disabled: !session,
          run: () => {
            reconnect();
            back();
          },
        },
        {
          id: "new-connection",
          label: "New connection",
          title: "Close this tab and show the connect view in its place",
          run: ondismiss,
        },
      );
    } else {
      list.push({
        id: "disconnect",
        label: "Disconnect",
        disabled: !session,
        run: () => {
          session?.close();
          back();
        },
      });
    }
    if (ontogglegroup) {
      const toggle = ontogglegroup;
      list.push({
        id: "multi-send",
        label:
          groupSize === null
            ? "Join the multi-send group"
            : "Leave the multi-send group",
        shortcut: "Ctrl+Shift+M",
        title:
          groupSize === null
            ? "What you type in a member reaches every member"
            : undefined,
        attributes: {
          "data-multi-send-toggle": groupSize === null ? "join" : "leave",
        },
        run: () => {
          toggle();
          back();
        },
      });
    }
    if (groupSize !== null && onendgroup) {
      const end = onendgroup;
      list.push({
        id: "end-group",
        label: "End the multi-send group",
        attributes: { "data-end-group": "" },
        run: () => {
          end();
          back();
        },
      });
    }
    if (session?.transport === "serial" && status.state === "connected") {
      list.push({
        id: "send-break",
        label: "Send break",
        title: "Send a break signal on the line (to interrupt a device's boot)",
        attributes: { "data-send-break": "" },
        run: () => {
          session?.sendBreak();
          back();
        },
      });
    }
    return list;
  }

  function toggleMenu(event: MouseEvent) {
    if (menuAt) {
      menuAt = null;
      return;
    }
    const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
    // Right-aligned under the button (SessionMenu keeps it in the window).
    menuAt = { x: box.right - 192, y: box.bottom + 2 };
  }

  /** Gives the terminal the font, scrollback and colours chosen. */
  let applyPreferences: (chosen: Preferences, dark: boolean) => void = () => {};
  /** Turns keyword highlighting on or off, for the theme's colours. */
  let updateHighlights: (on: boolean, dark: boolean) => void = () => {};

  onMount(() => {
    const chosen = untrack(() => preferences.current);
    const terminal = new Terminal({
      cursorBlink: true,
      fontFamily: fontStack(chosen.fontFamily),
      fontSize: chosen.fontSize,
      scrollback: chosen.scrollback,
      theme: terminalTheme(untrack(isDark)),
      // For the search addon's match highlighting and count (decorations).
      allowProposedApi: true,
      // Hyperlinks a device prints (OSC 8) open nothing: without this,
      // xterm asks with a dialog and then calls window.open, which the
      // window refuses, so the question would only mislead.
      linkHandler: { activate: () => {} },
    });
    xterm = terminal;
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    const search = new SearchAddon();
    terminal.loadAddon(search);
    const serializer = new SerializeAddon();
    terminal.loadAddon(serializer);
    terminal.open(container);
    fit.fit();
    // Decoration over what the screen shows; the bytes are not touched.
    const highlights = highlightTerminal(terminal);
    updateHighlights = (on, dark) => highlights.update(on, dark);
    // For the smoke test, which reads the screen from the buffer (the WebGL
    // renderer draws no text into the page) and the highlighted cells.
    Object.assign(container, { xterm: terminal, highlights });
    const webgl = webglRenderer(terminal);
    /** Input with its origin; set up below, once the session handlers are. */
    let input: TerminalInput | null = null;

    // Output reaches the terminal only while it is on screen; meanwhile it
    // is held, and written when the terminal is shown again. Held output is
    // acknowledged at once up to `HIDDEN_BUFFER_BYTES`, so the device keeps
    // sending; beyond that only once written, so flow control then pauses
    // the device instead of the buffer growing without end.
    let live = false;
    /** Set while held output is parsed at the old size (no fitting). */
    let catchingUp = false;
    let held: { bytes: Uint8Array; consumed: (() => void) | undefined }[] = [];
    let heldBytes = 0;
    const deliver = (bytes: Uint8Array, consumed: () => void) => {
      if (live) {
        terminal.write(bytes, consumed);
        return;
      }
      const early = heldBytes < HIDDEN_BUFFER_BYTES;
      if (early) consumed();
      held.push({ bytes, consumed: early ? undefined : consumed });
      heldBytes += bytes.length;
    };
    reveal = () => {
      if (live) return;
      live = true;
      webgl.show();
      // The held output was sent for the terminal's old size: it is parsed
      // at that size, and the terminal fits its pane after it.
      for (const { bytes, consumed } of held) terminal.write(bytes, consumed);
      held = [];
      heldBytes = 0;
      catchingUp = true;
      terminal.write("", () => {
        catchingUp = false;
        fit.fit();
      });
    };
    conceal = () => {
      if (!live) return;
      live = false;
      webgl.hide();
    };
    if (untrack(() => shown)) reveal();

    applyPreferences = (chosen, dark) => {
      const options = terminal.options;
      const fontFamily = fontStack(chosen.fontFamily);
      const resized =
        options.fontFamily !== fontFamily ||
        options.fontSize !== chosen.fontSize;
      if (options.fontFamily !== fontFamily) options.fontFamily = fontFamily;
      if (options.fontSize !== chosen.fontSize)
        options.fontSize = chosen.fontSize;
      if (options.scrollback !== chosen.scrollback)
        options.scrollback = chosen.scrollback;
      options.theme = terminalTheme(dark);
      // A new cell size changes how many rows and columns fit.
      if (resized && live && !catchingUp) fit.fit();
    };

    // Scrollback search: matches are highlighted, the current one stands out.
    search.onDidChangeResults(
      ({ resultIndex, resultCount }) =>
        (results = { index: resultIndex, count: resultCount }),
    );
    find = (direction, incremental = false) => {
      if (query === "") {
        search.clearDecorations();
        results = null;
        return;
      }
      // The colours are the theme's --highlight-* tokens (app.css), read
      // now: the window sets the theme on the root before this runs again.
      const style = getComputedStyle(document.documentElement);
      const token = (name: string) =>
        style.getPropertyValue(`--highlight-${name}`).trim();
      const options: ISearchOptions = {
        caseSensitive,
        regex,
        incremental,
        decorations: {
          matchBackground: token("match"),
          activeMatchBackground: token("match-active"),
          matchOverviewRuler: token("match-ruler"),
          activeMatchColorOverviewRuler: token("match-active-ruler"),
        },
      };
      try {
        if (direction === "next") search.findNext(query, options);
        else search.findPrevious(query, options);
      } catch {
        // An unfinished regular expression: no results yet.
        results = { index: -1, count: 0 };
      }
    };
    clearSearch = () => search.clearDecorations();

    // Copy and paste. Every paste (shortcut, right click, the browser's own
    // paste) and every snippet goes through `pasteText`, then the
    // terminal's own (bracketed paste, line endings), so paste safety has
    // one way in. The core plans each paste (`planPaste`): a multi-line
    // one is confirmed first (always on production hosts), showing the
    // whole text, before any line is sent; then the lines go one by one,
    // each (with its line break) as a write of its own, the host's paste
    // line delay apart. Pastes go in order, one at a time.
    const copySelection = () => {
      const text = terminal.getSelection();
      if (text) void navigator.clipboard.writeText(text).catch(() => {});
    };
    const wait = (ms: number) =>
      new Promise<void>((resolve) => setTimeout(resolve, ms));
    const ask = (text: string, lineCount: number, rules: PasteSafety) =>
      new Promise<boolean>((answer) => {
        pasteQuestion = {
          text,
          lineCount,
          environment: rules.environment,
          lineDelayMs: rules.lineDelayMs,
          answer,
        };
      });
    // Read afresh after each wait (the session may end meanwhile).
    const ended = () => disposed || status.state === "closed";
    const sendPaste = async (text: string) => {
      // A session that ended takes nothing (and asks nothing).
      if (ended()) return;
      const rules = untrack(() => safety);
      // A paste the core cannot plan is not sent (it may need asking).
      const plan = await planPaste(text, rules);
      if (ended()) return;
      if (plan.confirm) {
        const send = await ask(text, plan.lines.length, {
          ...rules,
          lineDelayMs: plan.lineDelayMs,
        });
        if (!send || ended()) return;
      }
      await pasteEach(plan.lines, plan.lineDelayMs, false);
    };
    /**
     * Sends the lines to this session (never fanned out, #123); `group`:
     * this member's part of a group paste, which stops once its tab is no
     * longer a member.
     */
    const pasteEach = async (
      lines: string[],
      lineDelayMs: number,
      group: boolean,
    ) => {
      for (const [index, line] of lines.entries()) {
        if (index > 0 && lineDelayMs > 0) await wait(lineDelayMs);
        // A session that ended (or a terminal gone) takes no more lines.
        if (ended()) return;
        // A paste belongs to the group it began in: a member that left
        // gets no more of it.
        if (group && untrack(() => groupSize) === null) return;
        pastingGroup = group;
        try {
          input?.paste(line);
        } finally {
          pastingGroup = false;
        }
      }
    };
    let pasting: Promise<void> = Promise.resolve();
    /** Queues `send`, counted in `pastes` until it is done (#84: a paste
     * in flight is never cut short by a move). */
    const queuePaste = (send: () => Promise<void>) => {
      pastes += 1;
      pasting = pasting
        .then(send)
        .catch(() => {})
        .finally(() => (pastes -= 1));
      return pasting;
    };
    pasteText = (text: string, alone = false) => {
      if (text === "") return;
      // A member's paste is the group's: planned once for every member.
      // Decided here, once: a paste begun outside the group stays this
      // tab's if the tab joins meanwhile (its lines never fan out, #123).
      if (!alone && untrack(() => groupSize) !== null) {
        if (!ended()) ongrouppaste(text);
        return;
      }
      void queuePaste(() => sendPaste(text));
    };
    sendLines = (lines, lineDelayMs) =>
      queuePaste(() => pasteEach(lines, lineDelayMs, true));
    const paste = async () => {
      try {
        pasteText(await navigator.clipboard.readText());
      } catch {
        // Nothing readable on the clipboard.
      }
    };
    /** What a key does in the terminal instead of reaching the device. */
    const terminalKey = (event: KeyboardEvent): (() => void) | null => {
      if (event.altKey || event.metaKey) return null;
      const ctrl = event.ctrlKey;
      const shift = event.shiftKey;
      const letter = (name: string) =>
        event.code === `Key${name.toUpperCase()}` ||
        event.key.toLowerCase() === name;
      if (ctrl && shift && letter("c")) return copySelection;
      // Ctrl+C copies only text that is selected; otherwise it interrupts.
      if (ctrl && !shift && letter("c") && terminal.hasSelection()) {
        return () => {
          copySelection();
          terminal.clearSelection();
        };
      }
      if (ctrl && !shift && event.key === "Insert") return copySelection;
      if (ctrl && letter("v")) return () => void paste();
      if (!ctrl && shift && event.key === "Insert") return () => void paste();
      if (ctrl && shift && letter("f")) return openSearch;
      return null;
    };
    /** The key whose press was taken, so that its keypress and keyup are. */
    let taken: string | null = null;
    terminal.attachCustomKeyEventHandler((event) => {
      if (event.type !== "keydown") {
        if (taken === null || event.code !== taken) return true;
        if (event.type === "keyup") taken = null;
        return false;
      }
      const action = terminalKey(event);
      if (!action) return true;
      // Also keeps the browser from pasting a second time by itself.
      event.preventDefault();
      taken = event.code;
      action();
      return false;
    });
    // The browser's own paste (its context menu) is taken from xterm.js.
    const onpaste = (event: ClipboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      pasteText(event.clipboardData?.getData("text/plain") ?? "");
    };
    // Copy-on-select copies what the mouse selects (not a search match),
    // when the button is released, wherever the drag ends. (Copying later,
    // outside the click, is refused by the webview.)
    const onmouseup = (event: MouseEvent) => {
      if (event.button !== 0) return;
      if (untrack(() => preferences.current.copyOnSelect)) copySelection();
    };
    const onmousedown = (event: MouseEvent) => {
      if (event.button !== 0) return;
      window.addEventListener("mouseup", onmouseup, { once: true });
    };
    const oncontextmenu = (event: MouseEvent) => {
      if (!untrack(() => preferences.current.rightClickPaste)) return;
      event.preventDefault();
      void paste();
    };
    // Ctrl+scroll sizes the font (every terminal's: it is a preference). A
    // mouse wheel's notch is one step; a touchpad's small steps add up.
    let wheel = 0;
    const onwheel = (event: WheelEvent) => {
      if (!event.ctrlKey) return;
      event.preventDefault();
      event.stopPropagation();
      const delta =
        event.deltaMode === WheelEvent.DOM_DELTA_PIXEL
          ? event.deltaY
          : event.deltaY * 40;
      if (Math.sign(delta) !== Math.sign(wheel)) wheel = 0;
      wheel += delta;
      if (Math.abs(wheel) < 40) return;
      void zoomFont(wheel < 0 ? "in" : "out");
      wheel = 0;
    };
    container.addEventListener("paste", onpaste, { capture: true });
    container.addEventListener("mousedown", onmousedown, { capture: true });
    container.addEventListener("contextmenu", oncontextmenu);
    container.addEventListener("wheel", onwheel, {
      capture: true,
      passive: false,
    });

    let disposed = false;
    /** Set while one of this member's lines of a group paste goes out. */
    let pastingGroup = false;
    const encoder = new TextEncoder();
    const size = () => ({ cols: terminal.cols, rows: terminal.rows });
    currentSize = size;
    markReconnect = () =>
      terminal.write("\r\n\x1b[2m--- reconnecting ---\x1b[0m\r\n");
    noteReconnect = (note) => terminal.write(`\x1b[2m${note}\x1b[0m\r\n`);

    // Every input goes to the shell with its origin (#34, #123), which
    // routes it: only typed keys fan out to the multi-send group. A paste
    // reaches the group only as a group paste (each member's lines through
    // its own terminal); the terminal's replies and mouse reports depend on
    // this terminal's screen and go to this session only.
    input = terminalInput(terminal, ({ data, binary, origin }) => {
      // A closed session keeps its scrollback; Enter reconnects it.
      if (status.state === "closed") {
        if (data === "\r") reconnect();
        return;
      }
      const bytes = binary
        ? // One character per byte (some mouse reports).
          Uint8Array.from(data, (char) => char.charCodeAt(0))
        : encoder.encode(data);
      // Only a connected session shares (input while connecting is
      // dropped by the core anyway, and must not reach the other members).
      if (status.state !== "connected") session?.write(bytes);
      else if (origin === "pasted" && pastingGroup)
        session?.input(bytes, "group-paste");
      else session?.input(bytes, origin);
    });
    // For the smoke test, which checks where input came from.
    Object.assign(container, { input });
    terminalSize = size();
    terminal.onResize((newSize) => {
      terminalSize = { cols: newSize.cols, rows: newSize.rows };
      session?.resize(newSize);
    });
    const resizeObserver = new ResizeObserver(() => {
      if (!catchingUp) fit.fit();
    });
    resizeObserver.observe(container);

    // A session moved here (#84): what the window giving it up showed
    // comes first, then its output from then on.
    const adopted = untrack(() => source);
    if (adopted.kind === "adopt") terminal.write(adopted.snapshot ?? LOST_LINE);
    /** Whether a state arrived here yet (it is newer than a taken one). */
    let stateSeen = false;
    giveUp = async () => {
      if (!session || moveRefusal() !== null) return null;
      const giving = session;
      // Held output goes into xterm first, or the snapshot would miss it.
      reveal();
      await giving.release();
      await new Promise<void>((resolve) => terminal.write("", resolve));
      // A paste or a question may have begun meanwhile: it stays here.
      if (moveRefusal() !== null) {
        await giving.reattach().catch(() => {});
        return null;
      }
      moved = true;
      return {
        id: giving.id,
        snapshot: serializer.serialize() + unserializedModes(terminal),
      };
    };
    const requestedSize = size();
    openSession(
      source,
      untrack(() => tab),
      requestedSize,
      {
        // Acknowledged once xterm.js has parsed it, so a flood of output
        // never queues up faster than the terminal can show it.
        onOutput: (bytes, consumed) => {
          if (status.state === "connecting") bannerShown = true;
          deliver(bytes, consumed);
        },
        onState: (state) => {
          stateSeen = true;
          status = state;
          if (state.state !== "connecting") {
            prompt = null;
            hostKey = null;
          }
          // Nothing is pasted into a session that ended.
          if (state.state === "closed" && pasteQuestion) {
            pasteQuestion.answer(false);
            pasteQuestion = null;
          }
        },
        onPrompt: (asked) => (prompt = { prompt: asked, key: ++prompts }),
        onHostKey: (question) => (hostKey = { question, key: ++prompts }),
        onAlgorithms: (agreed) => (algorithms = agreed),
        onLogFailure: (reason) => (logFailure = reason),
        onBackup: (outcome) => {
          backup = outcome;
          // Every window refreshes its config views on the shell's
          // config-saved (#78), this one included.
          backupNoticeDismissed = false;
        },
      },
    ).then(
      (opened) => {
        if (disposed) {
          opened.discard();
          return;
        }
        session = opened;
        // What it reported to the window before, unless newer events
        // (those that waited for this window) said otherwise (#86).
        const taken = opened.taken;
        if (taken) {
          if (taken.state && !stateSeen) status = taken.state;
          if (taken.algorithms && algorithms.length === 0)
            algorithms = taken.algorithms;
          logFailure ??= taken.logFailure;
          backup ??= taken.backup;
        }
        // The window may have been resized while the session was opening;
        // a session moved here learns this terminal's size whatever it is.
        const current = size();
        if (
          adopted.kind === "adopt" ||
          current.cols !== requestedSize.cols ||
          current.rows !== requestedSize.rows
        ) {
          opened.resize(current);
        }
        // Opened a while after it was asked for: not over a move since.
        if (untrack(() => focused)) giveKeys(terminal);
      },
      (error) =>
        (status = {
          state: "closed",
          reason: String(error),
          offersPasswordLogin: false,
          negotiation: null,
        }),
    );

    return () => {
      disposed = true;
      pasteQuestion?.answer(false);
      container.removeEventListener("paste", onpaste, { capture: true });
      container.removeEventListener("mousedown", onmousedown, {
        capture: true,
      });
      window.removeEventListener("mouseup", onmouseup);
      container.removeEventListener("contextmenu", oncontextmenu);
      container.removeEventListener("wheel", onwheel, { capture: true });
      resizeObserver.disconnect();
      // Gone with its terminal: it can no longer be reconnected. A session
      // moved to another window lives on there; one moved here that could
      // not be shown goes with this terminal too.
      if (!moved) {
        if (session) session.discard();
        else if (adopted.kind === "adopt") discardSession(adopted.id);
      }
      highlights.dispose();
      input?.dispose();
      webgl.dispose();
      terminal.dispose();
    };
  });

  // Declared after onMount, so that the terminal exists when it first runs.
  $effect(() => {
    if (shown) reveal();
    else conceal();
  });

  $effect(() => applyPreferences(preferences.current, isDark()));

  $effect(() => {
    void highlighting.version;
    updateHighlights(preferences.current.highlighting, isDark());
  });

  // A changed option (or theme: the match colours) searches again from
  // where the search is.
  $effect(() => {
    void caseSensitive;
    void regex;
    void isDark();
    // After the window's own effect has set the root's theme (the colours
    // are read from it), so on the next frame.
    const frame = requestAnimationFrame(() => {
      if (untrack(() => searching)) untrack(() => find("next", true));
    });
    return () => cancelAnimationFrame(frame);
  });
</script>

<div
  class="session"
  class:grouped={groupSize !== null}
  data-environment={safety.environment}
  data-session-environment={safety.environment ?? "none"}
  data-multi-send={groupSize === null
    ? undefined
    : status.state === "connected"
      ? "receiving"
      : "not-receiving"}
>
  <header
    class:has-role={role}
    style:--role-color={role?.color}
    data-session-header
  >
    {#if role}
      <span class="role" data-session-role={role.id} title={role.name}>
        <RoleIcon {role} />
        <span class="role-name">{role.name}</span>
      </span>
    {/if}
    <span class="title" title={name === title ? undefined : title}>{title}</span
    >
    {#if address}
      <span class="detail" data-session-address>{address}</span>
    {/if}
    {#if jumpHost}
      <span
        class="detail"
        data-session-jump-host={jumpHost}
        title="Reached through the jump host {jumpHost}">via {jumpHost}</span
      >
    {/if}
    {#if session}
      <span class="protocol" data-session-protocol={session.transport}
        >{PROTOCOL_NAMES[session.transport]}</span
      >
    {/if}
    {#if safety.environment}
      <span
        class="marker environment"
        data-environment-badge={safety.environment}
        title={safety.environment === "production"
          ? "A production device: a paste of more than one line always asks first"
          : `Marked as ${ENVIRONMENT_NAMES[safety.environment].toLowerCase()}`}
      >
        {ENVIRONMENT_NAMES[safety.environment]}
      </span>
    {/if}
    {#if session && !session.encrypted}
      <span
        class="marker unencrypted"
        data-unencrypted
        title="Everything this session sends, passwords included, crosses the network unencrypted (telnet)"
      >
        Unencrypted
      </span>
    {/if}
    {#if status.state === "connected" && legacy.length > 0}
      <span
        class="marker legacy"
        data-legacy-crypto={legacy.map((a) => a.legacy.join(",")).join(";")}
        title={`Outdated algorithms in use:\n${legacyTitle}`}
      >
        Legacy crypto
      </span>
    {/if}
    {#if groupSize !== null}
      <span
        class="marker multi-send"
        data-multi-send-bar
        data-group-size={groupSize}
        title={groupSize === 1
          ? "In the multi-send group: no other member yet"
          : `In the multi-send group: typing reaches all ${groupSize} sessions of the group (Ctrl+Shift+M leaves it)`}
      >
        Multi-send {groupSize}
        {#if status.state !== "connected"}
          <span
            class="not-receiving"
            data-not-receiving
            title="Not receiving while {status.state === 'closed'
              ? 'disconnected'
              : 'connecting'}">· not receiving</span
          >
        {/if}
      </span>
    {/if}
    {#if backup}
      <span
        class="marker backup {backup.kind}"
        data-login-backup={backup.kind}
        data-version={backup.kind === "failed" ? undefined : backup.version}
        title={backup.kind === "saved"
          ? `Backed up on login: the configuration was saved as ${backup.version}`
          : backup.kind === "unchanged"
            ? `Backed up on login: the configuration is unchanged since ${backup.version}`
            : `The backup on login failed: ${backup.reason}`}
      >
        {backup.kind === "failed" ? "Not backed up" : "Backed up"}
      </span>
    {/if}
    {#if logFailure}
      <span
        class="marker log-off"
        data-log-failed-marker
        title="The session log cannot be written: {logFailure}"
      >
        Not logging
      </span>
    {/if}
    <span class="status {status.state}" role="status" title={statusText}
      >{statusText}</span
    >
    <button
      class="menu-button"
      data-session-menu
      aria-label="Session actions"
      title="Session actions (also on the tab's context menu)"
      aria-haspopup="menu"
      aria-expanded={menuAt !== null}
      bind:this={menuButton}
      onmousedown={(event) => event.preventDefault()}
      onclick={toggleMenu}><Ellipsis size={16} aria-hidden="true" /></button
    >
  </header>
  {#if menuAt}
    <SessionMenu
      entries={actions()}
      x={menuAt.x}
      y={menuAt.y}
      opener={menuButton}
      onclose={() => {
        menuAt = null;
        xterm?.focus();
      }}
    />
  {/if}
  <div class="body">
    <div class="terminal" bind:this={container}></div>
    {#if searching}
      <!-- Escape from any of its controls (the bar's own key, not a widget's). -->
      <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
      <form
        class="search"
        role="search"
        data-terminal-search
        onkeydown={onsearchbarkey}
        onsubmit={(event) => {
          event.preventDefault();
          find("next");
        }}
      >
        <input
          class="compact"
          name="search"
          aria-label="Search the scrollback"
          placeholder="Search"
          autocomplete="off"
          spellcheck="false"
          bind:this={searchInput}
          bind:value={query}
          oninput={() => find("next", true)}
          onkeydown={onsearchkey}
        />
        <span class="results" data-search-results aria-live="polite"
          >{resultsText}</span
        >
        <label title="Match case">
          <input
            type="checkbox"
            name="search-case"
            bind:checked={caseSensitive}
          />
          Aa
        </label>
        <label title="Regular expression">
          <input type="checkbox" name="search-regex" bind:checked={regex} />
          .*
        </label>
        <button
          type="button"
          class="compact"
          data-search="previous"
          title="Previous match (Shift+Enter)"
          aria-label="Previous match"
          onmousedown={keepSearchKeys}
          onclick={() => find("previous")}>&uarr;</button
        >
        <button
          type="button"
          class="compact"
          data-search="next"
          title="Next match (Enter)"
          aria-label="Next match"
          onmousedown={keepSearchKeys}
          onclick={() => find("next")}>&darr;</button
        >
        <button
          type="button"
          class="compact"
          data-search="close"
          title="Close (Escape)"
          aria-label="Close the search"
          onclick={closeSearch}>&times;</button
        >
      </form>
    {/if}
    {#if logFailure}
      <div class="log-failed" role="alert" data-log-failed>
        <span class="reason">{logFailure}.</span>
        <button
          data-log-failed-dismiss
          onclick={() => {
            logFailure = null;
            xterm?.focus();
          }}
        >
          Dismiss
        </button>
      </div>
    {/if}
    {#if backup?.kind === "failed" && !backupNoticeDismissed}
      <div class="backup-failed" role="status" data-login-backup-failed>
        <span class="reason">The backup on login failed: {backup.reason}.</span>
        <button
          data-login-backup-dismiss
          onclick={() => {
            backupNoticeDismissed = true;
            xterm?.focus();
          }}
        >
          Dismiss
        </button>
      </div>
    {/if}
    {#if closedCard}
      <!-- The card over the terminal says it and offers a way on. -->
    {:else if status.state === "closed" && session}
      <div class="ended" role="alert" data-session-ended>
        <span class="reason">{status.reason}.</span>
        {#if status.advice}
          <!-- What to change (#116); Enter then uses it (#130). -->
          <span data-session-advice>{status.advice}</span>
        {/if}
        <span>Press <kbd>Enter</kbd> to reconnect.</span>
        <button data-session-new-connection onclick={ondismiss}
          >New connection</button
        >
      </div>
    {:else if status.state === "closed"}
      <!-- It never opened, so there is nothing to reconnect. -->
      <div class="ended" role="alert" data-session-failed>
        <span class="reason">{status.reason}.</span>
        {#if status.advice}
          <span data-session-advice>{status.advice}</span>
        {/if}
        <button data-session-new-connection onclick={ondismiss}
          >New connection</button
        >
      </div>
    {/if}
    {#if hostKey}
      {#key hostKey.key}
        <HostKeyCheck
          question={hostKey.question}
          ready={session !== null}
          ondecide={decideHostKey}
        />
      {/key}
    {/if}
    {#if prompt}
      {#key prompt.key}
        <LoginQuestions
          prompt={prompt.prompt}
          ready={session !== null}
          banner={bannerShown}
          onanswer={answer}
          oncancel={cancelLogin}
        />
      {/key}
    {/if}
    {#if pasteQuestion}
      <PasteConfirm
        text={pasteQuestion.text}
        lineCount={pasteQuestion.lineCount}
        target={name}
        environment={pasteQuestion.environment}
        lineDelayMs={pasteQuestion.lineDelayMs}
        onsend={() => answerPaste(true)}
        oncancel={() => answerPaste(false)}
      />
    {/if}
    {#if status.state === "closed" && status.negotiation}
      <NegotiationFailure failure={status.negotiation} {ondismiss} />
    {/if}
    {#if status.state === "closed" && status.offersPasswordLogin && onpasswordlogin}
      {@const reason = status.reason}
      <!-- Below the device's login banner, if it sent one (#134). -->
      <div class="overlay" class:below-banner={bannerShown}>
        <form
          class="card"
          role="alert"
          data-password-fallback
          onsubmit={(event) => {
            event.preventDefault();
            onpasswordlogin?.(reason);
          }}
        >
          <h2>Could not log in with the SSH agent</h2>
          <p>{reason}.</p>
          {#if status.advice}
            <p data-session-advice>{status.advice}</p>
          {/if}
          <div class="actions">
            <button type="button" onclick={ondismiss}>Cancel</button>
            <button type="submit">Log in with a password</button>
          </div>
        </form>
      </div>
    {/if}
  </div>
</div>

<style>
  .session {
    height: 100%;
    display: grid;
    grid-template-rows: auto 1fr;
    grid-template-columns: minmax(0, 1fr);
    border: 0 solid var(--env-color, transparent);
  }

  /* The environment's colour frames the session; production the most. */
  .session[data-environment="lab"] {
    --env-color: var(--env-lab);
    border-width: 1px;
  }

  .session[data-environment="staging"] {
    --env-color: var(--env-staging);
    border-width: 1px;
  }

  .session[data-environment="production"] {
    --env-color: var(--env-production);
    border-width: 3px;
  }

  .session[data-environment="production"] header {
    background: var(--env-production-bg);
  }

  .overlay {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: var(--space-6);
    background: var(--backdrop);
    overflow: auto;
  }

  /* The login banner stays readable above the card, and the terminal
     behind scrolls (#134). */
  .overlay.below-banner {
    align-items: end;
    background: transparent;
    pointer-events: none;
  }

  .overlay.below-banner > * {
    pointer-events: auto;
  }

  .card {
    width: min(22rem, 100%);
    display: grid;
    gap: var(--space-3);
    padding: var(--space-4);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
  }

  .card h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
  }

  .card p {
    margin: 0;
    font-size: var(--text-sm);
    overflow-wrap: anywhere;
  }

  .card p::first-letter {
    text-transform: uppercase;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--space-2);
  }

  /* One quiet line, whatever markers it shows: they never wrap it. */
  header {
    box-sizing: border-box;
    height: 30px;
    display: flex;
    flex-wrap: nowrap;
    align-items: center;
    gap: var(--space-2);
    padding: 0 var(--space-1) 0 var(--space-3);
    overflow: hidden;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
    font-size: var(--text-xs);
    white-space: nowrap;
  }

  header.has-role {
    box-shadow: inset 3px 0 var(--role-color);
  }

  /* A multi-send member: a line under the header (no extra height). */
  .session.grouped header {
    box-shadow: inset 0 -2px var(--multi-send);
  }

  .session.grouped header.has-role {
    box-shadow:
      inset 3px 0 var(--role-color),
      inset 0 -2px var(--multi-send);
  }

  .role {
    flex: none;
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    color: var(--role-color);
  }

  /* The role's own colour is the workspace's choice and may not stand out
     from the header: the icon carries it, the name stays readable. */
  .role-name {
    color: var(--muted);
  }

  .title {
    flex: 0 1 auto;
    min-width: 3rem;
    overflow: hidden;
    text-overflow: ellipsis;
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
    color: var(--text);
  }

  .detail,
  .protocol {
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--muted);
  }

  .protocol {
    flex: none;
  }

  .marker {
    flex: none;
    padding: 0 var(--space-1-5);
    line-height: 18px;
    border: 1px solid currentColor;
    border-radius: var(--radius-full);
    cursor: help;
  }

  .environment {
    border-color: var(--env-color);
    color: var(--env-color);
  }

  .session[data-environment="production"] .environment {
    color: var(--on-env-production);
    background: var(--env-production);
    font-weight: var(--weight-bold);
  }

  .unencrypted {
    color: var(--danger);
  }

  .legacy,
  .log-off,
  .backup.failed {
    color: var(--warning);
  }

  .multi-send {
    border-color: var(--multi-send);
    background: var(--multi-send-bg);
    color: var(--text);
    font-weight: var(--weight-semibold);
  }

  .multi-send .not-receiving {
    color: var(--warning);
  }

  .backup {
    color: var(--muted);
    border-color: var(--border-strong);
  }

  .backup-failed {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-0-5) var(--space-2);
    padding: var(--space-1) var(--space-2);
    background: var(--surface);
    border-top: 1px solid var(--border);
    color: var(--muted);
    font-size: var(--text-xs);
  }

  .backup-failed .reason {
    flex: 1 1 14rem;
    overflow-wrap: anywhere;
  }

  .status {
    /* Takes the room left; cut short before anything else is. */
    flex: 1 1 0;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--muted);
    text-align: right;
  }

  .status::before {
    content: "";
    display: inline-block;
    width: 8px;
    height: 8px;
    margin-right: var(--space-1-5);
    border-radius: 50%;
    box-sizing: border-box;
    border: 1.5px solid var(--state-connecting);
  }

  .status.connected::before {
    border: 0;
    background: var(--state-connected);
  }

  .status.closed::before {
    border: 0;
    background: var(--state-disconnected);
  }

  .menu-button {
    flex: none;
    display: inline-grid;
    place-items: center;
    width: var(--control-height-small);
    height: var(--control-height-small);
    min-height: 0;
    padding: 0;
    border-color: transparent;
    background: none;
    color: var(--muted);
  }

  .menu-button:hover,
  .menu-button[aria-expanded="true"] {
    color: var(--text);
    background: var(--surface-raised);
  }

  .body {
    position: relative;
    min-height: 0;
    display: grid;
    grid-template-rows: minmax(0, 1fr) auto;
  }

  .ended {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-0-5) var(--space-2);
    padding: var(--space-1-5) var(--space-2);
    background: var(--surface);
    border-top: 1px solid var(--danger-fill);
    font-size: var(--text-sm);
  }

  .log-failed {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-0-5) var(--space-2);
    padding: var(--space-1-5) var(--space-2);
    background: var(--surface);
    border-top: 1px solid var(--warning);
    color: var(--warning);
    font-size: var(--text-sm);
  }

  .log-failed .reason {
    flex: 1 1 14rem;
    overflow-wrap: anywhere;
  }

  .ended button {
    margin-left: auto;
    min-height: var(--control-height-small);
    padding: 0 var(--space-2);
    font-size: var(--text-xs);
  }

  .log-failed button,
  .backup-failed button {
    min-height: var(--control-height-small);
    padding: 0 var(--space-2);
    font-size: var(--text-xs);
  }

  .log-failed .reason::first-letter,
  .ended .reason::first-letter {
    text-transform: uppercase;
  }

  .search {
    position: absolute;
    top: 0.4rem;
    right: 1.2rem;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: var(--space-1-5);
    max-width: calc(100% - 2rem);
    padding: var(--space-1) var(--space-1-5);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow-small);
    font-size: var(--text-xs);
  }

  .search input[name="search"] {
    width: 12rem;
    min-width: 4rem;
    flex: 1 1 auto;
  }

  .search .results {
    min-width: 4.5rem;
    color: var(--muted);
    white-space: nowrap;
  }

  .search label {
    display: inline-flex;
    align-items: center;
    gap: var(--space-0-5);
    white-space: nowrap;
    font-family: var(--font-mono);
  }

  .search button {
    min-width: var(--control-height-small);
    padding: 0 var(--space-1-5);
  }

  .terminal {
    min-height: 0;
    min-width: 0;
    padding: var(--space-1) 0 0 var(--space-1-5);
    background: var(--terminal);
    overflow: hidden;
    /* xterm's layers carry z-indexes (the WebGL addon's canvases, inline);
       kept inside the terminal, so that the questions drawn over it get
       the clicks. */
    isolation: isolate;
  }
</style>
