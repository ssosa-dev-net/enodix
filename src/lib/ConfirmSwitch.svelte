<script lang="ts">
  import { onMount } from "svelte";
  import { transfersText } from "./fileTransfers.svelte";
  import { inSessionWindows } from "./windows";

  interface Props {
    /** The workspace being left. */
    from: string;
    /** How many of its sessions are connecting or connected. */
    live: number;
    /** How many of them are in session windows (#88). */
    elsewhere?: number;
    /** Transfers its Files tabs have running or waiting (#106): those tabs
     * close with the workspace, so they are cancelled either way. */
    transfers?: number;
    onkeep: () => void;
    onclosesessions: () => void;
    oncancel: () => void;
  }

  let {
    from,
    live,
    elsewhere = 0,
    transfers = 0,
    onkeep,
    onclosesessions,
    oncancel,
  }: Props = $props();

  let keep: HTMLButtonElement;
  let cancel: HTMLButtonElement;

  // Taken from the terminal and the switcher, which would keep the keys.
  // With transfers to lose, Enter by reflex keeps them.
  onMount(() => (transfers > 0 ? cancel : keep).focus());

  const cancelled = $derived(
    `${transfersText(transfers)} in its Files tabs ${transfers === 1 ? "is" : "are"} running or waiting; switching closes those tabs and cancels ${transfers === 1 ? "it" : "them"}.`,
  );

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
  data-switch-transfers={transfers || undefined}
>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      onkeep();
    }}
  >
    {#if live === 0}
      <h2 id="confirm-switch-title">Leave {from}?</h2>
      <p>{cancelled}</p>
    {:else}
      <h2 id="confirm-switch-title">Keep the sessions of {from} open?</h2>
      <p>
        {live === 1 ? "1 session" : `${live} sessions`} to hosts of {from}
        {live === 1 ? "is" : "are"} still open{inSessionWindows(
          live,
          elsewhere,
        )}. Kept sessions stay in their tabs, in whichever window, while you
        work in the other workspace.
      </p>
      {#if transfers > 0}
        <p>{cancelled}</p>
      {/if}
    {/if}
    <div class="actions">
      <button
        type="button"
        data-switch="cancel"
        bind:this={cancel}
        onclick={oncancel}>Cancel</button
      >
      {#if live > 0}
        <button type="button" data-switch="close" onclick={onclosesessions}
          >Close them</button
        >
      {/if}
      <button type="submit" data-switch="keep" bind:this={keep}
        >{live > 0 ? "Keep them open" : "Switch"}</button
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
