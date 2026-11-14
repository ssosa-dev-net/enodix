<script lang="ts">
  import { FitAddon } from "@xterm/addon-fit";
  import { SearchAddon, type ISearchOptions } from "@xterm/addon-search";
  import { Terminal } from "@xterm/xterm";
  import { onMount, tick, untrack } from "svelte";
  import { webglRenderer } from "./renderer";
  import { terminalInput, type TerminalInput } from "./terminalInput";
  import { highlighting, highlightTerminal } from "./highlight.svelte";
  import HostKeyCheck from "./HostKeyCheck.svelte";
  import LoginQuestions from "./LoginQuestions.svelte";
  import RoleIcon from "./RoleIcon.svelte";
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
    openSession,
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
    /** Adds this session's tab to the multi-send group, or takes it out. */
    ontogglegroup: () => void;
    /** Ends the multi-send group. */
    onendgroup: () => void;
    /**
     * Takes a paste (or snippet text) made here while the tab is in the
     * multi-send group: the group's paste is planned and confirmed once for
     * every member, then sent to each with `pasteLines` (#36).
     */
    ongrouppaste: (text: string) => void;
    /** Shown in the header, such as `user@host:port`. */
    title: string;
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
    onduplicate: (id: number) => void;
    /**
     * Called when the user asks to log in with a password after the SSH
     * agent did not work; without it, that is not offered.
     */
    onpasswordlogin?: (reason: string) => void;
  }

  let {
    source,
    tab,
    groupSize,
    ontogglegroup,
    onendgroup,
    ongrouppaste,
    title,
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
  }: Props = $props();

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
  }

  const statusText = $derived(
    status.state === "connecting"
      ? "Connecting…"
      : status.state === "connected"
        ? "Connected"
        : `Closed: ${status.reason}`,
  );

  /** The card over the terminal that takes the closed session's keys. */
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

  /** Opens the closed session again in this terminal (same target). */
  function reconnect() {
    if (!session || status.state !== "closed") return;
    const reconnecting = session;
    status = { state: "connecting" };
    prompt = null;
    hostKey = null;
    algorithms = [];
    logFailure = null;
    backup = null;
    backupNoticeDismissed = false;
    markReconnect();
    reconnecting.reconnect(currentSize()).catch(
      (error) =>
        (status = {
          state: "closed",
          reason: String(error),
          offersPasswordLogin: false,
          negotiation: null,
        }),
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
    if (event.key === "Escape") {
      event.preventDefault();
      closeSearch();
    } else if (event.key === "Enter" || event.key === "F3") {
      event.preventDefault();
      find(event.shiftKey ? "previous" : "next");
    }
  }

  // The terminal takes the keys when it comes on screen and when the
  // session closes (Enter reconnects), unless a card or the search bar
  // over it takes them.
  $effect(() => {
    void focusRequest;
    if (
      focused &&
      !prompt &&
      !hostKey &&
      !closedCard &&
      !searching &&
      !pasteQuestion
    )
      xterm?.focus();
  });

  $effect(() => onstate?.(status));

  /** Brings the terminal up to date and on screen; set up on mount. */
  let reveal: () => void = () => {};
  /** Keeps the terminal's output aside while it is out of sight. */
  let conceal: () => void = () => {};
  /** Sends text as pasted (set up on mount); see `sendText`. */
  let pasteText: (text: string) => void = () => {};
  /** Pastes lines into this session alone (set up on mount). */
  let sendLines: (
    lines: string[],
    lineDelayMs: number,
  ) => Promise<void> = async () => {};

  /**
   * Pastes `lines` (each with its line break) into this session only, as
   * one member's part of a multi-send group paste that was already planned
   * and confirmed: `lineDelayMs` apart, stopping if the session ends.
   * Resolves once the last line is sent (or the session ended).
   */
  export function pasteLines(
    lines: string[],
    lineDelayMs: number,
  ): Promise<void> {
    return sendLines(lines, lineDelayMs);
  }

  /**
   * Sends `text` to the session the way a paste is sent (a snippet's
   * lines, each ending in a line break). Only while connected; returns
   * whether it was sent.
   */
  export function sendText(text: string): boolean {
    if (!session || status.state !== "connected") return false;
    pasteText(text);
    return true;
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
    });
    xterm = terminal;
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    const search = new SearchAddon();
    terminal.loadAddon(search);
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
      const dark = isDark();
      const options: ISearchOptions = {
        caseSensitive,
        regex,
        incremental,
        decorations: {
          matchBackground: dark ? "#5c4a00" : "#fff1a8",
          activeMatchBackground: dark ? "#b36b00" : "#ffb340",
          matchOverviewRuler: "#d29922",
          activeMatchColorOverviewRuler: "#ff8800",
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
    /** Sends the lines; `alone`: to this session only, not its group. */
    const pasteEach = async (
      lines: string[],
      lineDelayMs: number,
      alone: boolean,
    ) => {
      for (const [index, line] of lines.entries()) {
        if (index > 0 && lineDelayMs > 0) await wait(lineDelayMs);
        // A session that ended (or a terminal gone) takes no more lines.
        if (ended()) return;
        pastingAlone = alone;
        try {
          input?.paste(line);
        } finally {
          pastingAlone = false;
        }
      }
    };
    let pasting: Promise<void> = Promise.resolve();
    pasteText = (text: string) => {
      if (text === "") return;
      // A member's paste is the group's: planned once for every member.
      if (untrack(() => groupSize) !== null) {
        if (!ended()) ongrouppaste(text);
        return;
      }
      pasting = pasting.then(() => sendPaste(text)).catch(() => {});
    };
    sendLines = (lines, lineDelayMs) => {
      const sent = pasting.then(() => pasteEach(lines, lineDelayMs, true));
      pasting = sent.catch(() => {});
      return pasting;
    };
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
    /** Set while a group paste's line goes to this session alone. */
    let pastingAlone = false;
    const encoder = new TextEncoder();
    const size = () => ({ cols: terminal.cols, rows: terminal.rows });
    currentSize = size;
    markReconnect = () =>
      terminal.write("\r\n\x1b[2m--- reconnecting ---\x1b[0m\r\n");

    // Only typed and pasted input is the user's own (#34): it goes through
    // the shell's multi-send routing. The terminal's replies and mouse
    // reports depend on this terminal's screen and go to this session only.
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
      const shared =
        origin === "typed" || (origin === "pasted" && !pastingAlone);
      if (shared && status.state === "connected") session?.input(bytes);
      else session?.write(bytes);
    });
    // For the smoke test, which checks where input came from.
    Object.assign(container, { input });
    terminal.onResize((newSize) => session?.resize(newSize));
    const resizeObserver = new ResizeObserver(() => {
      if (!catchingUp) fit.fit();
    });
    resizeObserver.observe(container);

    const requestedSize = size();
    openSession(
      source,
      untrack(() => tab),
      requestedSize,
      {
        // Acknowledged once xterm.js has parsed it, so a flood of output
        // never queues up faster than the terminal can show it.
        onOutput: deliver,
        onState: (state) => {
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
        // The window may have been resized while the session was opening.
        const current = size();
        if (
          current.cols !== requestedSize.cols ||
          current.rows !== requestedSize.rows
        ) {
          opened.resize(current);
        }
        if (untrack(() => focused)) terminal.focus();
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
      // Gone with its terminal: it can no longer be reconnected.
      session?.discard();
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
    if (untrack(() => searching)) untrack(() => find("next", true));
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
  <header class:has-role={role} style:--role-color={role?.color}>
    {#if role}
      <span class="role" data-session-role={role.id}>
        <RoleIcon {role} />
        {role.name}
      </span>
    {/if}
    <span class="title">{title}</span>
    {#if safety.environment}
      <span
        class="environment"
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
        class="unencrypted"
        data-unencrypted
        title="Everything this session sends, passwords included, crosses the network unencrypted (telnet)"
      >
        Unencrypted
      </span>
    {/if}
    {#if backup}
      <span
        class="backup {backup.kind}"
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
    {#if status.state === "connected" && legacy.length > 0}
      <span
        class="legacy"
        data-legacy-crypto={legacy.map((a) => a.legacy.join(",")).join(";")}
        title={`Outdated algorithms in use:\n${legacyTitle}`}
      >
        Legacy crypto
      </span>
    {/if}
    <span class="status {status.state}" role="status" title={statusText}
      >{statusText}</span
    >
    {#if session?.transport === "serial" && status.state === "connected"}
      <button
        data-send-break
        onclick={() => {
          session?.sendBreak();
          xterm?.focus();
        }}
        title="Send a break signal on the line (to interrupt a device's boot)"
      >
        Send break
      </button>
    {/if}
    <button
      data-multi-send-toggle={groupSize === null ? "join" : "leave"}
      aria-pressed={groupSize !== null}
      onclick={() => {
        ontogglegroup();
        xterm?.focus();
      }}
      title={groupSize === null
        ? "Add this session to the multi-send group: what you type in a member reaches every member (Ctrl+Shift+M)"
        : "Take this session out of the multi-send group (Ctrl+Shift+M)"}
    >
      {groupSize === null ? "Multi-send" : "Leave group"}
    </button>
    <button
      onclick={() => session && onduplicate(session.id)}
      disabled={!session}
      title="Open a second session to the same host"
    >
      Duplicate
    </button>
    {#if status.state === "closed"}
      <button onclick={reconnect} disabled={!session}>Reconnect</button>
      <button onclick={ondismiss}>New connection</button>
    {:else}
      <button onclick={() => session?.close()} disabled={!session}>
        Disconnect
      </button>
    {/if}
  </header>
  {#if groupSize !== null}
    <div class="multi-send" data-multi-send-bar data-group-size={groupSize}>
      <span class="what">
        {groupSize === 1
          ? "Multi-send group: no other member yet"
          : `Multi-send: typing reaches all ${groupSize} sessions of the group`}
      </span>
      {#if status.state !== "connected"}
        <span class="not-receiving" data-not-receiving
          >Not receiving while {status.state === "closed"
            ? "disconnected"
            : "connecting"}</span
        >
      {/if}
      <button
        data-end-group
        onclick={() => {
          onendgroup();
          xterm?.focus();
        }}>End group</button
      >
    </div>
  {/if}
  <div class="body">
    <div class="terminal" bind:this={container}></div>
    {#if searching}
      <form
        class="search"
        role="search"
        data-terminal-search
        onsubmit={(event) => {
          event.preventDefault();
          find("next");
        }}
      >
        <input
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
          data-search="previous"
          title="Previous match (Shift+Enter)"
          aria-label="Previous match"
          onclick={() => find("previous")}>&uarr;</button
        >
        <button
          type="button"
          data-search="next"
          title="Next match (Enter)"
          aria-label="Next match"
          onclick={() => find("next")}>&darr;</button
        >
        <button
          type="button"
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
    {#if status.state === "closed" && session}
      <div class="ended" role="alert" data-session-ended>
        <span class="reason">{status.reason}.</span>
        <span>Press <kbd>Enter</kbd> to reconnect.</span>
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
      <div class="overlay">
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

  .environment {
    padding: 0.05rem 0.45rem;
    border: 1px solid var(--env-color);
    border-radius: 999px;
    color: var(--env-color);
    font-size: 0.75rem;
    white-space: nowrap;
    cursor: help;
  }

  .session.grouped {
    grid-template-rows: auto auto 1fr;
  }

  /* A multi-send member: what is typed here reaches the whole group. */
  .multi-send {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    padding: 0.2rem 0.6rem;
    border-bottom: 2px solid var(--multi-send);
    background: var(--multi-send-bg);
    color: var(--text);
    font-size: 0.8rem;
  }

  .multi-send .what {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }

  .multi-send .not-receiving {
    color: var(--warning);
    white-space: nowrap;
  }

  .session[data-environment="production"] .environment {
    color: #fff;
    background: var(--env-production);
    font-weight: 700;
  }

  .overlay {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: 1.5rem;
    background: rgb(0 0 0 / 0.45);
    overflow: auto;
  }

  .card {
    width: min(22rem, 100%);
    display: grid;
    gap: 0.8rem;
    padding: 1.2rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .card h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
  }

  .card p {
    margin: 0;
    font-size: 0.85rem;
    overflow-wrap: anywhere;
  }

  .card p::first-letter {
    text-transform: uppercase;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 0.6rem;
  }

  header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.4rem 0.75rem;
    padding: 0.4rem 0.6rem;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
    font-size: 0.85rem;
  }

  header.has-role {
    box-shadow: inset 3px 0 var(--role-color);
  }

  .role {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    color: var(--role-color);
    font-size: 0.8rem;
    white-space: nowrap;
  }

  .title {
    font-weight: 600;
  }

  .unencrypted {
    padding: 0.05rem 0.45rem;
    border: 1px solid var(--danger);
    border-radius: 999px;
    color: var(--danger);
    font-size: 0.75rem;
    white-space: nowrap;
    cursor: help;
  }

  .backup {
    color: var(--muted);
    font-size: 0.75rem;
    white-space: nowrap;
    cursor: help;
  }

  .backup.failed {
    color: var(--warning);
  }

  .backup-failed {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.2rem 0.6rem;
    padding: 0.25rem 0.6rem;
    background: var(--surface);
    border-top: 1px solid var(--border);
    color: var(--muted);
    font-size: 0.8rem;
  }

  .backup-failed .reason {
    flex: 1 1 14rem;
    overflow-wrap: anywhere;
  }

  .legacy {
    padding: 0.05rem 0.45rem;
    border: 1px solid var(--warning);
    border-radius: 999px;
    color: var(--warning);
    font-size: 0.75rem;
    white-space: nowrap;
    cursor: help;
  }

  .status {
    /* Room for the reason; the buttons wrap below when it lacks it. */
    flex: 1 1 14rem;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted);
  }

  .status::before {
    content: "";
    display: inline-block;
    width: 0.55rem;
    height: 0.55rem;
    margin-right: 0.4rem;
    border-radius: 50%;
    background: var(--muted);
  }

  .status.connected::before {
    background: var(--ok);
  }

  .status.closed::before {
    background: var(--danger);
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
    gap: 0.2rem 0.6rem;
    padding: 0.35rem 0.6rem;
    background: var(--surface);
    border-top: 1px solid var(--danger);
    font-size: 0.85rem;
  }

  .log-failed {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 0.2rem 0.6rem;
    padding: 0.35rem 0.6rem;
    background: var(--surface);
    border-top: 1px solid var(--warning);
    color: var(--warning);
    font-size: 0.85rem;
  }

  .log-failed .reason {
    flex: 1 1 14rem;
    overflow-wrap: anywhere;
  }

  .log-failed .reason::first-letter,
  .ended .reason::first-letter {
    text-transform: uppercase;
  }

  kbd {
    padding: 0 0.3rem;
    border: 1px solid var(--border);
    border-radius: 3px;
    font-family: inherit;
    font-size: 0.8rem;
  }

  .search {
    position: absolute;
    top: 0.4rem;
    right: 1.2rem;
    z-index: 2;
    display: flex;
    align-items: center;
    gap: 0.35rem;
    max-width: calc(100% - 2rem);
    padding: 0.3rem 0.4rem;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 4px;
    box-shadow: 0 4px 14px rgb(0 0 0 / 0.3);
    font-size: 0.8rem;
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
    gap: 0.15rem;
    white-space: nowrap;
    font-family: monospace;
  }

  .search button {
    padding: 0.15rem 0.45rem;
  }

  .terminal {
    min-height: 0;
    min-width: 0;
    padding: 0.3rem 0 0 0.4rem;
    background: var(--terminal);
    overflow: hidden;
  }
</style>
