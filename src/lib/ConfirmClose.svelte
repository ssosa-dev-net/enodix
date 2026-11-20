<script lang="ts">
  import { onMount } from "svelte";
  import { transfersText } from "./fileTransfers.svelte";
  import type { Environment } from "./workspace";

  interface Props {
    /** The tab's name, such as the host's. */
    name: string;
    state: "connecting" | "connected";
    /** The session's environment: on production Cancel has the focus. */
    environment?: Environment | null;
    /** A Files tab's transfers running or waiting (#106): the question is
     * about them, and Cancel has the focus. */
    transfers?: number;
    /** A Files tab's question: it stays when its transfers end meanwhile
     * (closing then clears their kept outcomes). */
    files?: boolean;
    onclose: () => void;
    oncancel: () => void;
  }

  let {
    name,
    state,
    environment = null,
    transfers = 0,
    files = false,
    onclose,
    oncancel,
  }: Props = $props();

  let cancel: HTMLButtonElement;
  let confirm: HTMLButtonElement;

  // Taken from the terminal, which would otherwise keep the keys. On a
  // production host, or with transfers to lose, Enter by reflex keeps them.
  onMount(() =>
    (environment === "production" || files ? cancel : confirm).focus(),
  );

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
  data-confirm-transfers={files ? transfers : undefined}
  data-environment={environment ?? undefined}
>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      onclose();
    }}
  >
    {#if files}
      <h2 id="confirm-close-title">Close the files of {name}?</h2>
      <p>
        {#if transfers > 0}
          {transfersText(transfers)}
          {transfers === 1 ? "is" : "are"} running or waiting. Closing the tab cancels
          {transfers === 1 ? "it" : "them"}.
        {:else}
          Its transfers have ended. Closing the tab clears their list.
        {/if}
      </p>
    {:else}
      <h2 id="confirm-close-title">Close the session to {name}?</h2>
      <p>
        It is still {state}. Closing the tab ends the session.
      </p>
    {/if}
    <div class="actions">
      <button
        type="button"
        data-confirm="cancel"
        bind:this={cancel}
        onclick={oncancel}>Cancel</button
      >
      <button type="submit" data-confirm="close" bind:this={confirm}
        >{files ? "Close tab" : "Close session"}</button
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
