<script lang="ts">
  // Unsaved edits in the host editor or a snippet editor (#10), before they
  // would be left: closing the tab, showing another host, quitting or
  // opening another workspace. Save writes them, Discard drops them, Cancel stays.
  import { onMount } from "svelte";
  import { unsaved } from "./hostDrafts.svelte";
  import { inventory } from "./inventory.svelte";
  import { NEW_SNIPPET } from "./snippetDrafts.svelte";
  import { findHost } from "./workspace";

  interface Props {
    hosts: string[];
    /** Snippets (by name) instead of hosts. */
    snippets: string[];
    doing: string;
    error: string | null;
    onsave: () => void;
    ondiscard: () => void;
    oncancel: () => void;
  }

  let { hosts, snippets, doing, error, onsave, ondiscard, oncancel }: Props =
    $props();

  let save: HTMLButtonElement;

  // Taken from the terminal or the field; Enter by reflex saves.
  onMount(() => save.focus());

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      oncancel();
    }
  }

  const name = (id: string) => {
    const tree = inventory.workspace?.tree;
    return (tree && findHost(tree, id)?.name) ?? id;
  };
  const changes = $derived(hosts.reduce((sum, id) => sum + unsaved(id), 0));
  const what = $derived(
    snippets.length === 1
      ? snippets[0] === NEW_SNIPPET
        ? NEW_SNIPPET
        : `snippet ${snippets[0]}`
      : snippets.length > 1
        ? `${snippets.length} snippets (${snippets.join(", ")})`
        : hosts.length === 1
          ? name(hosts[0])
          : `${hosts.length} hosts (${hosts.map(name).join(", ")})`,
  );
</script>

<svelte:window {onkeydown} />

<div
  class="backdrop"
  role="dialog"
  aria-modal="true"
  aria-labelledby="confirm-unsaved-title"
  data-confirm-unsaved
>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      onsave();
    }}
  >
    <h2 id="confirm-unsaved-title">Save changes to {what}?</h2>
    <p>
      {#if snippets.length > 0}
        Your changes are not saved yet. Save them before {doing}, or discard
        them?
      {:else}
        {changes === 1
          ? `1 change is not saved yet. Save it before ${doing}, or discard it?`
          : `${changes} changes are not saved yet. Save them before ${doing}, or discard them?`}
      {/if}
    </p>
    {#if error}
      <p class="error" role="alert" data-unsaved-error>
        Not saved: {error}
      </p>
    {/if}
    <div class="actions">
      <button type="button" data-unsaved="cancel" onclick={oncancel}
        >Cancel</button
      >
      <button type="button" data-unsaved="discard" onclick={ondiscard}
        >Discard</button
      >
      <button type="submit" data-unsaved="save" bind:this={save}>Save</button>
    </div>
  </form>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 40;
    display: grid;
    place-items: center;
    padding: var(--space-6);
    background: var(--backdrop);
  }

  form {
    width: min(26rem, 100%);
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
    font-size: var(--text-sm);
    line-height: 1.5;
  }

  .error {
    color: var(--danger);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }

  .actions button[data-unsaved="cancel"] {
    margin-right: auto;
  }
</style>
