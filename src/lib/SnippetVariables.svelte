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
    padding: 1rem;
    background: rgb(0 0 0 / 0.45);
  }

  form {
    width: min(26rem, 100%);
    max-height: 100%;
    overflow: auto;
    display: grid;
    gap: 0.8rem;
    padding: 1.2rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.4);
  }

  h2 {
    margin: 0;
    font-size: 1.05rem;
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  p {
    margin: 0;
    color: var(--muted);
    font-size: 0.85rem;
  }

  .fields {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 0.5rem 0.8rem;
    font-size: 0.85rem;
  }

  label {
    font-family: "Cascadia Mono", Consolas, monospace;
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.6rem;
  }
</style>
