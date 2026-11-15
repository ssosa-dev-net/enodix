<script lang="ts">
  // A newer version exists: a card in the corner that never takes focus, so
  // work goes on. An installed copy offers to install it; a portable copy
  // only points at the download.
  import {
    installUpdate,
    openReleasePage,
    shownOffer,
    updates,
  } from "./updates.svelte";

  interface Props {
    /** Sessions that installing would end (connecting or connected). */
    live: number;
  }

  let { live }: Props = $props();

  const offer = $derived(shownOffer());
  /** Installing asks once more when it would end sessions. */
  let confirming = $state(false);

  function install() {
    if (live > 0 && !confirming) {
      confirming = true;
      return;
    }
    confirming = false;
    void installUpdate();
  }
</script>

{#if offer}
  <aside
    class="notice"
    aria-live="polite"
    data-update={offer.action}
    data-update-version={offer.version}
  >
    <p class="title">Version {offer.version} is available</p>
    <p class="detail">
      You have {offer.current}.
      {#if offer.action === "notify"}
        This is a portable copy: download the new version and unpack it over
        this one.
      {/if}
    </p>
    {#if offer.notes}
      <p class="notes" data-update-notes>{offer.notes}</p>
    {/if}
    {#if confirming}
      <p class="warning" data-update-confirm>
        Installing ends {live === 1 ? "the open session" : `${live} sessions`}.
        Install anyway?
      </p>
    {/if}
    {#if updates.error}
      <p class="error" role="alert" data-update-error>{updates.error}</p>
    {/if}
    <div class="actions">
      {#if offer.page}
        <button
          type="button"
          data-update-page
          onmousedown={(event) => event.preventDefault()}
          onclick={() => void openReleasePage()}
          >{offer.action === "notify"
            ? "Download page"
            : "Release notes"}</button
        >
      {/if}
      <button
        type="button"
        data-update-later
        onmousedown={(event) => event.preventDefault()}
        onclick={() => {
          confirming = false;
          updates.error = null;
          updates.dismissed = offer.version;
        }}>Later</button
      >
      {#if offer.action === "install"}
        <button
          type="button"
          data-update-install
          disabled={updates.busy}
          onmousedown={(event) => event.preventDefault()}
          onclick={install}
          >{confirming ? "Install anyway" : "Install and restart"}</button
        >
      {/if}
    </div>
  </aside>
{/if}

<style>
  .notice {
    position: fixed;
    right: 1rem;
    bottom: 2.5rem; /* above the status line */
    z-index: 12;
    width: min(22rem, calc(100% - 2rem));
    display: grid;
    gap: var(--space-1-5);
    padding: var(--space-3) var(--space-3);
    background: var(--surface);
    border: 1px solid var(--border);
    border-left: 3px solid var(--accent);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow);
    font-size: var(--text-sm);
  }

  p {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .title {
    font-weight: var(--weight-semibold);
  }

  .detail,
  .notes {
    color: var(--muted);
  }

  .notes {
    max-height: 6rem;
    overflow: auto;
    white-space: pre-wrap;
  }

  .warning {
    color: var(--warning);
  }

  .error {
    color: var(--danger);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    flex-wrap: wrap;
    gap: var(--space-1-5);
  }
</style>
