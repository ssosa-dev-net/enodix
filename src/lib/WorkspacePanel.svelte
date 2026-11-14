<script lang="ts">
  import SnippetsPanel from "./SnippetsPanel.svelte";
  import TreeFolder from "./TreeFolder.svelte";
  import {
    canMoveInto,
    inventory,
    move,
    moveInto,
    newFilter,
    newFolder,
    newHost,
    openCsvImport,
    openNornirImport,
    remove,
    removeFilter,
    rename,
    targetFolder,
    trackFilterView,
    viewThrough,
    type Item,
  } from "./inventory.svelte";
  import { preferences } from "./preferences.svelte";
  import { profiles } from "./profiles.svelte";
  import { findHost, type Host } from "./workspace";

  interface Props {
    /** Opens the workspace switcher. */
    onopen: () => void;
    onconnect: (host: Host) => void;
    /** The session snippets go to, by name; null: none. */
    snippetTarget: string | null;
    onsnippet: (name: string) => void;
  }

  let { onopen, onconnect, snippetTarget, onsnippet }: Props = $props();

  const workspace = $derived(inventory.workspace);

  trackFilterView();

  /** The item under the pointer while the button is held. */
  let pressed: { item: Item; label: string; x: number; y: number } | null =
    null;
  /** Where the dragged item's label follows the pointer, once dragging. */
  let ghost = $state<{ label: string; x: number; y: number } | null>(null);
  let menu = $state<{ item: Item; x: number; y: number } | null>(null);

  function itemAt(target: EventTarget | null): Item | null {
    const row = (target as Element | null)?.closest<HTMLElement>("[data-item]");
    if (!row) return null;
    const path = row.dataset.path ?? "";
    return row.dataset.item === "host"
      ? { kind: "host", id: path }
      : { kind: "folder", path };
  }

  /** The folder under the pointer: a folder row, a host row's folder, or
   * the empty part of the tree for the top level. */
  function dropFolderAt(x: number, y: number): string | null {
    const element = document.elementFromPoint(x, y);
    return element?.closest<HTMLElement>("[data-drop]")?.dataset.drop ?? null;
  }

  function onpointerdown(event: PointerEvent) {
    if (event.button !== 0) return;
    const item = itemAt(event.target);
    const row = (event.target as Element).closest<HTMLElement>("[data-item]");
    if (!item || !row) return;
    pressed = {
      item,
      label: row.dataset.label ?? "",
      x: event.clientX,
      y: event.clientY,
    };
  }

  function onpointermove(event: PointerEvent) {
    if (!pressed) return;
    if (
      !ghost &&
      Math.hypot(event.clientX - pressed.x, event.clientY - pressed.y) < 5
    ) {
      return;
    }
    ghost = { label: pressed.label, x: event.clientX, y: event.clientY };
    const folder = dropFolderAt(event.clientX, event.clientY);
    inventory.dropTarget =
      folder !== null && canMoveInto(pressed.item, folder) ? folder : null;
  }

  function onpointerup() {
    const dragged = ghost !== null;
    const item = pressed?.item;
    const target = inventory.dropTarget;
    pressed = null;
    ghost = null;
    inventory.dropTarget = null;
    if (!dragged || !item) return;
    // The click that ends a drag is not a selection.
    const swallow = (event: Event) => {
      event.stopPropagation();
      event.preventDefault();
    };
    window.addEventListener("click", swallow, { capture: true, once: true });
    setTimeout(() => window.removeEventListener("click", swallow, true), 0);
    if (target !== null) moveInto(item, target);
  }

  function oncontextmenu(event: MouseEvent) {
    event.preventDefault();
    const item = itemAt(event.target) ?? { kind: "folder", path: "" };
    inventory.selected = item;
    menu = { item, x: event.clientX, y: event.clientY };
  }

  function ontreekeydown(event: KeyboardEvent) {
    const item = itemAt(event.target);
    if (!item) return;
    if (event.key === "F2") {
      event.preventDefault();
      rename(item);
    } else if (event.key === "Delete") {
      event.preventDefault();
      remove(item);
    }
  }

  function connect(item: Item) {
    const host =
      item.kind === "host" && workspace
        ? findHost(workspace.tree, item.id)
        : null;
    if (host) onconnect(host);
  }

  /** The menu's entries for an item. */
  function entries(item: Item): [string, () => void][] {
    if (item.kind === "host") {
      return [
        ["Connect", () => connect(item)],
        ["Rename…", () => rename(item)],
        ["Move to…", () => move(item)],
        ["Delete…", () => remove(item)],
      ];
    }
    const creating: [string, () => void][] = [
      ["New folder…", () => newFolder(item.path)],
      ["New host…", () => newHost(item.path)],
      ["Import CSV…", () => openCsvImport(item.path)],
      ["Import Nornir inventory…", () => openNornirImport(item.path)],
    ];
    if (item.path === "") return creating;
    return [
      ...creating,
      ["Rename…", () => rename(item)],
      ["Move to…", () => move(item)],
      ["Delete", () => remove(item)],
    ];
  }
</script>

<svelte:window
  {onpointermove}
  {onpointerup}
  onpointerdown={(event) => {
    if (menu && !(event.target as Element).closest(".menu")) menu = null;
  }}
  onkeydown={(event) => {
    if (event.key === "Escape") menu = null;
  }}
  onblur={() => (menu = null)}
/>

<aside>
  <header>
    <button
      class="name"
      title={workspace?.root}
      disabled={!workspace}
      class:selected={inventory.selected?.kind === "folder" &&
        inventory.selected.path === ""}
      onclick={() => (inventory.selected = { kind: "folder", path: "" })}
    >
      {workspace?.tree.name ?? "No workspace"}
    </button>
    <button
      data-open-workspaces
      title="Switch, create or add workspaces (Ctrl+Shift+O)"
      onclick={onopen}>Workspaces…</button
    >
    <button
      class="preferences"
      data-open-preferences
      title="Preferences: theme, terminal font, scrollback, copy and paste"
      aria-label="Preferences"
      onclick={() => (preferences.dialog = true)}
    >
      <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true">
        <path
          fill="currentColor"
          d="M9.4 1l.3 1.8c.4.1.8.3 1.1.5l1.5-1 1.4 1.4-1 1.5c.2.3.4.7.5 1.1L15 6.6v2l-1.8.3c-.1.4-.3.8-.5 1.1l1 1.5-1.4 1.4-1.5-1c-.3.2-.7.4-1.1.5L9.4 15h-2l-.3-1.8c-.4-.1-.8-.3-1.1-.5l-1.5 1-1.4-1.4 1-1.5c-.2-.3-.4-.7-.5-1.1L1.8 9.4v-2l1.8-.3c.1-.4.3-.8.5-1.1l-1-1.5 1.4-1.4 1.5 1c.3-.2.7-.4 1.1-.5L7.4 1h2zM8.4 5.5a2.5 2.5 0 100 5 2.5 2.5 0 000-5z"
        />
      </svg>
    </button>
  </header>

  {#if workspace}
    <div class="toolbar">
      <button title="New folder" onclick={() => newFolder(targetFolder())}
        >+ Folder</button
      >
      <button title="New host" onclick={() => newHost(targetFolder())}
        >+ Host</button
      >
      <button
        data-open-csv-import
        title="Import hosts from a CSV file into the selected folder"
        onclick={() => openCsvImport(targetFolder())}>Import…</button
      >
      <button
        data-open-nornir-import
        title="Import hosts from a Nornir inventory into the selected folder"
        onclick={() => openNornirImport(targetFolder())}>Nornir…</button
      >
      <button
        title="Credential profiles"
        onclick={() => (profiles.dialog = true)}
        >Profiles{profiles.view.missing.length > 0 ? " (!)" : ""}</button
      >
      <button
        class="search"
        title="Search hosts (Ctrl+K)"
        onclick={() => (inventory.palette = true)}
        >Search… <kbd>Ctrl+K</kbd></button
      >
    </div>
    <div class="filters">
      <select
        name="filter"
        aria-label="View"
        value={inventory.activeFilter ?? ""}
        onchange={(event) => viewThrough(event.currentTarget.value || null)}
      >
        <option value="">All hosts</option>
        {#each workspace.filters as filter (filter.name)}
          <option value={filter.name}>{filter.name}</option>
        {/each}
      </select>
      {#if inventory.activeFilter !== null}
        {@const name = inventory.activeFilter}
        <span class="count">
          {inventory.filterView
            ? inventory.filterView.hosts.size === 1
              ? "1 host"
              : `${inventory.filterView.hosts.size} hosts`
            : ""}
        </span>
        <button
          title="Delete this saved filter"
          onclick={() => removeFilter(name)}>Delete filter</button
        >
      {:else}
        <button title="Save a filter to the workspace" onclick={newFilter}
          >Save filter…</button
        >
      {/if}
    </div>
  {/if}

  {#if inventory.openError}
    <p class="error" role="alert">{inventory.openError}</p>
  {/if}

  {#if inventory.editError}
    <p class="error edit-error" role="alert">
      <span>{inventory.editError}</span>
      <button
        class="dismiss"
        aria-label="Dismiss"
        onclick={() => (inventory.editError = null)}>×</button
      >
    </p>
  {/if}

  {#if workspace}
    {#if workspace.problems.length > 0}
      <details class="problems" open>
        <summary>
          {workspace.problems.length === 1
            ? "1 file could not be loaded"
            : `${workspace.problems.length} files could not be loaded`}
        </summary>
        <ul>
          {#each workspace.problems as problem, index (index)}
            <li><strong>{problem.file}</strong>: {problem.message}</li>
          {/each}
        </ul>
      </details>
    {/if}

    <!-- The rows are buttons; the tree only delegates their pointer, menu
         and key events. -->
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <nav
      class="tree"
      class:drop-target={inventory.dropTarget === ""}
      aria-label="Inventory"
      data-drop=""
      {onpointerdown}
      {oncontextmenu}
      onkeydown={ontreekeydown}
    >
      <TreeFolder folder={workspace.tree} depth={0} {onconnect} />
      {#if inventory.filterView?.hosts.size === 0}
        <p class="hint">No host matches this filter.</p>
      {/if}
    </nav>
    <SnippetsPanel target={snippetTarget} onsend={onsnippet} />
  {:else if !inventory.openError}
    <p class="hint">
      Create a workspace or add a workspace folder (Workspaces…) to see its
      hosts.
    </p>
  {/if}
</aside>

{#if ghost}
  <div class="ghost" style:left="{ghost.x + 12}px" style:top="{ghost.y + 8}px">
    {ghost.label}
  </div>
{/if}

{#if menu}
  <div class="menu" role="menu" style:left="{menu.x}px" style:top="{menu.y}px">
    {#each entries(menu.item) as [label, action] (label)}
      <button
        role="menuitem"
        onclick={() => {
          menu = null;
          action();
        }}>{label}</button
      >
    {/each}
  </div>
{/if}

<style>
  aside {
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    border-right: 1px solid var(--border);
    font-size: 0.85rem;
  }

  header {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.4rem 0.6rem;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }

  header button {
    padding: 0.25rem 0.6rem;
  }

  header .preferences {
    display: inline-grid;
    place-items: center;
    padding: 0.3rem 0.4rem;
  }

  header .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
    text-align: left;
    border-color: transparent;
    background: none;
    padding-left: 0.2rem;
  }

  header .name:disabled {
    opacity: 1;
  }

  header .name.selected {
    box-shadow: inset 2px 0 var(--accent);
  }

  .toolbar {
    display: flex;
    /* The panel is at most 18rem wide: the buttons go on a second row
       rather than out of sight. */
    flex-wrap: wrap;
    gap: 0.4rem;
    padding: 0.35rem 0.6rem;
    border-bottom: 1px solid var(--border);
  }

  .toolbar button,
  .filters button {
    padding: 0.15rem 0.55rem;
    font-size: 0.8rem;
    white-space: nowrap;
  }

  .toolbar .search {
    margin-left: auto;
    color: var(--muted);
  }

  kbd {
    font-family: inherit;
    font-size: 0.7rem;
  }

  .filters {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.35rem 0.6rem;
    border-bottom: 1px solid var(--border);
  }

  .filters select {
    flex: 1;
    min-width: 0;
    font-size: 0.8rem;
  }

  .filters .count {
    color: var(--muted);
    font-size: 0.75rem;
    white-space: nowrap;
  }

  .tree {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0.3rem 0;
  }

  .tree.drop-target {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }

  .hint,
  .error {
    margin: 0.8rem 0.6rem;
    color: var(--muted);
  }

  .error {
    color: var(--danger);
    overflow-wrap: anywhere;
  }

  .edit-error {
    display: flex;
    align-items: flex-start;
    gap: 0.4rem;
    margin: 0.5rem 0.6rem 0;
    font-size: 0.8rem;
  }

  .edit-error span {
    flex: 1;
  }

  .dismiss {
    border: 0;
    background: none;
    padding: 0 0.3rem;
    color: var(--muted);
  }

  .problems {
    margin: 0.5rem 0.6rem 0;
    padding: 0.4rem 0.5rem;
    border: 1px solid var(--danger);
    border-radius: 4px;
    color: var(--text);
    font-size: 0.8rem;
  }

  .problems summary {
    cursor: pointer;
    color: var(--danger);
  }

  .problems ul {
    margin: 0.4rem 0 0;
    padding-left: 1rem;
    max-height: 10rem;
    overflow: auto;
    overflow-wrap: anywhere;
  }

  .ghost {
    position: fixed;
    z-index: 30;
    pointer-events: none;
    padding: 0.15rem 0.5rem;
    font-size: 0.8rem;
    background: var(--surface);
    border: 1px solid var(--accent);
    border-radius: 4px;
    opacity: 0.9;
  }

  .menu {
    position: fixed;
    z-index: 25;
    display: grid;
    min-width: 10rem;
    padding: 0.25rem 0;
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 4px;
    box-shadow: 0 6px 20px rgb(0 0 0 / 0.4);
  }

  .menu button {
    border: 0;
    border-radius: 0;
    background: none;
    text-align: left;
    padding: 0.3rem 0.9rem;
  }

  .menu button:hover {
    filter: none;
    background: var(--border);
  }
</style>
