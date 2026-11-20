<script lang="ts">
  // Closing the main window while sessions are live, in it or in session
  // windows (#88), or Files tabs' transfers run or wait (#106): asked once,
  // then every window closes.
  import { onMount } from "svelte";
  import { transfersText } from "./fileTransfers.svelte";
  import { inSessionWindows } from "./windows";

  interface Props {
    /** The app's name, as the shell gives it. */
    product: string;
    /** How many sessions are connecting or connected, in every window. */
    live: number;
    /** How many of them are in session windows. */
    elsewhere: number;
    /** How many transfers the Files tabs have running or waiting. */
    transfers?: number;
    onquit: () => void;
    oncancel: () => void;
  }

  let {
    product,
    live,
    elsewhere,
    transfers = 0,
    onquit,
    oncancel,
  }: Props = $props();

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
  const open = $derived(
    live === 0
      ? ""
      : `${sessions}${where} ${live === 1 ? "is" : "are"} still open`,
  );
  const moving = $derived(
    transfers === 0
      ? ""
      : `${transfersText(transfers)} ${transfers === 1 ? "is" : "are"} running or waiting in Files tabs`,
  );
  /** What closing ends. */
  const ends = $derived(
    [
      live === 0 ? "" : live === 1 ? "the session" : "the sessions",
      transfers === 0
        ? ""
        : transfers === 1
          ? "cancels the transfer"
          : "cancels the transfers",
    ]
      .filter(Boolean)
      .join(", "),
  );
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
    <p data-quit-transfers={transfers || undefined}>
      {[open, moving].filter(Boolean).join("; ")}. Closing
      {live > 0 ? `ends ${ends}` : ends} and closes every window.
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
