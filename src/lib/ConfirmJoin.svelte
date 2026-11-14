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
    padding: 1.5rem;
    background: rgb(0 0 0 / 0.5);
  }

  form {
    width: min(26rem, 100%);
    display: grid;
    gap: 0.8rem;
    padding: 1.2rem;
    background: var(--bg);
    border: 2px solid var(--env-production);
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

  .warning {
    color: var(--env-production);
    font-weight: 600;
  }

  ul {
    margin: 0;
    padding-left: 1.2rem;
    font-size: 0.85rem;
    max-height: 10rem;
    overflow: auto;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.6rem;
  }
</style>
