<script lang="ts">
  // The Hosts side view: the workspace's name (opening the switcher), a
  // filter field with the saved filters as chips, the host tree, one add
  // control and a line for the workspace's problems at the foot.
  import Archive from "@lucide/svelte/icons/archive";
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import Cpu from "@lucide/svelte/icons/cpu";
  import FileCode from "@lucide/svelte/icons/file-code";
  import Play from "@lucide/svelte/icons/play";
  import Plus from "@lucide/svelte/icons/plus";
  import Search from "@lucide/svelte/icons/search";
  import SlidersHorizontal from "@lucide/svelte/icons/sliders-horizontal";
  import Trash from "@lucide/svelte/icons/trash";
  import Upload from "@lucide/svelte/icons/upload";
  import TriangleAlert from "@lucide/svelte/icons/triangle-exclamation-point";
  import X from "@lucide/svelte/icons/x";
  import TreeFolder from "./TreeFolder.svelte";
  import { openView } from "./views";
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
    openConfigHistory,
    remove,
    removeFilter,
    rename,
    targetFolder,
    viewThrough,
    type Item,
  } from "./inventory.svelte";
  import {
    allHosts,
    findHost,
    openLogFolder,
    resolveSettings,
    type EffectiveSettings,
    type Host,
    type Protocol,
  } from "./workspace";
  import { otherWays, ownWayBlocked, PROTOCOL_NAMES } from "./ways";

  interface Props {
    /** Opens the workspace switcher. */
    onopen: () => void;
    onconnect: (host: Host, protocol?: Protocol) => void;
    /** Opens the details sheet on an item. */
    ondetails: (item: Item) => void;
  }

  let { onopen, onconnect, ondetails }: Props = $props();

  const workspace = $derived(inventory.workspace);
  /** Whether the open workspace holds no host at all (folders aside). */
  const empty = $derived(!!workspace && allHosts(workspace.tree).length === 0);

  /** Whether the list of workspace problems is open. */
  let problemsOpen = $state(false);

  /** The item under the pointer while the button is held. */
  let pressed: { item: Item; label: string; x: number; y: number } | null =
    null;
  /** Where the dragged item's label follows the pointer, once dragging. */
  let ghost = $state<{ label: string; x: number; y: number } | null>(null);
  let menu = $state<{
    item: Item;
    x: number;
    y: number;
    /** A host's settings, once the core gave them (for its ways in). */
    settings: EffectiveSettings | null;
  } | null>(null);
  /** Counts the context menus opened, so that a late answer for an
   * earlier one is dropped. */
  let menus = 0;
  /** Where the add control's menu is open, if it is. */
  let adding = $state<{ x: number; y: number } | null>(null);

  function openAdd(event: MouseEvent) {
    if (adding) {
      adding = null;
      return;
    }
    const box = (event.currentTarget as HTMLElement).getBoundingClientRect();
    adding = { x: box.left, y: box.bottom + 4 };
  }

  /** The add control's entries: what it creates, in the selected folder
   * (or the selected host's). */
  const additions: [string, string, () => void][] = [
    ["host", "New host…", () => newHost(targetFolder())],
    ["folder", "New folder…", () => newFolder(targetFolder())],
    ["csv", "Import from CSV…", () => openCsvImport(targetFolder())],
    ["nornir", "Import from Nornir…", () => openNornirImport(targetFolder())],
  ];

  /** Applies the saved filter `name`, or clears it when it is applied. */
  function toggleFilter(name: string) {
    viewThrough(inventory.activeFilter === name ? null : name);
  }

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
    menu = { item, x: event.clientX, y: event.clientY, settings: null };
    const opened = ++menus;
    if (item.kind === "host") {
      resolveSettings(item)
        .then((settings) => {
          if (menu && menus === opened) menu.settings = settings;
        })
        .catch(() => {
          // The ways in stay offered; connecting says what is wrong.
        });
    }
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

  function connect(item: Item, protocol?: Protocol) {
    const host =
      item.kind === "host" && workspace
        ? findHost(workspace.tree, item.id)
        : null;
    if (host) onconnect(host, protocol);
  }

  /** The name a job tab gives a folder. */
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

  /** An entry of the tree's context menu: `id` is its `data-menu`. */
  interface Entry {
    id: string;
    label: string;
    action: () => void;
    /** Why it cannot be used now, if it cannot. */
    blocked?: string | null;
  }

  /** The menu's entries for an item: every action of its details too. */
  function entries(item: Item, settings: EffectiveSettings | null): Entry[] {
    if (item.kind === "host") {
      const host = workspace ? findHost(workspace.tree, item.id) : null;
      const name = host?.name ?? item.id;
      const target = { kind: "hosts" as const, ids: [item.id] };
      return [
        {
          id: "connect",
          label: "Connect",
          action: () => connect(item),
          blocked: ownWayBlocked(settings),
        },
        ...otherWays(item.id, settings).map((way) => ({
          id: `connect-${way.protocol}`,
          label: `Connect by ${PROTOCOL_NAMES[way.protocol]}`,
          action: () => connect(item, way.protocol),
          blocked: way.blocked,
        })),
        { id: "details", label: "Details", action: () => ondetails(item) },
        {
          id: "run-snippet",
          label: "Run snippet…",
          action: () => openJob(target, name),
        },
        {
          id: "run-script",
          label: "Run script…",
          action: () => openScript(target, name),
        },
        {
          id: "back-up",
          label: "Back up config",
          action: () => openBackup(target, name),
        },
        {
          id: "enrich",
          label: "Fill in from device…",
          action: () => openEnrich(target, name),
        },
        {
          id: "config-history",
          label: "Config history",
          action: () => openConfigHistory(item.id),
        },
        {
          id: "log-folder",
          label: "Open log folder",
          action: () =>
            openLogFolder(item.id).catch(
              (error) => (inventory.editError = String(error)),
            ),
        },
        { id: "rename", label: "Rename…", action: () => rename(item) },
        { id: "move", label: "Move to…", action: () => move(item) },
        { id: "delete", label: "Delete…", action: () => remove(item) },
      ];
    }
    const target = { kind: "folder" as const, path: item.path };
    const label = folderLabel(item.path);
    const creating: Entry[] = [
      { id: "details", label: "Details", action: () => ondetails(item) },
      {
        id: "new-folder",
        label: "New folder…",
        action: () => newFolder(item.path),
      },
      { id: "new-host", label: "New host…", action: () => newHost(item.path) },
      {
        id: "import-csv",
        label: "Import CSV…",
        action: () => openCsvImport(item.path),
      },
      {
        id: "import-nornir",
        label: "Import Nornir inventory…",
        action: () => openNornirImport(item.path),
      },
      {
        id: "run-snippet",
        label: "Run snippet…",
        action: () => openJob(target, label),
      },
      {
        id: "run-script",
        label: "Run script…",
        action: () => openScript(target, label),
      },
      {
        id: "back-up",
        label: "Back up configs",
        action: () => openBackup(target, label),
      },
      {
        id: "enrich",
        label: "Fill in from devices…",
        action: () => openEnrich(target, label),
      },
    ];
    if (item.path === "") return creating;
    return [
      ...creating,
      { id: "rename", label: "Rename…", action: () => rename(item) },
      { id: "move", label: "Move to…", action: () => move(item) },
      { id: "delete", label: "Delete", action: () => remove(item) },
    ];
  }
</script>

<svelte:window
  {onpointermove}
  {onpointerup}
  onpointerdown={(event) => {
    const target = event.target as Element;
    if (menu && !target.closest(".menu")) menu = null;
    if (adding && !target.closest(".menu, [data-add-menu]")) adding = null;
  }}
  onkeydown={(event) => {
    if (event.key === "Escape") {
      menu = null;
      adding = null;
    }
  }}
  onblur={() => {
    menu = null;
    adding = null;
  }}
/>

<div class="hosts">
  <header data-window-bar data-tauri-drag-region="deep">
    <button
      class="name"
      data-open-workspaces
      title={`${workspace?.root ?? "No workspace open"}: switch, create or add workspaces (Ctrl+Shift+O)`}
      onclick={onopen}
    >
      <span>{workspace?.tree.name ?? "No workspace"}</span>
      <ChevronDown size={14} aria-hidden="true" />
    </button>
    <button
      class="tool"
      aria-label="Search hosts"
      title="Search hosts and snippets (Ctrl+K)"
      disabled={!workspace}
      onclick={() => (inventory.palette = true)}
      ><Search size={16} aria-hidden="true" /></button
    >
    <button
      class="tool"
      data-workspace-settings
      aria-label="Workspace settings"
      aria-pressed={inventory.selected?.kind === "folder" &&
        inventory.selected.path === ""}
      title="Settings of the whole workspace (its top folder)"
      disabled={!workspace}
      onclick={() => (inventory.selected = { kind: "folder", path: "" })}
      ><SlidersHorizontal size={16} aria-hidden="true" /></button
    >
    <button
      class="tool"
      data-open-platforms
      aria-label="Platform definitions"
      title="Platform definitions: how jobs drive each kind of device"
      disabled={!workspace}
      onclick={() => openView("platforms")}
      ><Cpu size={16} aria-hidden="true" /></button
    >
    <button
      class="tool"
      data-add-menu
      aria-label="Add"
      aria-haspopup="menu"
      aria-expanded={adding !== null}
      title="Add a host or folder, or import hosts, in the selected folder"
      disabled={!workspace}
      onclick={openAdd}><Plus size={16} aria-hidden="true" /></button
    >
  </header>

  {#if workspace}
    <div class="filtering">
      <label class="field">
        <Search size={14} aria-hidden="true" />
        <input
          name="host-filter"
          data-host-filter
          type="search"
          placeholder="Filter hosts"
          aria-label="Filter hosts"
          autocomplete="off"
          spellcheck="false"
          bind:value={inventory.query}
          onkeydown={(event) => {
            if (event.key === "Escape" && inventory.query) {
              event.stopPropagation();
              inventory.query = "";
            }
          }}
        />
        {#if inventory.query}
          <button
            class="clear"
            aria-label="Clear the filter"
            title="Clear the filter"
            onclick={() => (inventory.query = "")}
            ><X size={14} aria-hidden="true" /></button
          >
        {/if}
      </label>
      <div class="chips" data-filter-chips>
        {#each workspace.filters as filter (filter.name)}
          <button
            class="chip"
            data-filter-chip={filter.name}
            aria-pressed={inventory.activeFilter === filter.name}
            title={inventory.activeFilter === filter.name
              ? `Showing the hosts of ${filter.name}: click to show all`
              : `Show only the hosts of the saved filter ${filter.name}`}
            onclick={() => toggleFilter(filter.name)}>{filter.name}</button
          >
        {/each}
        {#if inventory.activeFilter === null}
          <button
            class="chip save"
            data-save-filter
            title={inventory.query.trim()
              ? "Save these words as a filter in the workspace"
              : "Save a filter to the workspace"}
            onclick={() => newFilter(inventory.query.trim())}
            ><Plus size={12} aria-hidden="true" />Save filter…</button
          >
        {/if}
      </div>
      {#if inventory.activeFilter !== null}
        {@const name = inventory.activeFilter}
        <div class="filter-actions" data-filter-actions>
          <span class="count">
            {inventory.filterView
              ? inventory.filterView.hosts.size === 1
                ? "1 host"
                : `${inventory.filterView.hosts.size} hosts`
              : ""}
          </span>
          <button
            class="tool"
            data-filter-run-snippet
            aria-label="Run snippet"
            title="Run a saved snippet on the hosts this filter shows, as a job"
            disabled={!inventory.filterView?.hosts.size}
            onclick={() => runOnFilter(name)}
            ><Play size={15} aria-hidden="true" /></button
          >
          <button
            class="tool"
            data-filter-run-script
            aria-label="Run script"
            title="Run a Python script of the workspace for the hosts this filter shows, as a job"
            disabled={!inventory.filterView?.hosts.size}
            onclick={() => scriptOnFilter(name)}
            ><FileCode size={15} aria-hidden="true" /></button
          >
          <button
            class="tool"
            data-filter-back-up
            aria-label="Back up configs"
            title="Back up the configuration of the hosts this filter shows"
            disabled={!inventory.filterView?.hosts.size}
            onclick={() => backUpFilter(name)}
            ><Archive size={15} aria-hidden="true" /></button
          >
          <button
            class="tool"
            data-filter-delete
            aria-label="Delete filter"
            title="Delete this saved filter from the workspace"
            onclick={() => removeFilter(name)}
            ><Trash size={15} aria-hidden="true" /></button
          >
        </div>
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
    {#if empty}
      <div class="empty" data-hosts-empty>
        <p>
          No hosts yet. Each host is a small file in the workspace folder: add
          one here, or import a list from a CSV file or a Nornir inventory.
        </p>
        {#each additions.filter(([kind]) => kind !== "folder") as [kind, label, action] (kind)}
          <button data-hosts-empty-add={kind} onclick={action}>
            {#if kind === "host"}<Plus
                size={14}
                aria-hidden="true"
              />{:else}<Upload size={14} aria-hidden="true" />{/if}
            {label}
          </button>
        {/each}
      </div>
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
      <TreeFolder
        folder={workspace.tree}
        depth={0}
        onconnect={(host) => onconnect(host)}
        {ondetails}
      />
      {#if inventory.filterView?.hosts.size === 0 && !empty}
        <p class="hint">No host matches this filter.</p>
      {/if}
    </nav>

    {#if workspace.problems.length > 0}
      <footer class="problems">
        {#if problemsOpen}
          <ul data-problems-list>
            {#each workspace.problems as problem, index (index)}
              <li><strong>{problem.file}</strong>: {problem.message}</li>
            {/each}
          </ul>
        {/if}
        <button
          class="problems-line"
          data-problems={workspace.problems.length}
          aria-expanded={problemsOpen}
          title={problemsOpen ? "Hide the list" : "Show which files and why"}
          onclick={() => (problemsOpen = !problemsOpen)}
        >
          <TriangleAlert size={14} aria-hidden="true" />
          {workspace.problems.length === 1
            ? "1 file could not be loaded"
            : `${workspace.problems.length} files could not be loaded`}
        </button>
      </footer>
    {/if}
  {:else if !inventory.openError}
    <p class="hint" data-needs-workspace>
      No workspace is open. Open or create one from the welcome view, or by the
      name above, to see its hosts here.
    </p>
  {/if}
</div>

{#if ghost}
  <div class="ghost" style:left="{ghost.x + 12}px" style:top="{ghost.y + 8}px">
    {ghost.label}
  </div>
{/if}

{#if adding}
  <div
    class="menu"
    role="menu"
    data-add-entries
    style:left="{adding.x}px"
    style:top="{adding.y}px"
  >
    {#each additions as [kind, label, action] (kind)}
      <button
        role="menuitem"
        data-add={kind}
        data-open-csv-import={kind === "csv" || undefined}
        data-open-nornir-import={kind === "nornir" || undefined}
        onclick={() => {
          adding = null;
          action();
        }}>{label}</button
      >
    {/each}
  </div>
{/if}

{#if menu}
  <div class="menu" role="menu" style:left="{menu.x}px" style:top="{menu.y}px">
    {#each entries(menu.item, menu.settings) as entry (entry.id)}
      <button
        role="menuitem"
        data-menu={entry.id}
        disabled={Boolean(entry.blocked)}
        title={entry.blocked ?? undefined}
        onclick={() => {
          menu = null;
          entry.action();
        }}>{entry.label}</button
      >
    {/each}
  </div>
{/if}

<style>
  .hosts {
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    font-size: var(--text-sm);
  }

  header {
    display: flex;
    align-items: center;
    gap: var(--space-0-5);
    padding: var(--space-1-5) var(--space-1-5) var(--space-1-5) var(--space-2);
    border-bottom: 1px solid var(--border);
  }

  header .name {
    flex: 1;
    min-width: 0;
    display: flex;
    align-items: center;
    gap: var(--space-1);
    border-color: transparent;
    background: none;
    padding: var(--space-1) var(--space-1-5);
    font-weight: var(--weight-semibold);
    font-size: var(--text-sm);
    color: var(--text);
    text-align: left;
  }

  header .name span {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  header .name :global(.lucide-icon) {
    flex: none;
    color: var(--muted);
  }

  .tool {
    flex: none;
    display: inline-grid;
    place-items: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border-color: transparent;
    background: none;
    color: var(--muted);
  }

  .tool:hover:not(:disabled),
  .tool[aria-pressed="true"],
  .tool[aria-expanded="true"] {
    color: var(--text);
    background: var(--surface-raised);
  }

  .filtering {
    display: grid;
    gap: var(--space-1-5);
    padding: var(--space-2);
    border-bottom: 1px solid var(--border);
  }

  .field {
    display: flex;
    align-items: center;
    gap: var(--space-1-5);
    padding: 0 var(--space-1-5) 0 var(--space-2);
    background: var(--input);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-md);
    color: var(--muted);
  }

  .field:focus-within {
    border-color: var(--accent);
  }

  .field input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: none;
    padding: var(--space-1) 0;
    font-size: var(--text-sm);
    outline: none;
  }

  .field input::-webkit-search-cancel-button {
    display: none;
  }

  .field .clear {
    display: inline-grid;
    place-items: center;
    padding: var(--space-0-5);
    border: 0;
    background: none;
    color: var(--muted);
  }

  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
  }

  .chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-0-5);
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    padding: var(--space-0-5) var(--space-2);
    border-radius: var(--radius-full);
    font-size: var(--text-xs);
    color: var(--muted);
  }

  /* Not on a pressed chip, whose text is on the accent (#94: under the
     mouse it was unreadable). */
  .chip:hover:not(:disabled, [aria-pressed="true"]) {
    color: var(--text);
  }

  .chip[aria-pressed="true"] {
    color: var(--on-accent);
    background: var(--accent);
    border-color: var(--accent);
  }

  .chip.save {
    border-style: dashed;
  }

  .filter-actions {
    display: flex;
    align-items: center;
    gap: var(--space-0-5);
  }

  .filter-actions .count {
    flex: 1;
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

  .empty {
    display: grid;
    justify-items: start;
    gap: var(--space-2);
    padding: var(--space-3);
    color: var(--muted);
    font-size: var(--text-xs);
  }

  .empty p {
    margin: 0;
  }

  .empty button {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
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
    border-top: 1px solid var(--warning-border);
    background: var(--warning-bg);
    font-size: var(--text-xs);
  }

  .problems-line {
    display: flex;
    align-items: center;
    gap: var(--space-1-5);
    width: 100%;
    border: 0;
    border-radius: 0;
    background: none;
    padding: var(--space-1-5) var(--space-2-5);
    color: var(--warning);
    text-align: left;
  }

  .problems ul {
    margin: 0;
    padding: var(--space-1-5) var(--space-2-5) 0 var(--space-5);
    max-height: 10rem;
    overflow: auto;
    overflow-wrap: anywhere;
    color: var(--text);
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
