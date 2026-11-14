<script lang="ts">
  import { onMount } from "svelte";

  interface Props {
    /** The workspace being left. */
    from: string;
    /** How many of its sessions are connecting or connected. */
    live: number;
    onkeep: () => void;
    onclosesessions: () => void;
    oncancel: () => void;
  }

  let { from, live, onkeep, onclosesessions, oncancel }: Props = $props();

  let keep: HTMLButtonElement;

  // Taken from the terminal and the switcher, which would keep the keys.
  onMount(() => keep.focus());

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
  aria-labelledby="confirm-switch-title"
  data-confirm-switch
>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      onkeep();
    }}
  >
    <h2 id="confirm-switch-title">Keep the sessions of {from} open?</h2>
    <p>
      {live === 1 ? "1 session" : `${live} sessions`} to hosts of {from}
      {live === 1 ? "is" : "are"} still open. Kept sessions stay in their tabs while
      you work in the other workspace.
    </p>
    <div class="actions">
      <button type="button" data-switch="cancel" onclick={oncancel}
        >Cancel</button
      >
      <button type="button" data-switch="close" onclick={onclosesessions}
        >Close them</button
      >
      <button type="submit" data-switch="keep" bind:this={keep}
        >Keep them open</button
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
    padding: 1.5rem;
    background: rgb(0 0 0 / 0.5);
  }

  form {
    width: min(26rem, 100%);
    display: grid;
    gap: 0.8rem;
    padding: 1.2rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
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

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.6rem;
  }
</style>
