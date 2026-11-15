<script lang="ts">
  import { onMount } from "svelte";
  import type { Environment } from "./workspace";

  interface Props {
    /** The tab's name, such as the host's. */
    name: string;
    state: "connecting" | "connected";
    /** The session's environment: on production Cancel has the focus. */
    environment?: Environment | null;
    onclose: () => void;
    oncancel: () => void;
  }

  let { name, state, environment = null, onclose, oncancel }: Props = $props();

  let cancel: HTMLButtonElement;
  let confirm: HTMLButtonElement;

  // Taken from the terminal, which would otherwise keep the keys. On a
  // production host Enter by reflex keeps the session.
  onMount(() => (environment === "production" ? cancel : confirm).focus());

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      oncancel();
    }
  }
</script>

<div
  class="backdrop"
  role="dialog"
  tabindex="-1"
  {onkeydown}
  aria-labelledby="confirm-close-title"
  data-confirm-close
  data-environment={environment ?? undefined}
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
      <button
        type="button"
        data-confirm="cancel"
        bind:this={cancel}
        onclick={oncancel}>Cancel</button
      >
      <button type="submit" data-confirm="close" bind:this={confirm}
        >Close session</button
      >
    </div>
  </form>
</div>

<style>
  /* Over its pane only (#74): the caller places it there. */
  .backdrop {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: var(--space-4);
    background: var(--backdrop);
    overflow: auto;
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
