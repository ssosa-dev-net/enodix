<script lang="ts">
  import { onMount } from "svelte";
  import { ENVIRONMENT_NAMES } from "./paste";
  import type { Environment } from "./workspace";

  interface Props {
    /** The text as it would be sent. */
    text: string;
    lineCount: number;
    /** The session's name, such as its host's. */
    target: string;
    environment: Environment | null;
    /** Milliseconds between lines; 0: none. */
    lineDelayMs: number;
    onsend: () => void;
    oncancel: () => void;
  }

  let {
    text,
    lineCount,
    target,
    environment,
    lineDelayMs,
    onsend,
    oncancel,
  }: Props = $props();

  let cancel: HTMLButtonElement;
  let send: HTMLButtonElement;

  // Taken from the terminal, which would otherwise keep the keys. On a
  // production host Enter by reflex cancels.
  onMount(() => (environment === "production" ? cancel : send).focus());

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      oncancel();
    }
  }
</script>

<div
  class="overlay"
  role="alertdialog"
  aria-modal="true"
  aria-labelledby="paste-confirm-title"
  tabindex="-1"
  {onkeydown}
>
  <form
    class="card"
    class:production={environment === "production"}
    data-paste-confirm={environment ?? "none"}
    onsubmit={(event) => {
      event.preventDefault();
      onsend();
    }}
  >
    <h2 id="paste-confirm-title">
      Paste <span data-paste-lines={lineCount}
        >{lineCount} {lineCount === 1 ? "line" : "lines"}</span
      >
      into {target}?
    </h2>
    {#if environment === "production"}
      <p class="warning" data-paste-production>
        This is a {ENVIRONMENT_NAMES[environment].toLowerCase()} device. Check that
        it is the right one.
      </p>
    {/if}
    <pre data-paste-text>{text}</pre>
    {#if lineDelayMs > 0}
      <p class="muted">Sent one line at a time, {lineDelayMs} ms apart.</p>
    {/if}
    <div class="actions">
      <button
        type="button"
        data-paste="cancel"
        bind:this={cancel}
        onclick={oncancel}>Cancel</button
      >
      <button type="submit" data-paste="send" bind:this={send}>Paste</button>
    </div>
  </form>
</div>

<style>
  .overlay {
    position: absolute;
    inset: 0;
    z-index: 3;
    display: grid;
    place-items: center;
    padding: 1rem;
    background: rgb(0 0 0 / 0.45);
    overflow: auto;
  }

  .card {
    width: min(36rem, 100%);
    max-height: 100%;
    display: grid;
    grid-template-rows: auto auto minmax(0, 1fr) auto auto;
    gap: 0.7rem;
    padding: 1.1rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .card.production {
    border: 2px solid var(--env-production);
  }

  h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  p {
    margin: 0;
    font-size: 0.85rem;
  }

  .warning {
    color: var(--env-production);
    font-weight: 600;
  }

  .muted {
    color: var(--muted);
  }

  pre {
    margin: 0;
    min-height: 2.5rem;
    max-height: 16rem;
    overflow: auto;
    padding: 0.5rem 0.6rem;
    background: var(--terminal);
    border: 1px solid var(--border);
    border-radius: 4px;
    font-family: "Cascadia Mono", Consolas, monospace;
    font-size: 0.8rem;
    white-space: pre;
    tab-size: 8;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.6rem;
  }
</style>
