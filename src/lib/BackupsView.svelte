<script lang="ts">
  // The Config backups side view (#70): the workspace's hosts that have
  // kept configurations, in tree order, each with when it was last backed
  // up and whether that backup changed anything. A host opens its config
  // history in a tab. It refetches whenever a backup keeps or checks a
  // configuration. With no backups yet it says how they are made and
  // offers to back up a folder.
  import { backedUpHosts, versionTime, type BackedUpHost } from "./jobs";
  import { liveJobs } from "./jobs.svelte";
  import { inventory, openBackup, openConfigHistory } from "./inventory.svelte";
  import { findHost, type Folder } from "./workspace";

  const workspace = $derived(inventory.workspace);

  let hosts = $state<BackedUpHost[] | null>(null);
  let error = $state<string | null>(null);
  /** The folder the empty view offers to back up ("" = the workspace). */
  let folder = $state("");

  $effect(() => {
    void liveJobs.configs;
    // The tree too: a host renamed, moved or gone.
    void inventory.workspace?.revision;
    if (!inventory.workspace?.root) {
      hosts = null;
      return;
    }
    backedUpHosts().then(
      (list) => {
        hosts = list;
        error = null;
      },
      (reason) => (error = String(reason)),
    );
  });

  /** Every folder of the tree, the root first, as paths. */
  function folders(tree: Folder): Folder[] {
    return [tree, ...tree.folders.flatMap(folders)];
  }

  const LAST = {
    first: "first backup",
    changed: "changed",
    unchanged: "unchanged",
  } as const;

  const LAST_TITLES = {
    first: "The last backup kept this host's first version",
    changed: "The last backup kept a version that differs from the one before",
    unchanged: "The last backup found the configuration unchanged",
  } as const;

  function nameOf(id: string) {
    return (workspace && findHost(workspace.tree, id)?.name) ?? id;
  }

  /** The host's folder path, for telling hosts of one name apart. */
  function folderOf(id: string) {
    const at = id.lastIndexOf("/");
    return at < 0 ? "" : id.slice(0, at);
  }

  function backUp() {
    const tree = workspace?.tree;
    if (!tree) return;
    const chosen = folders(tree).find((f) => f.path === folder) ?? tree;
    openBackup({ kind: "folder", path: chosen.path }, chosen.name);
  }
</script>

<div class="backups" data-backups-view>
  <header data-window-bar data-tauri-drag-region="deep">
    <h2>Config backups</h2>
  </header>

  {#if !workspace}
    <p class="hint" data-needs-workspace>
      Config backups are kept per workspace: open or create one to see its hosts
      backed up here.
    </p>
  {:else if hosts === null}
    <p class="hint">{error ?? "Loading…"}</p>
  {:else if hosts.length === 0}
    <div class="empty" data-backups-empty>
      <p class="hint">
        No host of this workspace has been backed up yet. A backup fetches each
        device's configuration with its platform's show-config command and keeps
        a version whenever it changed; run one on a folder, a saved filter or a
        host from its menu in Hosts, or turn on "Back up on login" in a host's
        settings.
      </p>
      <div class="row">
        <select
          name="backups-folder"
          aria-label="Folder to back up"
          bind:value={folder}
        >
          {#each folders(workspace.tree) as entry (entry.path)}
            <option value={entry.path}
              >{entry.path === ""
                ? `${entry.name} (all hosts)`
                : entry.path}</option
            >
          {/each}
        </select>
        <button
          data-backups-back-up
          title="Back up the configuration of every host in this folder"
          onclick={backUp}>Back up…</button
        >
      </div>
    </div>
  {:else}
    <ul aria-label="Hosts with backups" data-backups={hosts.length}>
      {#each hosts as entry (entry.host)}
        <li>
          <button
            class="host"
            data-backup-host={entry.host}
            data-last={entry.last}
            data-versions={entry.versions}
            data-newest={entry.newest}
            title="Open this host's config history in a tab"
            onclick={() => openConfigHistory(entry.host)}
          >
            <span class="title">{nameOf(entry.host)}</span>
            {#if folderOf(entry.host)}
              <span class="meta">{folderOf(entry.host)}</span>
            {/if}
            <span class="meta">
              {versionTime(entry.lastBackup)} ·
              <span
                class="last {entry.last}"
                data-last-backup={entry.last}
                title={LAST_TITLES[entry.last]}>{LAST[entry.last]}</span
              >
              · {entry.versions}
              {entry.versions === 1 ? "version" : "versions"}
            </span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
  {#if error && hosts !== null}
    <p class="error" role="alert" data-backups-error>{error}</p>
  {/if}
</div>

<style>
  .backups {
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    font-size: var(--text-sm);
  }

  header {
    padding: var(--space-2) var(--space-3);
    border-bottom: 1px solid var(--border);
  }

  h2 {
    margin: 0;
    font-size: var(--text-lg);
    font-weight: var(--weight-semibold);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: var(--space-1-5) 0;
    overflow: auto;
  }

  li {
    display: flex;
    padding: 0 var(--space-2) 0 var(--space-1-5);
  }

  .host {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    text-align: left;
    border-color: transparent;
    background: none;
    gap: 0;
    min-height: 0;
    padding: var(--space-0-5) var(--space-1-5);
  }

  .host:hover {
    background: var(--surface-raised);
  }

  .host > span {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .title {
    font-weight: var(--weight-semibold);
  }

  .meta {
    color: var(--muted);
    font-size: var(--text-2xs);
    font-variant-numeric: tabular-nums;
  }

  .last.changed {
    color: var(--accent);
    font-weight: var(--weight-semibold);
  }

  .empty {
    display: grid;
    gap: var(--space-2);
    padding: var(--space-1-5) 0;
    line-height: 1.5;
  }

  .row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1-5);
    padding: 0 var(--space-3);
  }

  .row select {
    flex: 1;
    min-width: 0;
  }

  .hint {
    margin: var(--space-1-5) var(--space-3);
    color: var(--muted);
    font-size: var(--text-xs);
    line-height: 1.5;
  }

  .empty .hint {
    margin: 0 var(--space-3);
  }

  .error {
    margin: var(--space-1-5) var(--space-3);
    color: var(--danger);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }
</style>
