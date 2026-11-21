<script lang="ts">
  // Asks before a folder of other things becomes a workspace (#131): the
  // core found no host files and no workspace file in it, and opening it
  // writes _workspace.yaml there. Nothing is written before the answer.
  import { onMount } from "svelte";

  interface Props {
    /** The folder, as it was chosen or typed. */
    path: string;
    onopen: () => void;
    oncancel: () => void;
  }

  let { path, onopen, oncancel }: Props = $props();

  let cancel: HTMLButtonElement;

  // A folder chosen by mistake is the likely case: Enter by reflex cancels.
  onMount(() => cancel.focus());

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      oncancel();
    }
  }
</script>

<svelte:window {onkeydown} />

<div
  class="backdrop"
  role="dialog"
  aria-modal="true"
  aria-labelledby="confirm-folder-title"
  aria-describedby="confirm-folder-what"
  data-confirm-open-folder={path}
>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      onopen();
    }}
  >
    <h2 id="confirm-folder-title">Make this folder a workspace?</h2>
    <p class="path" data-confirm-folder-path>{path}</p>
    <p id="confirm-folder-what">
      No host files here, and it is not a workspace yet. Opening it makes it
      one: a small file, _workspace.yaml, is written into it, and its subfolders
      show as host folders.
    </p>
    <div class="actions">
      <button
        type="button"
        data-confirm-folder="cancel"
        bind:this={cancel}
        onclick={oncancel}>Cancel</button
      >
      <button type="submit" data-confirm-folder="open"
        >Make it a workspace</button
      >
    </div>
  </form>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 24;
    display: grid;
    place-items: center;
    padding: var(--space-6);
    background: var(--backdrop);
  }

  form {
    width: min(28rem, 100%);
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
  }

  p {
    margin: 0;
    font-size: var(--text-sm);
  }

  .path {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    color: var(--muted);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
