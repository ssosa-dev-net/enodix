<script lang="ts">
  // The details sheet (#64): the selected host's or folder's details, over
  // the lower part of the Hosts side view, so that the tree stays above it
  // and the sheet follows what is selected there. It never covers a
  // terminal. Back (or Escape, or Ctrl+Shift+D) closes it.
  import { tick } from "svelte";
  import ChevronLeft from "@lucide/svelte/icons/chevron-left";
  import FolderDetails from "./FolderDetails.svelte";
  import HostDetails from "./HostDetails.svelte";
  import type { Folder, Host, Protocol } from "./workspace";

  interface Props {
    host: Host | null;
    folder: Folder | null;
    onconnect: (host: Host, protocol?: Protocol) => void;
    onclose: () => void;
  }

  let { host, folder, onconnect, onclose }: Props = $props();

  let sheet = $state<HTMLElement>();

  // Another item shown: its details start at the top, and its row stays
  // in sight in the shorter tree above.
  $effect(() => {
    void (host?.id ?? folder?.path);
    if (sheet) sheet.scrollTop = 0;
    void tick().then(() =>
      document
        .querySelector("nav [aria-pressed=true], nav .selected")
        ?.scrollIntoView({ block: "nearest" }),
    );
  });

  function onkeydown(event: KeyboardEvent) {
    if (event.key !== "Escape" || event.defaultPrevented) return;
    const target = event.target as Element;
    // Escape in a field leaves the field alone (it may undo there).
    if (target.closest("input, textarea, select")) return;
    event.preventDefault();
    onclose();
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<section
  class="side sheet"
  data-details-sheet={host ? "host" : "folder"}
  aria-label={host ? "Host details" : "Folder details"}
  {onkeydown}
>
  <div class="bar">
    <button
      class="back"
      data-details-close
      title="Back to the tree (Ctrl+Shift+D)"
      onclick={onclose}
      ><ChevronLeft size={14} aria-hidden="true" /> Back to the tree</button
    >
    <span class="what">{host ? "Host" : "Folder"} details</span>
  </div>
  <div class="body" bind:this={sheet}>
    {#if host}
      <HostDetails {host} {onconnect} />
    {:else if folder}
      <FolderDetails {folder} />
    {/if}
  </div>
</section>

<style>
  .sheet {
    display: flex;
    flex-direction: column;
    min-height: 0;
    border-top: 1px solid var(--border);
    background: var(--surface);
    box-shadow: 0 -6px 16px color-mix(in srgb, black 25%, transparent);
  }

  .bar {
    flex: none;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-2);
    border-bottom: 1px solid var(--border);
  }

  .back {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
    min-height: var(--control-height-small);
    padding: 0 var(--space-2);
    font-size: var(--text-xs);
  }

  .what {
    margin-left: auto;
    color: var(--muted);
    font-size: var(--text-xs);
  }

  .body {
    flex: 1 1 auto;
    min-height: 0;
    overflow: auto;
  }
</style>
