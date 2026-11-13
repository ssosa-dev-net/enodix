<script lang="ts">
  import TreeFolder from "./TreeFolder.svelte";
  import type { Host, WorkspaceView } from "./workspace";

  interface Props {
    workspace: WorkspaceView | null;
    /** Why the last workspace could not be opened, if it could not. */
    error: string | null;
    onopen: () => void;
    onconnect: (host: Host) => void;
  }

  let { workspace, error, onopen, onconnect }: Props = $props();

  let selected = $state<string | null>(null);
</script>

<aside>
  <header>
    <span class="name" title={workspace?.root}>
      {workspace?.tree.name ?? "No workspace"}
    </span>
    <button onclick={onopen}>Open workspace…</button>
  </header>

  {#if error}
    <p class="error" role="alert">{error}</p>
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

    <nav class="tree" aria-label="Inventory">
      <TreeFolder
        folder={workspace.tree}
        depth={0}
        {selected}
        onselect={(host) => (selected = host.id)}
        onconnect={(host) => {
          selected = host.id;
          onconnect(host);
        }}
      />
    </nav>
  {:else if !error}
    <p class="hint">Open a workspace folder to see its hosts.</p>
  {/if}
</aside>

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

  .name {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-weight: 600;
  }

  header button {
    padding: 0.25rem 0.6rem;
  }

  .tree {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: 0.3rem 0;
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
</style>
