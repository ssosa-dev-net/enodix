<script lang="ts">
  // A snippet's form, drawn in the Snippets side view in place of the list
  // (#79: no longer a window-modal dialog, as the Credentials view's forms).
  // Cancel (or Escape, handled by the view) goes back to the list; saving
  // goes through the core.
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

<form
  data-snippet-form
  data-snippet-editing={previous ?? ""}
  aria-labelledby="snippet-title"
  onsubmit={(event) => {
    event.preventDefault();
    void save();
  }}
>
  <h3 id="snippet-title">
    {previous === null ? "New snippet" : `Edit ${previous}`}
  </h3>
  {#if error}
    <p class="error" role="alert" data-snippet-error>{error}</p>
  {/if}
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
  <p class="hint">
    Each line is sent as a line of its own. Host fields fill variables:
    <code>{"{{name}}"}</code>, <code>{"{{address}}"}</code>,
    <code>{"{{site}}"}</code>, <code>{"{{custom.circuit}}"}</code> and so on.
    You are asked for any other <code>{"{{variable}}"}</code>, and for fields
    the host leaves empty, when the snippet is sent. Snippets are kept in the
    workspace (<code>_snippets.yaml</code>).
  </p>
  <div class="actions">
    <button type="button" data-snippet-cancel onclick={onclose}>Cancel</button>
    <button type="submit" data-snippet-save>Save</button>
  </div>
</form>

<style>
  form {
    display: grid;
    align-content: start;
    gap: var(--space-1-5);
    padding: var(--space-3);
    font-size: var(--text-sm);
  }

  h3 {
    margin: 0 0 var(--space-1);
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }

  label {
    margin-top: var(--space-1);
    color: var(--muted);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
  }

  textarea {
    resize: vertical;
    font-family: var(--font-mono);
    font-size: var(--text-sm);
  }

  .hint {
    margin: var(--space-1) 0 0;
    color: var(--muted);
    font-size: var(--text-xs);
    line-height: 1.5;
  }

  .error {
    margin: 0;
    color: var(--danger);
    font-size: var(--text-sm);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
    margin-top: var(--space-1);
  }
</style>
