<script lang="ts">
  // The host editor tab (#10): a host's details beside the terminals, with
  // room to read and fill them in. One tab follows the host selected in
  // the tree (subject null); pinning it keeps it on one host (subject:
  // the host's id), so that another tab can follow the tree.
  import Pin from "@lucide/svelte/icons/pin";
  import PinOff from "@lucide/svelte/icons/pin-off";
  import HostDetails from "./HostDetails.svelte";
  import { inventory, selectedHost } from "./inventory.svelte";
  import { openView } from "./views";
  import { findHost, type Host, type Protocol } from "./workspace";

  interface Props {
    /** The pinned host's id, or null to follow the selection. */
    subject: string | null;
    onconnect: (host: Host, protocol?: Protocol) => void;
    /** Closes this tab (pinning and unpinning open another). */
    onclose: () => void;
  }

  let { subject, onconnect, onclose }: Props = $props();

  const host = $derived.by(() => {
    if (subject === null) return selectedHost();
    const tree = inventory.workspace?.tree;
    return (tree && findHost(tree, subject)) ?? null;
  });

  function pin() {
    if (!host) return;
    openView("host", host.id);
    onclose();
  }

  function unpin() {
    openView("host", null);
    onclose();
  }
</script>

<div
  class="view"
  data-view="host"
  data-host-editor={host?.id ?? ""}
  data-host-editor-pinned={subject !== null || undefined}
>
  <div class="bar">
    <span class="mode">
      {#if subject === null}
        Follows the host selected in the tree
      {:else}
        Pinned to this host
      {/if}
    </span>
    {#if subject === null}
      <button
        class="compact"
        data-host-editor-pin
        disabled={!host}
        title="Keep this tab on this host when another is selected"
        onclick={pin}><Pin size={14} aria-hidden="true" /> Pin</button
      >
    {:else}
      <button
        class="compact"
        data-host-editor-unpin
        title="Follow the host selected in the tree again"
        onclick={unpin}><PinOff size={14} aria-hidden="true" /> Unpin</button
      >
    {/if}
  </div>
  <div class="body">
    {#if host}
      {#key host.id}
        <HostDetails {host} {onconnect} />
      {/key}
    {:else if subject === null}
      <p class="empty">Select a host in the tree to edit it here.</p>
    {:else}
      <p class="empty">
        This host is no longer in the workspace.
        <button class="compact" onclick={onclose}>Close tab</button>
      </p>
    {/if}
  </div>
</div>

<style>
  .view {
    display: flex;
    flex-direction: column;
    height: 100%;
    background: var(--bg);
  }

  .bar {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-1-5) var(--space-4);
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }

  .mode {
    margin-right: auto;
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
  }

  .body > :global(*) {
    max-width: 48rem;
    margin: 0 auto;
  }

  .empty {
    padding: var(--space-6) var(--space-4);
    color: var(--muted);
    line-height: 1.5;
  }
</style>
