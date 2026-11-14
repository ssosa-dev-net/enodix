<script lang="ts">
  import { untrack } from "svelte";
  import { inventory, saveSnippet } from "./inventory.svelte";
  import type { Snippet } from "./workspace";

  interface Props {
    /** The name of the snippet edited; null for a new one. */
    previous: string | null;
    /** What the form starts with. */
    initial: Snippet;
    onclose: () => void;
  }

  let { previous, initial, onclose }: Props = $props();

  // Edited here; the form starts from `initial` as it was when opened.
  const snippet = $state(
    untrack(() => ({ ...initial, group: initial.group ?? "" })),
  );
  let error = $state<string | null>(null);
  let nameInput = $state<HTMLInputElement>();

  const groups = $derived([
    ...new Set(
      (inventory.workspace?.snippets ?? []).flatMap((s) =>
        s.group === null ? [] : [s.group],
      ),
    ),
  ]);

  // Take the keyboard from whatever had it (the terminal, say).
  $effect(() => {
    nameInput?.focus();
  });

  async function save() {
    try {
      await saveSnippet(previous, {
        name: snippet.name,
        group: snippet.group.trim() === "" ? null : snippet.group,
        text: snippet.text,
      });
      onclose();
    } catch (failure) {
      error = String(failure);
    }
  }
</script>

<svelte:window
  onkeydown={(event) => {
    if (event.key === "Escape") onclose();
  }}
/>

<div
  class="backdrop"
  role="dialog"
  aria-modal="true"
  aria-labelledby="snippet-title"
  data-dialog="snippet"
>
  <form
    data-snippet-form
    onsubmit={(event) => {
      event.preventDefault();
      void save();
    }}
  >
    <h2 id="snippet-title">
      {previous === null ? "New snippet" : "Edit snippet"}
    </h2>
    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}
    <div class="fields">
      <label for="snippet-name">Name</label>
      <input
        id="snippet-name"
        name="snippet-name"
        autocomplete="off"
        bind:this={nameInput}
        bind:value={snippet.name}
      />
      <label for="snippet-group">Group</label>
      <input
        id="snippet-group"
        name="snippet-group"
        autocomplete="off"
        placeholder="None"
        list="snippet-groups"
        bind:value={snippet.group}
      />
      <datalist id="snippet-groups">
        {#each groups as group (group)}
          <option value={group}></option>
        {/each}
      </datalist>
      <label for="snippet-text">Text</label>
      <textarea
        id="snippet-text"
        name="snippet-text"
        rows="8"
        spellcheck="false"
        placeholder={"show interface {{interface}}"}
        bind:value={snippet.text}></textarea>
    </div>
    <p class="hint">
      Each line is sent as a line of its own. Host fields fill variables:
      <code>{"{{name}}"}</code>, <code>{"{{address}}"}</code>,
      <code>{"{{site}}"}</code>, <code>{"{{custom.circuit}}"}</code> and so on.
      You are asked for any other <code>{"{{variable}}"}</code>, and for fields
      the host leaves empty, when the snippet is sent. Snippets are kept in the
      workspace (<code>_snippets.yaml</code>).
    </p>
    <div class="actions">
      <button type="button" onclick={onclose}>Cancel</button>
      <button type="submit" data-snippet-save>Save</button>
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
    width: min(36rem, 100%);
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
  }

  .fields {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 0.5rem 0.8rem;
    font-size: 0.85rem;
  }

  label[for="snippet-text"] {
    align-self: start;
    padding-top: 0.3rem;
  }

  textarea {
    resize: vertical;
    font-family: "Cascadia Mono", Consolas, monospace;
    font-size: 0.85rem;
  }

  .hint {
    margin: 0;
    color: var(--muted);
    font-size: 0.8rem;
  }

  .error {
    margin: 0;
    color: var(--danger);
    font-size: 0.85rem;
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.6rem;
  }
</style>
