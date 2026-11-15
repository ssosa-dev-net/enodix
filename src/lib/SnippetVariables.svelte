<script lang="ts">
  // The values a snippet's variables lack, asked before it is sent. It
  // belongs to one terminal, so it covers only that session's pane (#79,
  // as the paste confirmation since #74): drawn inside the session's slot;
  // other panes stay usable. Escape is handled here, not on the window, so
  // Escape typed in another terminal reaches its device.
  import { untrack } from "svelte";

  interface Props {
    /** The snippet being sent. */
    snippet: string;
    /** The session it goes to (or the multi-send group). */
    target: string;
    /** The variables without a value, in the order the snippet uses them. */
    names: string[];
    onsend: (answers: Record<string, string>) => void;
    oncancel: () => void;
  }

  let { snippet, target, names, onsend, oncancel }: Props = $props();

  const values = $state(untrack(() => names.map(() => "")));
  let fields = $state<HTMLElement>();

  // Take the keyboard from the terminal.
  $effect(() => {
    fields?.querySelector("input")?.focus();
  });

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
  role="dialog"
  aria-labelledby="snippet-variables-title"
  tabindex="-1"
  data-snippet-variables={snippet}
  {onkeydown}
>
  <form
    class="card"
    onsubmit={(event) => {
      event.preventDefault();
      onsend(Object.fromEntries(names.map((name, i) => [name, values[i]])));
    }}
  >
    <h2 id="snippet-variables-title">Send {snippet}</h2>
    <p>To {target}. The host has no value for these:</p>
    <div class="fields" bind:this={fields}>
      {#each names as name, index (name)}
        <label for="snippet-variable-{index}">{name}</label>
        <input
          id="snippet-variable-{index}"
          name="variable-{index}"
          data-variable={name}
          autocomplete="off"
          spellcheck="false"
          bind:value={values[index]}
        />
      {/each}
    </div>
    <div class="actions">
      <button type="button" data-snippet-variables-cancel onclick={oncancel}
        >Cancel</button
      >
      <button type="submit" data-snippet-variables-send>Send</button>
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
    width: min(26rem, 100%);
    max-height: 100%;
    overflow: auto;
    display: grid;
    gap: var(--space-3);
    padding: var(--space-4);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }

  p {
    margin: 0;
    color: var(--muted);
    font-size: var(--text-sm);
  }

  .fields {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: var(--space-2) var(--space-3);
    font-size: var(--text-sm);
  }

  label {
    font-family: var(--font-mono);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
