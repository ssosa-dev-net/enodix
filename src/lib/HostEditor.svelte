<script lang="ts">
  // The host editor tab (#10): a host's details beside the terminals, with
  // room to read and fill them in. One tab follows the host selected in
  // the tree (subject null); pinning it keeps it on one host (subject:
  // the host's id), so that another tab can follow the tree.
  //
  // Edits are not written to the host's file one by one: the host's files
  // go into git, so they collect until Save (Ctrl+S), and Revert drops
  // them (hostDrafts). Showing another host, closing the tab, quitting or
  // opening another workspace with edits unsaved asks first.
  import { untrack } from "svelte";
  import Pin from "@lucide/svelte/icons/pin";
  import PinOff from "@lucide/svelte/icons/pin-off";
  import Button from "./Button.svelte";
  import HostDetails from "./HostDetails.svelte";
  import {
    edit,
    following,
    revert,
    save,
    saveError,
    settleUnsaved,
    shown,
    unsaved,
  } from "./hostDrafts.svelte";
  import { inventory, select, selectedHost } from "./inventory.svelte";
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

  /** The host the tab shows while following: the selected one, once its
   * predecessor's edits are settled. */
  let followed = $state<string | null>(
    untrack(() => selectedHost()?.id ?? null),
  );

  /** The selected host's id. A derived value notifies only when the id
   * changes, not each time the tree is replaced: a save replaces it while
   * its draft is still unsettled, and the effect below would then ask
   * about the same edits a second time. */
  const selectedId = $derived(selectedHost()?.id ?? null);

  $effect(() => {
    if (subject !== null) return;
    const next = selectedId;
    untrack(() => {
      const from = followed;
      if (next === null || next === from) return;
      if (from === null || unsaved(from) === 0) {
        followed = next;
        return;
      }
      const tree = inventory.workspace?.tree;
      const nextName = (tree && findHost(tree, next)?.name) ?? next;
      void settleUnsaved([from], `showing ${nextName}`).then((go) => {
        if (go) {
          go();
          followed = next;
        }
        // Stay: the tree selects the host still being edited again.
        else select({ kind: "host", id: from });
      });
    });
  });

  // The window asks about this tab's edits when it closes.
  $effect(() => {
    if (subject !== null) return;
    following.host = followed;
    return () => {
      if (following.host === followed) following.host = null;
    };
  });

  /** The host as saved. */
  const saved = $derived.by(() => {
    const id = subject ?? followed;
    const tree = inventory.workspace?.tree;
    return (id && tree && findHost(tree, id)) || null;
  });

  const count = $derived(saved ? unsaved(saved.id) : 0);
  const error = $derived(saved ? saveError(saved.id) : null);
  let saving = $state(false);

  async function saveEdits() {
    if (!saved || count === 0 || saving) return;
    saving = true;
    try {
      await save(saved.id);
    } finally {
      saving = false;
    }
  }

  function pin() {
    if (!saved) return;
    openView("host", saved.id);
    onclose();
  }

  function unpin() {
    openView("host", null);
    onclose();
  }

  function onkeydown(event: KeyboardEvent) {
    if (
      event.key.toLowerCase() === "s" &&
      (event.ctrlKey || event.metaKey) &&
      !event.shiftKey &&
      !event.altKey
    ) {
      event.preventDefault();
      // The field being typed in is part of the save.
      (document.activeElement as HTMLElement | null)?.blur?.();
      void saveEdits();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="view"
  data-view="host"
  data-host-editor={saved?.id ?? ""}
  data-host-editor-pinned={subject !== null || undefined}
  data-unsaved={count}
  {onkeydown}
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
      <Button
        size="compact"
        variant="ghost"
        data-host-editor-pin
        disabled={!saved}
        title="Keep this tab on this host when another is selected"
        onclick={pin}><Pin size={14} aria-hidden="true" /> Pin</Button
      >
    {:else}
      <Button
        size="compact"
        variant="ghost"
        data-host-editor-unpin
        title="Follow the host selected in the tree again"
        onclick={unpin}><PinOff size={14} aria-hidden="true" /> Unpin</Button
      >
    {/if}
    <span class="unsaved" data-unsaved-count={count} aria-live="polite">
      {#if count > 0}
        {count === 1 ? "1 unsaved change" : `${count} unsaved changes`}
      {:else if saved}
        All changes saved
      {/if}
    </span>
    <Button
      size="compact"
      data-host-revert
      disabled={count === 0 || saving}
      title="Drop the unsaved changes"
      onclick={() => saved && revert(saved.id)}>Revert</Button
    >
    <Button
      size="compact"
      variant="primary"
      data-host-save
      disabled={count === 0 || saving}
      title="Write the changes to the host's file (Ctrl+S)"
      onclick={saveEdits}>Save</Button
    >
  </div>
  {#if error}
    <p class="error" role="alert" data-host-save-error>Not saved: {error}</p>
  {/if}
  <div class="body">
    {#if saved}
      {@const host = saved}
      {#key host.id}
        <HostDetails
          host={shown(host)}
          saved={host}
          {onconnect}
          onedit={(changes) => edit(host, changes)}
        />
      {/key}
    {:else if subject === null}
      <p class="empty">Select a host in the tree to edit it here.</p>
    {:else}
      <p class="empty">
        This host is no longer in the workspace.
        <Button size="compact" onclick={onclose}>Close tab</Button>
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
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .unsaved {
    margin-left: auto;
    font-size: var(--text-xs);
    color: var(--muted);
    font-variant-numeric: tabular-nums;
  }

  .error {
    margin: 0;
    padding: var(--space-2) var(--space-4);
    font-size: var(--text-sm);
    color: var(--danger);
    border-bottom: 1px solid var(--border);
    background: var(--surface);
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
