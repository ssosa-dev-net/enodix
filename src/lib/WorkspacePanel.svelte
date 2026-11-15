<script lang="ts">
  import X from "@lucide/svelte/icons/x";
  import Settings from "@lucide/svelte/icons/settings";
  import SavedJobsPanel from "./SavedJobsPanel.svelte";
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
    openBackup,
    openEnrich,
    openCsvImport,
    openJob,
    openScript,
    openNornirImport,
    remove,
    removeFilter,
    rename,
    targetFolder,
    trackFilterView,
    viewThrough,
    type Item,
  } from "./inventory.svelte";
  import { openJobsView, runningCount } from "./jobs.svelte";
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

  /** The name the run-snippet dialog gives a folder. */
  function folderLabel(path: string): string {
    if (path === "") return workspace?.tree.name ?? "the workspace";
    return path.split("/").pop() ?? path;
  }

  /** Runs a snippet on the hosts the saved filter `name` shows. */
  function runOnFilter(name: string) {
    openJob({ kind: "filter", name }, `the hosts of ${name}`);
  }

  /** Runs a script for the hosts the saved filter `name` shows. */
  function scriptOnFilter(name: string) {
    openScript({ kind: "filter", name }, `the hosts of ${name}`);
  }

  /** Backs up the configs of the hosts the saved filter `name` shows. */
  function backUpFilter(name: string) {
    openBackup({ kind: "filter", name }, `the hosts of ${name}`);
  }

  /** The menu's entries for an item. */
  function entries(item: Item): [string, () => void][] {
    if (item.kind === "host") {
      const host = workspace ? findHost(workspace.tree, item.id) : null;
      return [
        ["Connect", () => connect(item)],
        [
          "Run snippet…",
          () =>
            openJob({ kind: "hosts", ids: [item.id] }, host?.name ?? item.id),
        ],
        [
          "Run script…",
          () =>
            openScript(
              { kind: "hosts", ids: [item.id] },
              host?.name ?? item.id,
            ),
        ],
        [
          "Back up config",
          () =>
            openBackup(
              { kind: "hosts", ids: [item.id] },
              host?.name ?? item.id,
            ),
        ],
        [
          "Fill in from device…",
          () =>
            openEnrich(
              { kind: "hosts", ids: [item.id] },
              host?.name ?? item.id,
            ),
        ],
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
      [
        "Run snippet…",
        () =>
          openJob({ kind: "folder", path: item.path }, folderLabel(item.path)),
      ],
      [
        "Run script…",
        () =>
          openScript(
            { kind: "folder", path: item.path },
            folderLabel(item.path),
          ),
      ],
      [
        "Back up configs",
        () =>
          openBackup(
            { kind: "folder", path: item.path },
            folderLabel(item.path),
          ),
      ],
      [
        "Fill in from devices…",
        () =>
          openEnrich(
            { kind: "folder", path: item.path },
            folderLabel(item.path),
          ),
      ],
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
      class="jobs"
      class:running={runningCount() > 0}
      data-open-jobs
      data-jobs-running={runningCount()}
      disabled={!workspace}
      title={(runningCount() > 0
        ? `${runningCount()} job${runningCount() === 1 ? "" : "s"} running: open the jobs view`
        : "Jobs: past runs, their output, search, compare and save") +
        (runningCount(true) > 0
          ? ` (${runningCount(true)} more running in other workspaces)`
          : "")}
      onclick={() => openJobsView()}
      >{#if runningCount() > 0}<span class="dot" aria-hidden="true"
        ></span>{runningCount()} running{:else}Jobs{/if}</button
    >
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
      <Settings size={15} aria-hidden="true" />
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
        data-open-platforms
        title="Platform definitions: how jobs drive each kind of device"
        onclick={() => (inventory.platforms = true)}>Platforms</button
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
          data-filter-run-snippet
          title="Run a saved snippet on the hosts this filter shows, as a job"
          disabled={!inventory.filterView?.hosts.size}
          onclick={() => runOnFilter(name)}>Run snippet…</button
        >
        <button
          data-filter-run-script
          title="Run a Python script of the workspace for the hosts this filter shows, as a job"
          disabled={!inventory.filterView?.hosts.size}
          onclick={() => scriptOnFilter(name)}>Run script…</button
        >
        <button
          data-filter-back-up
          title="Back up the configuration of the hosts this filter shows"
          disabled={!inventory.filterView?.hosts.size}
          onclick={() => backUpFilter(name)}>Back up configs</button
        >
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
        onclick={() => (inventory.editError = null)}
        ><X size={14} aria-hidden="true" /></button
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
    <SavedJobsPanel />
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
    background: var(--surface);
    border-right: 1px solid var(--border);
    font-size: var(--text-sm);
  }

  header {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-1-5) var(--space-2) var(--space-1-5) var(--space-3);
    border-bottom: 1px solid var(--border);
  }

  header button {
    padding: var(--space-1) var(--space-2);
    font-size: var(--text-xs);
    color: var(--muted);
  }

  header button:hover:not(:disabled) {
    color: var(--text);
  }

  header .preferences {
    display: inline-grid;
    place-items: center;
    padding: var(--space-1) var(--space-1-5);
  }

  header .jobs {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1-5);
    white-space: nowrap;
  }

  header .jobs.running {
    border-color: var(--accent);
    color: var(--accent);
  }

  header .jobs .dot {
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 50%;
    background: var(--accent);
    animation: pulse 1.2s ease-in-out infinite;
  }

  @keyframes pulse {
    50% {
      opacity: 0.3;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    header .jobs .dot {
      animation: none;
    }
  }

  header .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: var(--weight-semibold);
    font-size: var(--text-sm);
    color: var(--text);
    text-align: left;
    border-color: transparent;
    background: none;
    padding-left: var(--space-1);
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
    gap: var(--space-1-5);
    padding: var(--space-1-5) var(--space-2);
    border-bottom: 1px solid var(--border);
  }

  .toolbar button,
  .filters button {
    padding: var(--space-0-5) var(--space-2);
    font-size: var(--text-xs);
    white-space: nowrap;
  }

  .toolbar .search {
    margin-left: auto;
    color: var(--muted);
  }

  .filters {
    display: flex;
    align-items: center;
    gap: var(--space-1-5);
    padding: var(--space-1-5) var(--space-2);
    border-bottom: 1px solid var(--border);
  }

  .filters select {
    flex: 1;
    min-width: 0;
    font-size: var(--text-xs);
  }

  .filters .count {
    color: var(--muted);
    font-size: var(--text-xs);
    white-space: nowrap;
  }

  .tree {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: var(--space-1) 0;
  }

  .tree.drop-target {
    background: color-mix(in srgb, var(--accent) 12%, transparent);
  }

  .hint,
  .error {
    margin: var(--space-3) var(--space-2);
    color: var(--muted);
  }

  .error {
    color: var(--danger);
    overflow-wrap: anywhere;
  }

  .edit-error {
    display: flex;
    align-items: flex-start;
    gap: var(--space-1-5);
    margin: var(--space-2) var(--space-2) 0;
    font-size: var(--text-xs);
  }

  .edit-error span {
    flex: 1;
  }

  .dismiss {
    border: 0;
    background: none;
    padding: 0 var(--space-1);
    color: var(--muted);
  }

  .problems {
    margin: var(--space-2) var(--space-2) 0;
    padding: var(--space-1-5) var(--space-2);
    background: var(--warning-bg);
    border: 1px solid var(--warning-border);
    border-radius: var(--radius-md);
    color: var(--text);
    font-size: var(--text-xs);
  }

  .problems summary {
    cursor: pointer;
    color: var(--warning);
  }

  .problems ul {
    margin: var(--space-1-5) 0 0;
    padding-left: var(--space-4);
    max-height: 10rem;
    overflow: auto;
    overflow-wrap: anywhere;
  }

  .ghost {
    position: fixed;
    z-index: 30;
    pointer-events: none;
    padding: var(--space-0-5) var(--space-2);
    font-size: var(--text-xs);
    background: var(--surface);
    border: 1px solid var(--accent);
    border-radius: var(--radius-sm);
    opacity: 0.9;
  }

  .menu {
    position: fixed;
    z-index: 25;
    display: grid;
    min-width: 10rem;
    padding: var(--space-1);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow);
  }

  .menu button {
    border: 0;
    border-radius: var(--radius-sm);
    background: none;
    text-align: left;
    padding: var(--space-1-5) var(--space-2-5);
  }

  .menu button:hover:not(:disabled) {
    background: var(--selected);
  }
</style>
