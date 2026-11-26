<script lang="ts">
  import { onMount } from "svelte";
  import { ENVIRONMENT_NAMES } from "./paste";
  import type { Environment } from "./workspace";

  interface Props {
    /** The snippet's name. */
    snippet: string;
    /** What its steps will do, one line each (the core's words). */
    preview: string[];
    /** How many steps there are. */
    count: number;
    /** The session's name, such as its host's. */
    target: string;
    environment: Environment | null;
    onrun: () => void;
    oncancel: () => void;
  }

  let { snippet, preview, count, target, environment, onrun, oncancel }: Props =
    $props();

  let cancel: HTMLButtonElement;
  let run: HTMLButtonElement;

  // Taken from the terminal; on a production host Enter by reflex cancels.
  onMount(() => (environment === "production" ? cancel : run).focus());

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
  aria-labelledby="steps-confirm-title"
  tabindex="-1"
  {onkeydown}
>
  <form
    class="card"
    class:production={environment === "production"}
    data-steps-confirm={environment ?? "none"}
    data-steps-count={count}
    onsubmit={(event) => {
      event.preventDefault();
      onrun();
    }}
  >
    <h2 id="steps-confirm-title">
      Run {snippet} ({count}
      {count === 1 ? "step" : "steps"}) on {target}?
    </h2>
    {#if environment === "production"}
      <p class="warning" data-steps-production>
        This is a {ENVIRONMENT_NAMES[environment].toLowerCase()} device. Check that
        it is the right one.
      </p>
    {/if}
    <ol class="preview" data-steps-preview>
      {#each preview as line, index (index)}
        <li>{line}</li>
      {/each}
    </ol>
    <p class="muted">
      Each line waits for the device as shown. Typing in the session, or Stop,
      ends the run; the key typed is not sent.
    </p>
    <div class="actions">
      <button
        type="button"
        data-steps="cancel"
        bind:this={cancel}
        onclick={oncancel}>Cancel</button
      >
      <button type="submit" data-steps="run" bind:this={run}>Run</button>
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
    padding: var(--space-4);
    background: var(--backdrop);
    overflow: auto;
  }

  .card {
    width: min(36rem, 100%);
    max-height: 100%;
    display: grid;
    grid-template-rows: auto auto minmax(0, 1fr) auto auto;
    gap: var(--space-2-5);
    padding: var(--space-4);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
  }

  .card.production {
    border: 2px solid var(--env-production);
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }

  p {
    margin: 0;
    font-size: var(--text-sm);
  }

  .warning {
    color: var(--env-production);
    font-weight: var(--weight-semibold);
  }

  .muted {
    color: var(--muted);
  }

  .preview {
    margin: 0;
    min-height: 2.5rem;
    max-height: 16rem;
    overflow: auto;
    padding: var(--space-2) var(--space-2);
    list-style: none;
    background: var(--terminal);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
