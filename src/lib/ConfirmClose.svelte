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
    padding: 1.5rem;
    background: rgb(0 0 0 / 0.5);
  }

  form {
    width: min(24rem, 100%);
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
