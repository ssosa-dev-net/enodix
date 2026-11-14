<script lang="ts">
  import { FitAddon } from "@xterm/addon-fit";
  import { Terminal } from "@xterm/xterm";
  import { onMount } from "svelte";
  import HostKeyCheck from "./HostKeyCheck.svelte";
  import LoginQuestions from "./LoginQuestions.svelte";
  import RoleIcon from "./RoleIcon.svelte";
  import NegotiationFailure from "./NegotiationFailure.svelte";
  import {
    openSession,
    type HostKeyDecision,
    type HostKeyQuestion,
    type LoginPrompt,
    type NegotiatedAlgorithms,
    type Session,
    type SessionSource,
    type SessionState,
  } from "./session";
  import type { Role } from "./workspace";

  interface Props {
    source: SessionSource;
    /** Shown in the header, such as `user@host:port`. */
    title: string;
    /** The role of the session's host, if it has one. */
    role: Role | null;
    /** Whether this session is the one on screen. */
    shown: boolean;
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
    title,
    role,
    shown,
    ondismiss,
    onduplicate,
    onpasswordlogin,
  }: Props = $props();

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

  // The terminal takes the keys when it comes on screen and when the
  // session closes (Enter reconnects), unless a card over it takes them.
  $effect(() => {
    if (shown && !prompt && !hostKey && !closedCard) xterm?.focus();
  });

  onMount(() => {
    const terminal = new Terminal({
      cursorBlink: true,
      fontFamily: '"Cascadia Mono", Consolas, "Courier New", monospace',
      fontSize: 14,
      scrollback: 10000,
      theme: { background: "#0e1116", foreground: "#d7dce2" },
    });
    xterm = terminal;
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(container);
    fit.fit();

    let disposed = false;
    const encoder = new TextEncoder();
    const size = () => ({ cols: terminal.cols, rows: terminal.rows });
    currentSize = size;
    markReconnect = () =>
      terminal.write("\r\n\x1b[2m--- reconnecting ---\x1b[0m\r\n");

    terminal.onData((text) => {
      // A closed session keeps its scrollback; Enter reconnects it.
      if (status.state === "closed") {
        if (text === "\r") reconnect();
        return;
      }
      session?.write(encoder.encode(text));
    });
    // Binary input (some mouse reports) arrives as one character per byte.
    terminal.onBinary((text) =>
      session?.write(Uint8Array.from(text, (char) => char.charCodeAt(0))),
    );
    terminal.onResize((newSize) => session?.resize(newSize));
    const resizeObserver = new ResizeObserver(() => fit.fit());
    resizeObserver.observe(container);

    const requestedSize = size();
    openSession(source, requestedSize, {
      // Acknowledged once xterm.js has parsed it, so a flood of output
      // never queues up faster than the terminal can show it.
      onOutput: (bytes, consumed) => terminal.write(bytes, consumed),
      onState: (state) => {
        status = state;
        if (state.state !== "connecting") {
          prompt = null;
          hostKey = null;
        }
      },
      onPrompt: (asked) => (prompt = { prompt: asked, key: ++prompts }),
      onHostKey: (question) => (hostKey = { question, key: ++prompts }),
      onAlgorithms: (agreed) => (algorithms = agreed),
    }).then(
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
        if (shown) terminal.focus();
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
      resizeObserver.disconnect();
      // Gone with its terminal: it can no longer be reconnected.
      session?.discard();
      terminal.dispose();
    };
  });
</script>

<div class="session">
  <header class:has-role={role} style:--role-color={role?.color}>
    {#if role}
      <span class="role" data-session-role={role.id}>
        <RoleIcon {role} />
        {role.name}
      </span>
    {/if}
    <span class="title">{title}</span>
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
  <div class="body">
    <div class="terminal" bind:this={container}></div>
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

  .terminal {
    min-height: 0;
    min-width: 0;
    padding: 0.3rem 0 0 0.4rem;
    background: var(--terminal);
    overflow: hidden;
  }
</style>
