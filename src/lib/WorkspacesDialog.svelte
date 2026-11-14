<script lang="ts">
  import { tick } from "svelte";
  import {
    createWorkspace,
    forgetWorkspace,
    listWorkspaces,
    openWorkspace,
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
    onclose: () => void;
  }

  let { onswitch, onclose }: Props = $props();

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

  async function run(open: () => Promise<WorkspaceView>) {
    if (busy) return;
    busy = true;
    error = null;
    try {
      error = await onswitch(open);
    } finally {
      busy = false;
    }
  }

  function switchTo(workspace: KnownWorkspace) {
    if (workspace.open) {
      onclose();
      return;
    }
    void run(() => openWorkspace(workspace.path));
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
        if (folder) void run(() => createWorkspace(folder));
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
        if (folder) void run(() => openWorkspace(folder));
      }}
    >
      <label for="workspace-add-folder">Add an existing folder</label>
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
        <button type="submit" disabled={busy || !addFolder.trim()}>Add</button>
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
    padding: 10vh 1rem 1rem;
    background: rgb(0 0 0 / 0.35);
  }

  .switcher {
    width: min(40rem, 100%);
    max-height: 80vh;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.8rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.45);
    overflow: hidden;
  }

  h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
  }

  input {
    padding: 0.4rem 0.6rem;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow: auto;
    min-height: 0;
    border: 1px solid var(--border);
    border-radius: 4px;
  }

  ul:empty {
    display: none;
  }

  li {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    padding: 0.3rem 0.6rem;
    font-size: 0.85rem;
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
    font-weight: 600;
  }

  .tag {
    font-size: 0.72rem;
    color: var(--muted);
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 0 0.3rem;
  }

  .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    color: var(--muted);
    font-size: 0.78rem;
    opacity: 0.7;
  }

  .path.shown,
  li.active .path {
    opacity: 1;
  }

  .remove {
    padding: 0.05rem 0.45rem;
    font-size: 0.75rem;
  }

  form {
    display: grid;
    gap: 0.25rem;
    font-size: 0.85rem;
  }

  .row {
    display: flex;
    gap: 0.4rem;
  }

  .row input {
    flex: 1;
    min-width: 0;
  }

  .empty,
  .error {
    margin: 0;
    font-size: 0.85rem;
    color: var(--muted);
  }

  .error {
    color: var(--danger);
    overflow-wrap: anywhere;
  }

  footer {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    padding-top: 0.35rem;
    border-top: 1px solid var(--border);
    color: var(--muted);
    font-size: 0.75rem;
  }

  kbd {
    font-family: inherit;
    color: var(--text);
  }
</style>
