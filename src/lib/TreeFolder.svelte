<script lang="ts">
  import RoleIcon from "./RoleIcon.svelte";
  import TreeFolder from "./TreeFolder.svelte";
  import {
    folderShown,
    hostRole,
    hostShown,
    inventory,
  } from "./inventory.svelte";
  import type { Folder, Host } from "./workspace";

  interface Props {
    folder: Folder;
    depth: number;
    onconnect: (host: Host) => void;
  }

  let { folder, depth, onconnect }: Props = $props();

  const indent = $derived(`${0.6 + depth * 0.9}rem`);

  function isSelected(kind: "host" | "folder", path: string) {
    const selected = inventory.selected;
    return (
      selected?.kind === kind &&
      (selected.kind === "host" ? selected.id : selected.path) === path
    );
  }

  function clickFolder(child: Folder) {
    inventory.selected = { kind: "folder", path: child.path };
    inventory.expanded[child.path] = !inventory.expanded[child.path];
  }

  function onkeydown(event: KeyboardEvent, host: Host) {
    if (event.key === "Enter") {
      event.preventDefault();
      onconnect(host);
    }
  }
</script>

<!-- Rows carry data-item (what is dragged or acted on) and data-drop (the
     folder something dropped on them goes into); WorkspacePanel handles
     dragging and the context menu for the whole tree. -->
<!-- Under a saved filter, only its hosts and the folders holding them show. -->
<ul>
  {#each folder.folders.filter( (child) => folderShown(child.path) ) as child (child.path)}
    <li>
      <button
        class="folder"
        class:selected={isSelected("folder", child.path)}
        class:drop-target={inventory.dropTarget === child.path}
        aria-expanded={!!inventory.expanded[child.path]}
        style:padding-left={indent}
        title={child.description ?? child.path}
        data-item="folder"
        data-path={child.path}
        data-drop={child.path}
        data-label={child.name}
        onclick={() => clickFolder(child)}
      >
        <span class="twisty">{inventory.expanded[child.path] ? "▾" : "▸"}</span>
        <span class="label">{child.name}</span>
      </button>
      {#if inventory.expanded[child.path]}
        <TreeFolder folder={child} depth={depth + 1} {onconnect} />
      {/if}
    </li>
  {/each}
  {#each folder.hosts.filter((host) => hostShown(host.id)) as host (host.id)}
    {@const role = hostRole(host.id)}
    <li>
      <button
        class="host"
        aria-pressed={isSelected("host", host.id)}
        class:selected={isSelected("host", host.id)}
        style:padding-left={indent}
        data-host={host.id}
        data-item="host"
        data-path={host.id}
        data-drop={folder.path}
        data-label={host.name}
        title={role ? `${host.address} · ${role.name}` : host.address}
        onclick={() => (inventory.selected = { kind: "host", id: host.id })}
        ondblclick={() => onconnect(host)}
        onkeydown={(event) => onkeydown(event, host)}
      >
        <span class="twisty"></span>
        {#if role}
          <RoleIcon {role} />
        {/if}
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

  .drop-target,
  .drop-target:hover:not(:disabled) {
    background: color-mix(in srgb, var(--accent) 25%, transparent);
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
