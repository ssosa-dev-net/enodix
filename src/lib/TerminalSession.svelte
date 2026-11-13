<script lang="ts">
  import { FitAddon } from "@xterm/addon-fit";
  import { Terminal } from "@xterm/xterm";
  import { onMount } from "svelte";
  import {
    openSession,
    type QuickConnectTarget,
    type Session,
    type SessionState,
  } from "./session";

  interface Props {
    target: QuickConnectTarget;
    /** Called when the user leaves this session's view. */
    ondismiss: () => void;
  }

  let { target, ondismiss }: Props = $props();

  let container: HTMLDivElement;
  let session = $state<Session | null>(null);
  let status = $state<SessionState>({ state: "connecting" });

  const title = $derived(`${target.username}@${target.host}:${target.port}`);
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
      (state) => (status = state),
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
      (error) => (status = { state: "closed", reason: String(error) }),
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
  <header>
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
  <div class="terminal" bind:this={container}></div>
</div>

<style>
  .session {
    height: 100%;
    display: grid;
    grid-template-rows: auto 1fr;
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

  .terminal {
    min-height: 0;
    min-width: 0;
    padding: 0.3rem 0 0 0.4rem;
    background: var(--terminal);
    overflow: hidden;
  }
</style>
