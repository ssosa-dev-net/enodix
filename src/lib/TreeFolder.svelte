<script lang="ts">
  import TreeFolder from "./TreeFolder.svelte";
  import type { Folder, Host } from "./workspace";

  interface Props {
    folder: Folder;
    depth: number;
    selected: string | null;
    onselect: (host: Host) => void;
    onconnect: (host: Host) => void;
  }

  let { folder, depth, selected, onselect, onconnect }: Props = $props();

  // Folders start collapsed, so a large inventory stays light.
  let expanded = $state<Record<string, boolean>>({});

  const indent = $derived(`${0.6 + depth * 0.9}rem`);

  function onkeydown(event: KeyboardEvent, host: Host) {
    if (event.key === "Enter") {
      event.preventDefault();
      onconnect(host);
    }
  }
</script>

<ul>
  {#each folder.folders as child (child.path)}
    <li>
      <button
        class="folder"
        aria-expanded={!!expanded[child.path]}
        style:padding-left={indent}
        title={child.description ?? child.path}
        onclick={() => (expanded[child.path] = !expanded[child.path])}
      >
        <span class="twisty">{expanded[child.path] ? "▾" : "▸"}</span>
        <span class="label">{child.name}</span>
      </button>
      {#if expanded[child.path]}
        <TreeFolder
          folder={child}
          depth={depth + 1}
          {selected}
          {onselect}
          {onconnect}
        />
      {/if}
    </li>
  {/each}
  {#each folder.hosts as host (host.id)}
    <li>
      <button
        class="host"
        aria-pressed={selected === host.id}
        class:selected={selected === host.id}
        style:padding-left={indent}
        data-host={host.id}
        title="{host.address}{host.port ? `:${host.port}` : ''}"
        onclick={() => onselect(host)}
        ondblclick={() => onconnect(host)}
        onkeydown={(event) => onkeydown(event, host)}
      >
        <span class="twisty"></span>
        <span class="label">{host.name}</span>
        <span class="address">{host.address}</span>
      </button>
    </li>
  {/each}
</ul>

<style>
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  button {
    display: flex;
    align-items: baseline;
    gap: 0.35rem;
    width: 100%;
    border: 0;
    border-radius: 0;
    background: none;
    padding: 0.2rem 0.6rem;
    text-align: left;
    white-space: nowrap;
    user-select: none;
  }

  button:hover:not(:disabled) {
    filter: none;
    background: var(--surface);
  }

  button:focus-visible {
    outline-offset: -2px;
  }

  .selected {
    background: var(--surface);
    box-shadow: inset 2px 0 var(--accent);
  }

  .twisty {
    width: 0.8rem;
    flex: none;
    color: var(--muted);
  }

  .label {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .folder .label {
    font-weight: 600;
  }

  .address {
    margin-left: auto;
    padding-left: 0.5rem;
    color: var(--muted);
    font-size: 0.75rem;
  }
</style>
