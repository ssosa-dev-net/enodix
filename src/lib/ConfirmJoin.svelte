<script lang="ts">
  import { onMount } from "svelte";

  interface Props {
    /** The production sessions about to join, by their tab's name. */
    production: string[];
    /** How many tabs join (a whole pane may add more than these). */
    joining: number;
    onjoin: () => void;
    oncancel: () => void;
  }

  let { production, joining, onjoin, oncancel }: Props = $props();

  let cancel: HTMLButtonElement;

  // Taken from the terminal, which would otherwise keep the keys. Enter by
  // reflex cancels: these are production devices.
  onMount(() => cancel.focus());

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
  role="alertdialog"
  aria-modal="true"
  aria-labelledby="confirm-join-title"
  data-confirm-join
  data-join-count={joining}
>
  <form
    onsubmit={(event) => {
      event.preventDefault();
      onjoin();
    }}
  >
    <h2 id="confirm-join-title">
      {#if production.length === 1 && joining === 1}
        Add the production session {production[0]} to the multi-send group?
      {:else}
        Add {joining}
        {joining === 1 ? "session" : "sessions"} to the multi-send group?
      {/if}
    </h2>
    <p class="warning">
      What you type in any member then reaches
      {production.length === 1
        ? "this production device"
        : "these production devices"}
      too.
    </p>
    <ul>
      <!-- Two tabs may show the same host. -->
      {#each production as name, index (index)}
        <li data-join-production={name}>{name}</li>
      {/each}
    </ul>
    <div class="actions">
      <button
        type="button"
        data-join="cancel"
        bind:this={cancel}
        onclick={oncancel}>Cancel</button
      >
      <button type="submit" data-join="join">Add to group</button>
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
    width: min(26rem, 100%);
    display: grid;
    gap: var(--space-3);
    padding: var(--space-4);
    background: var(--dialog);
    border: 2px solid var(--env-production);
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

  .warning {
    color: var(--env-production);
    font-weight: var(--weight-semibold);
  }

  ul {
    margin: 0;
    padding-left: var(--space-4);
    font-size: var(--text-sm);
    max-height: 10rem;
    overflow: auto;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
