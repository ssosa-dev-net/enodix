<script lang="ts">
  import { tick } from "svelte";
  import {
    createWorkspace,
    forgetWorkspace,
    listWorkspaces,
    pickWorkspaceFolder,
    type KnownWorkspace,
    type WorkspaceView,
  } from "./workspace";

  interface Props {
    /**
     * Switches to the workspace `open` gives (asking first about the open
     * sessions, if any); resolves to why it failed, or null.
     */
    onswitch: (open: () => Promise<WorkspaceView>) => Promise<string | null>;
    /** Opens the folder at `path`, asking first about a folder of other
     * things (#131), then as `onswitch`. */
    onopen: (path: string) => Promise<string | null>;
    onclose: () => void;
  }

  let { onswitch, onopen, onclose }: Props = $props();

  let known = $state<KnownWorkspace[]>([]);
  let query = $state("");
  let active = $state(0);
  let error = $state<string | null>(null);
  /** The folder typed (or browsed to) for a new or added workspace. */
  let createFolder = $state("");
  let addFolder = $state("");
  let busy = $state(false);
  let input = $state<HTMLInputElement>();
  let list = $state<HTMLElement>();

  const shown = $derived.by(() => {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    return known.filter((workspace) =>
      words.every((word) =>
        `${workspace.name} ${workspace.path}`.toLowerCase().includes(word),
      ),
    );
  });

  // Several workspaces may share a folder name: those show their path too.
  const shared = $derived.by(() => {
    const counts = new Map<string, number>();
    for (const workspace of known) {
      const name = workspace.name.toLowerCase();
      counts.set(name, (counts.get(name) ?? 0) + 1);
    }
    return counts;
  });

  listWorkspaces().then(
    (list) => {
      known = list;
      // The open one is first, being the most recent: offer the next.
      active = list.length > 1 && list[0].open ? 1 : 0;
    },
    (reason) => (error = String(reason)),
  );

  // Take the keyboard from whatever had it (the terminal, say).
  $effect(() => {
    input?.focus();
  });

  async function run(go: () => Promise<string | null>) {
    if (busy) return;
    busy = true;
    error = null;
    try {
      error = await go();
    } finally {
      busy = false;
    }
  }

  function switchTo(workspace: KnownWorkspace) {
    if (workspace.open) {
      onclose();
      return;
    }
    void run(() => onopen(workspace.path));
  }

  async function forget(workspace: KnownWorkspace) {
    error = null;
    try {
      known = await forgetWorkspace(workspace.path);
    } catch (reason) {
      error = String(reason);
    }
  }

  async function browse(which: "create" | "add") {
    const folder = await pickWorkspaceFolder(
      which === "create"
        ? "Folder for the new workspace"
        : "Existing workspace folder",
    );
    if (folder === null) return;
    if (which === "create") createFolder = folder;
    else addFolder = folder;
  }

  async function move(by: number) {
    if (shown.length === 0) return;
    active = (active + by + shown.length) % shown.length;
    await tick();
    list
      ?.querySelector(`[data-index="${active}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      void move(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      void move(-1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const workspace = shown[active];
      if (workspace) switchTo(workspace);
    }
  }

  function onwindowkeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && !busy) {
      event.preventDefault();
      onclose();
    }
  }
</script>

<svelte:window onkeydown={onwindowkeydown} />

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
  class="backdrop"
  onclick={(event) => {
    if (event.target === event.currentTarget && !busy) onclose();
  }}
>
  <div
    class="switcher"
    role="dialog"
    aria-modal="true"
    aria-labelledby="workspaces-title"
    data-dialog="workspaces"
  >
    <h2 id="workspaces-title">Workspaces</h2>
    <input
      bind:this={input}
      name="workspace-query"
      placeholder="Switch to a workspace…"
      bind:value={query}
      oninput={() => (active = 0)}
      {onkeydown}
      autocomplete="off"
      spellcheck="false"
      role="combobox"
      aria-expanded="true"
      aria-controls="workspace-list"
      aria-activedescendant={shown.length ? `workspace-${active}` : undefined}
    />
    <ul id="workspace-list" role="listbox" bind:this={list}>
      {#each shown as workspace, index (workspace.path)}
        {@const several = (shared.get(workspace.name.toLowerCase()) ?? 0) > 1}
        <li
          id="workspace-{index}"
          role="option"
          aria-selected={index === active}
          aria-disabled={!workspace.available}
          class:active={index === active}
          class:missing={!workspace.available}
          data-index={index}
          data-workspace={workspace.path}
          data-workspace-open={workspace.open || undefined}
          title={workspace.path}
          onclick={() => switchTo(workspace)}
          onpointermove={() => (active = index)}
        >
          <span class="name">{workspace.name}</span>
          {#if workspace.open}<span class="tag">open</span>{/if}
          {#if !workspace.available}<span class="tag">folder not found</span
            >{/if}
          <span class="path" class:shown={several}>{workspace.path}</span>
          <button
            type="button"
            class="remove"
            data-workspace-remove
            disabled={workspace.open}
            title={workspace.open
              ? "The open workspace stays on the list"
              : "Remove from this list (the folder is not deleted)"}
            onclick={(event) => {
              event.stopPropagation();
              void forget(workspace);
            }}>Remove</button
          >
        </li>
      {/each}
    </ul>
    {#if known.length > 0 && shown.length === 0}
      <p class="empty">No workspace matches.</p>
    {:else if known.length === 0}
      <p class="empty">No workspace yet: create one or add a folder.</p>
    {/if}

    {#if error}
      <p class="error" role="alert" data-workspace-error>{error}</p>
    {/if}

    <form
      data-workspace-form="create"
      onsubmit={(event) => {
        event.preventDefault();
        const folder = createFolder.trim();
        if (folder) void run(() => onswitch(() => createWorkspace(folder)));
      }}
    >
      <label for="workspace-create-folder">New empty workspace in</label>
      <div class="row">
        <input
          id="workspace-create-folder"
          name="workspace-folder"
          placeholder="Folder (created if missing)"
          bind:value={createFolder}
          autocomplete="off"
          spellcheck="false"
        />
        <button type="button" onclick={() => browse("create")}>Browse…</button>
        <button type="submit" disabled={busy || !createFolder.trim()}
          >Create</button
        >
      </div>
    </form>

    <form
      data-workspace-form="add"
      onsubmit={(event) => {
        event.preventDefault();
        const folder = addFolder.trim();
        if (folder) void run(() => onopen(folder));
      }}
    >
      <label for="workspace-add-folder">Open an existing folder</label>
      <div class="row">
        <input
          id="workspace-add-folder"
          name="workspace-folder"
          placeholder="Workspace folder, such as a git clone"
          bind:value={addFolder}
          autocomplete="off"
          spellcheck="false"
        />
        <button type="button" onclick={() => browse("add")}>Browse…</button>
        <button type="submit" disabled={busy || !addFolder.trim()}>Open</button>
      </div>
    </form>

    <footer>
      <span>Removing a workspace from the list never deletes its folder.</span>
      <span class="keys"><kbd>Enter</kbd> switch · <kbd>Esc</kbd> close</span>
    </footer>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 22;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding: 10vh var(--space-4) var(--space-4);
    background: var(--backdrop);
  }

  .switcher {
    width: min(40rem, 100%);
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    overflow: hidden;
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow: auto;
    min-height: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }

  ul:empty {
    display: none;
  }

  li {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-2);
    font-size: var(--text-sm);
    cursor: pointer;
    white-space: nowrap;
  }

  li.active {
    background: var(--surface);
    box-shadow: inset 2px 0 var(--accent);
  }

  li.missing .name {
    color: var(--muted);
    text-decoration: line-through;
  }

  .name {
    font-weight: var(--weight-semibold);
  }

  .tag {
    font-size: var(--text-2xs);
    color: var(--muted);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding: 0 var(--space-1);
  }

  .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--muted);
    font-size: var(--text-xs);
  }

  /* Workspaces of the same name: the path tells them apart. */
  .path.shown,
  li.active .path {
    color: var(--text);
  }

  .remove {
    min-height: var(--control-height-small);
    padding: 0 var(--space-1-5);
    font-size: var(--text-xs);
  }

  form {
    display: grid;
    gap: var(--space-1);
    font-size: var(--text-sm);
  }

  .row {
    display: flex;
    gap: var(--space-1-5);
  }

  .row input {
    flex: 1;
    min-width: 0;
  }

  .empty,
  .error {
    margin: 0;
    font-size: var(--text-sm);
    color: var(--muted);
  }

  .error {
    color: var(--danger);
    overflow-wrap: anywhere;
  }

  footer {
    display: flex;
    justify-content: space-between;
    gap: var(--space-4);
    padding-top: var(--space-1-5);
    border-top: 1px solid var(--border);
    color: var(--muted);
    font-size: var(--text-xs);
  }

  kbd {
    font-family: inherit;
    color: var(--text);
  }
</style>
