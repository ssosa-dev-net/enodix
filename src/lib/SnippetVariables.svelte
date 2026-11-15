<script lang="ts">
  import { untrack } from "svelte";

  interface Props {
    /** The snippet being sent. */
    snippet: string;
    /** The session it goes to. */
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
</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key === "Escape") oncancel();
  }}
/>

<div
  class="backdrop"
  role="dialog"
  aria-modal="true"
  aria-labelledby="snippet-variables-title"
  data-dialog="snippet-variables"
>
  <form
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
      <button type="button" onclick={oncancel}>Cancel</button>
      <button type="submit" data-snippet-variables-send>Send</button>
    </div>
  </form>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 15;
    display: grid;
    place-items: center;
    padding: var(--space-4);
    background: var(--backdrop);
  }

  form {
    width: min(26rem, 100%);
    max-height: 100%;
    overflow: auto;
    display: grid;
    gap: var(--space-3);
    padding: var(--space-4);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
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
