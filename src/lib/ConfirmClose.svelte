<script lang="ts">
  import { onMount } from "svelte";

  interface Props {
    /** The tab's name, such as the host's. */
    name: string;
    state: "connecting" | "connected";
    onclose: () => void;
    oncancel: () => void;
  }

  let { name, state, onclose, oncancel }: Props = $props();

  let confirm: HTMLButtonElement;

  // Taken from the terminal, which would otherwise keep the keys.
  onMount(() => confirm.focus());

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      oncancel();
    }
  }
</script>

<svelte:window {onkeydown} />

<div
  class="backdrop"
  role="dialog"
  aria-modal="true"
  aria-labelledby="confirm-close-title"
  data-confirm-close
>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      onclose();
    }}
  >
    <h2 id="confirm-close-title">Close the session to {name}?</h2>
    <p>
      It is still {state}. Closing the tab ends the session.
    </p>
    <div class="actions">
      <button type="button" data-confirm="cancel" onclick={oncancel}
        >Cancel</button
      >
      <button type="submit" data-confirm="close" bind:this={confirm}
        >Close session</button
      >
    </div>
  </form>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 20;
    display: grid;
    place-items: center;
    padding: var(--space-6);
    background: var(--backdrop);
  }

  form {
    width: min(24rem, 100%);
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
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
