<script lang="ts">
  // What the start found about where the app's data is (#128): a move to
  // the local data folder that failed or waits, or data in both folders. A
  // card in the corner that never takes focus, shown until dismissed.
  import { onMount } from "svelte";
  import { dataNotices, type DataNotice } from "./workspace";

  let notices = $state<DataNotice[]>([]);

  onMount(() => {
    dataNotices().then(
      (found) => (notices = found),
      () => {},
    );
  });
</script>

{#if notices.length > 0}
  <aside class="notice" aria-live="polite" data-data-notice={notices.length}>
    <p class="title">Where your data is</p>
    {#each notices as notice, index (index)}
      <p
        class="detail"
        data-data-notice-item={notice.kind}
        data-data-notice-what={notice.what}
      >
        {notice.message}
      </p>
    {/each}
    <div class="actions">
      <button
        type="button"
        data-data-notice-dismiss
        onmousedown={(event) => event.preventDefault()}
        onclick={() => (notices = [])}>Dismiss</button
      >
    </div>
  </aside>
{/if}

<style>
  .notice {
    position: fixed;
    right: 1rem;
    top: 3rem; /* below the top bar */
    z-index: 12;
    width: min(26rem, calc(100% - 2rem));
    max-height: calc(100% - 6rem);
    overflow: auto;
    display: grid;
    gap: var(--space-1-5);
    padding: var(--space-3) var(--space-3);
    background: var(--surface);
    border: 1px solid var(--border);
    border-left: 3px solid var(--warning);
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

  .detail {
    color: var(--muted);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-1-5);
  }
</style>
