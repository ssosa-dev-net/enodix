<script lang="ts">
  // Closing a session window with live sessions (#88): close them, dock
  // them back into the main window, or cancel. Over the whole window: it
  // is about all of its sessions.
  import { onMount } from "svelte";

  interface Props {
    /** The names of its sessions that are connecting or connected. */
    live: string[];
    /** Whether one of them is to a production host: Cancel has the focus. */
    production: boolean;
    ondock: () => void;
    onclosesessions: () => void;
    oncancel: () => void;
  }

  let { live, production, ondock, onclosesessions, oncancel }: Props = $props();

  let cancel: HTMLButtonElement;
  let dock: HTMLButtonElement;

  // Taken from the terminal, which would otherwise keep the keys. Enter by
  // reflex keeps the sessions: docked back, or (production) the window.
  onMount(() => (production ? cancel : dock).focus());

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      oncancel();
    }
  }

  const count = $derived(
    live.length === 1 ? "1 session" : `${live.length} sessions`,
  );
</script>

<svelte:window {onkeydown} />

<div
  class="backdrop"
  role="dialog"
  aria-modal="true"
  aria-labelledby="confirm-close-window-title"
  data-confirm-close-window
  data-environment={production ? "production" : undefined}
>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      ondock();
    }}
  >
    <h2 id="confirm-close-window-title">Dock this window's sessions back?</h2>
    <p>
      {count} still open here: {live.join(", ")}. Docked back, they stay
      connected in the main window; closed, they end.
    </p>
    <div class="actions">
      <button
        type="button"
        data-close-window="cancel"
        bind:this={cancel}
        onclick={oncancel}>Cancel</button
      >
      <button type="button" data-close-window="close" onclick={onclosesessions}
        >Close them</button
      >
      <button type="submit" data-close-window="dock" bind:this={dock}
        >Dock back</button
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
    overflow: auto;
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
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
