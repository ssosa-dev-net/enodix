<script lang="ts">
  // Closing the main window while sessions are live, in it or in session
  // windows (#88): asked once, then every window closes.
  import { onMount } from "svelte";
  import { inSessionWindows } from "./windows";

  interface Props {
    /** The app's name, as the shell gives it. */
    product: string;
    /** How many sessions are connecting or connected, in every window. */
    live: number;
    /** How many of them are in session windows. */
    elsewhere: number;
    onquit: () => void;
    oncancel: () => void;
  }

  let { product, live, elsewhere, onquit, oncancel }: Props = $props();

  let cancel: HTMLButtonElement;

  // Taken from the terminal; Enter by reflex keeps everything open.
  onMount(() => cancel.focus());

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      oncancel();
    }
  }

  const sessions = $derived(live === 1 ? "1 session" : `${live} sessions`);
  const where = $derived(inSessionWindows(live, elsewhere));
</script>

<svelte:window {onkeydown} />

<div
  class="backdrop"
  role="dialog"
  aria-modal="true"
  aria-labelledby="confirm-quit-title"
  data-confirm-quit
>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      oncancel();
    }}
  >
    <h2 id="confirm-quit-title">Close {product || "the app"}?</h2>
    <p>
      {sessions}{where}
      {live === 1 ? "is" : "are"} still open. Closing ends {live === 1
        ? "it"
        : "them all"} and closes every window.
    </p>
    <div class="actions">
      <button type="submit" data-quit="cancel" bind:this={cancel}>Cancel</button
      >
      <button type="button" data-quit="close" onclick={onquit}>Close all</button
      >
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
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
