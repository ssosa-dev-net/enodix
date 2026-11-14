<script lang="ts">
  import { FitAddon } from "@xterm/addon-fit";
  import { Terminal } from "@xterm/xterm";
  import { onMount } from "svelte";
  import HostKeyCheck from "./HostKeyCheck.svelte";
  import LoginQuestions from "./LoginQuestions.svelte";
  import RoleIcon from "./RoleIcon.svelte";
  import {
    openSession,
    type ConnectTarget,
    type HostKeyDecision,
    type HostKeyQuestion,
    type LoginPrompt,
    type Session,
    type SessionState,
  } from "./session";
  import type { Role } from "./workspace";

  interface Props {
    target: ConnectTarget;
    /** Shown in the header, such as `user@host:port`. */
    title: string;
    /** The role of the session's host, if it has one. */
    role: Role | null;
    /** Called when the user leaves this session's view. */
    ondismiss: () => void;
    /**
     * Called when the user asks to log in with a password after the SSH
     * agent did not work; without it, that is not offered.
     */
    onpasswordlogin?: (reason: string) => void;
  }

  let { target, title, role, ondismiss, onpasswordlogin }: Props = $props();

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

    terminal.onData((text) => session?.write(encoder.encode(text)));
    // Binary input (some mouse reports) arrives as one character per byte.
    terminal.onBinary((text) =>
      session?.write(Uint8Array.from(text, (char) => char.charCodeAt(0))),
    );
    terminal.onResize((newSize) => session?.resize(newSize));
    const resizeObserver = new ResizeObserver(() => fit.fit());
    resizeObserver.observe(container);

    const requestedSize = size();
    openSession(
      target,
      requestedSize,
      // Acknowledged once xterm.js has parsed it, so a flood of output
      // never queues up faster than the terminal can show it.
      (bytes, consumed) => terminal.write(bytes, consumed),
      (state) => {
        status = state;
        if (state.state !== "connecting") {
          prompt = null;
          hostKey = null;
        }
      },
      (asked) => (prompt = { prompt: asked, key: ++prompts }),
      (question) => (hostKey = { question, key: ++prompts }),
    ).then(
      (opened) => {
        if (disposed) {
          opened.close();
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
        terminal.focus();
      },
      (error) =>
        (status = {
          state: "closed",
          reason: String(error),
          offersPasswordLogin: false,
        }),
    );

    return () => {
      disposed = true;
      resizeObserver.disconnect();
      session?.close();
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
    <span class="status {status.state}" role="status">{statusText}</span>
    {#if status.state === "closed"}
      <button onclick={ondismiss}>New connection</button>
    {:else}
      <button onclick={() => session?.close()} disabled={!session}>
        Disconnect
      </button>
    {/if}
  </header>
  <div class="body">
    <div class="terminal" bind:this={container}></div>
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
    align-items: center;
    gap: 0.75rem;
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

  .status {
    flex: 1;
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
  }

  .terminal {
    min-height: 0;
    min-width: 0;
    padding: 0.3rem 0 0 0.4rem;
    background: var(--terminal);
    overflow: hidden;
  }
</style>
