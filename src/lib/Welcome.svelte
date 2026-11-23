<script lang="ts">
  // The first window: no workspace is open. It lists the workspaces the app
  // knows (a folder that is gone is marked, and can be taken off the list),
  // opens an existing workspace folder or creates one, and quick connects
  // without any workspace.
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import Folder from "@lucide/svelte/icons/folder";
  import X from "@lucide/svelte/icons/x";
  import QuickConnect from "./QuickConnect.svelte";
  import type { QuickConnectTarget, SerialParameters } from "./session";
  import {
    createWorkspace,
    forgetWorkspace,
    listWorkspaces,
    pickWorkspaceFolder,
    type KnownWorkspace,
    type WorkspaceView,
  } from "./workspace";

  interface Props {
    /** The app's name, the view's title. */
    name: string;
    /** Opens the workspace `open` gives; resolves to why it failed, or null. */
    onswitch: (open: () => Promise<WorkspaceView>) => Promise<string | null>;
    /** Opens the folder at `path`, asking first about a folder of other
     * things (#131); resolves to why it failed, or null. */
    onopen: (path: string) => Promise<string | null>;
    onconnect: (target: QuickConnectTarget) => void;
    onserial: (port: string, parameters: SerialParameters) => void;
  }

  let { name, onswitch, onopen, onconnect, onserial }: Props = $props();

  /** null until the list came. */
  let known = $state<KnownWorkspace[] | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let openFolder = $state("");
  let createFolder = $state("");

  listWorkspaces().then(
    (list) => (known = list),
    (reason) => {
      known = [];
      error = String(reason);
    },
  );

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

  async function forget(workspace: KnownWorkspace) {
    error = null;
    try {
      known = await forgetWorkspace(workspace.path);
    } catch (reason) {
      error = String(reason);
    }
  }

  async function browse(which: "open" | "create") {
    const folder = await pickWorkspaceFolder(
      which === "create"
        ? "Folder for the new workspace"
        : "Existing workspace folder",
    );
    if (folder === null) return;
    if (which === "create") createFolder = folder;
    else openFolder = folder;
  }

  // Several workspaces may share a folder name; the path tells them apart.
  const shared = $derived.by(() => {
    const counts = new Map<string, number>();
    for (const workspace of known ?? []) {
      const key = workspace.name.toLowerCase();
      counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    return counts;
  });
</script>

<div class="welcome" data-welcome>
  <header>
    <h1>{name}</h1>
    <p>
      A workspace is a folder of host files, one per device, that your team can
      keep in git. Open one to see its hosts, or connect to a device right away.
    </p>
  </header>

  <div class="columns">
    <section class="card" data-welcome-workspaces aria-labelledby="welcome-ws">
      <h2 id="welcome-ws">Workspaces</h2>
      {#if known === null}
        <p class="hint">Loading…</p>
      {:else if known.length === 0}
        <p class="hint" data-welcome-none>
          No workspace yet. Create one in an empty folder, or open a folder that
          already holds host files.
        </p>
      {:else}
        <ul>
          {#each known as workspace (workspace.path)}
            {@const several =
              (shared.get(workspace.name.toLowerCase()) ?? 0) > 1}
            <li class:missing={!workspace.available}>
              <button
                type="button"
                class="workspace"
                data-welcome-workspace={workspace.path}
                data-unavailable={!workspace.available || undefined}
                disabled={!workspace.available || busy}
                title={workspace.available
                  ? `Open ${workspace.path}`
                  : `${workspace.path} is not there (a disconnected drive, or moved)`}
                onclick={() => void run(() => onopen(workspace.path))}
              >
                <Folder size={16} aria-hidden="true" />
                <span class="name">{workspace.name}</span>
                {#if !workspace.available}
                  <span class="tag">folder not found</span>
                {/if}
                <span class="path" class:shown={several || !workspace.available}
                  >{workspace.path}</span
                >
              </button>
              <button
                type="button"
                class="forget"
                data-welcome-forget
                aria-label={`Remove ${workspace.name} from the list`}
                title="Remove from this list (the folder is not deleted)"
                onclick={() => void forget(workspace)}
                ><X size={14} aria-hidden="true" /></button
              >
            </li>
          {/each}
        </ul>
      {/if}

      <form
        data-welcome-form="open"
        onsubmit={(event) => {
          event.preventDefault();
          const folder = openFolder.trim();
          if (folder) void run(() => onopen(folder));
        }}
      >
        <label for="welcome-open">
          <FolderOpen size={14} aria-hidden="true" /> Open a workspace folder
        </label>
        <div class="line">
          <input
            id="welcome-open"
            name="welcome-folder"
            bind:value={openFolder}
            placeholder="Folder holding host files"
            autocomplete="off"
            spellcheck="false"
          />
          <button type="button" onclick={() => void browse("open")}
            >Browse…</button
          >
          <button type="submit" disabled={busy || !openFolder.trim()}
            >Open</button
          >
        </div>
      </form>

      <form
        data-welcome-form="create"
        onsubmit={(event) => {
          event.preventDefault();
          const folder = createFolder.trim();
          if (folder) void run(() => onswitch(() => createWorkspace(folder)));
        }}
      >
        <label for="welcome-create">
          <FolderPlus size={14} aria-hidden="true" /> Create a workspace
        </label>
        <div class="line">
          <input
            id="welcome-create"
            name="welcome-folder"
            bind:value={createFolder}
            placeholder="An empty or new folder"
            autocomplete="off"
            spellcheck="false"
          />
          <button type="button" onclick={() => void browse("create")}
            >Browse…</button
          >
          <button type="submit" disabled={busy || !createFolder.trim()}
            >Create</button
          >
        </div>
      </form>

      {#if error}
        <p class="error" role="alert" data-welcome-error>{error}</p>
      {/if}
    </section>

    <section class="card">
      <QuickConnect {name} embedded {onconnect} {onserial} />
    </section>
  </div>
</div>

<style>
  .welcome {
    height: 100%;
    overflow: auto;
    display: grid;
    align-content: center;
    justify-items: center;
    gap: var(--space-6);
    padding: var(--space-8) var(--space-6);
  }

  header {
    width: min(54rem, 100%);
    display: grid;
    gap: var(--space-2);
  }

  h1 {
    margin: 0;
    font-size: var(--text-2xl);
    font-weight: var(--weight-semibold);
  }

  header p {
    margin: 0;
    max-width: 40rem;
    color: var(--muted);
  }

  .columns {
    width: min(54rem, 100%);
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(20rem, 1fr));
    gap: var(--space-4);
    align-items: start;
  }

  .card {
    display: grid;
    gap: var(--space-4);
    padding: var(--space-5);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    min-width: 0;
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
  }

  .hint {
    margin: 0;
    color: var(--muted);
    font-size: var(--text-sm);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--space-0-5);
  }

  li {
    display: flex;
    align-items: center;
    gap: var(--space-1);
  }

  .workspace {
    flex: 1;
    min-width: 0;
    display: grid;
    grid-template-columns: auto minmax(0, 1fr) auto;
    column-gap: var(--space-2);
    row-gap: 0;
    align-items: center;
    text-align: start;
    background: transparent;
    border-color: transparent;
  }

  .workspace:hover:not(:disabled) {
    background: var(--selected);
  }

  .workspace .name {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .workspace .path {
    display: none;
    grid-column: 2 / -1;
    color: var(--muted);
    font-size: var(--text-2xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .workspace .path.shown {
    display: block;
  }

  .missing .name {
    color: var(--muted);
  }

  .tag {
    color: var(--warning);
    font-size: var(--text-2xs);
  }

  .forget {
    width: var(--control-height);
    height: var(--control-height);
    padding: 0;
    background: transparent;
    border-color: transparent;
    color: var(--muted);
  }

  form {
    display: grid;
    gap: var(--space-1);
  }

  label {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .line {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1-5);
  }

  .line input {
    flex: 1 1 12rem;
    min-width: 0;
  }

  .error {
    margin: 0;
    color: var(--danger);
    font-size: var(--text-sm);
    overflow-wrap: anywhere;
  }
</style>
