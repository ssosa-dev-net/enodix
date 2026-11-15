<script lang="ts">
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import ChevronRight from "@lucide/svelte/icons/chevron-right";
  import PanelBottomOpen from "@lucide/svelte/icons/panel-bottom-open";
  import RoleIcon from "./RoleIcon.svelte";
  import TreeFolder from "./TreeFolder.svelte";
  import {
    folderShown,
    hostRole,
    hostSafety,
    hostShown,
    inventory,
    type Item,
  } from "./inventory.svelte";
  import type { Folder, Host } from "./workspace";

  interface Props {
    folder: Folder;
    depth: number;
    onconnect: (host: Host) => void;
    /** Opens the details sheet on an item (the row's details control). */
    ondetails: (item: Item) => void;
  }

  let { folder, depth, onconnect, ondetails }: Props = $props();

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

{#snippet detailsControl(item: Item, path: string, name: string)}
  <!-- Beside the row rather than in it (a button cannot hold a button);
       shown while the row is hovered, focused or selected. -->
  <button
    class="row-details"
    data-row-details={path}
    aria-label="Details of {name}"
    title="Details (Ctrl+Shift+D)"
    tabindex="-1"
    onclick={() => ondetails(item)}
    ><PanelBottomOpen size={14} aria-hidden="true" /></button
  >
{/snippet}

<!-- Rows carry data-item (what is dragged or acted on) and data-drop (the
     folder something dropped on them goes into); WorkspacePanel handles
     dragging and the context menu for the whole tree. -->
<!-- Under a saved filter, only its hosts and the folders holding them show. -->
<ul class:top={depth === 0}>
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
        <span class="twisty" aria-hidden="true"
          >{#if inventory.expanded[child.path]}<ChevronDown
              size={14}
            />{:else}<ChevronRight size={14} />{/if}</span
        >
        <span class="label">{child.name}</span>
      </button>
      {@render detailsControl(
        { kind: "folder", path: child.path },
        child.path,
        child.name,
      )}
      {#if inventory.expanded[child.path]}
        <TreeFolder folder={child} depth={depth + 1} {onconnect} {ondetails} />
      {/if}
    </li>
  {/each}
  {#each folder.hosts.filter((host) => hostShown(host.id)) as host (host.id)}
    {@const role = hostRole(host.id)}
    {@const environment = hostSafety(host.id)?.environment ?? null}
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
        data-environment={environment ?? undefined}
        title={[host.address, role?.name, environment]
          .filter(Boolean)
          .join(" · ")}
        onclick={() => (inventory.selected = { kind: "host", id: host.id })}
        ondblclick={() => onconnect(host)}
        onkeydown={(event) => onkeydown(event, host)}
      >
        <span class="twisty"></span>
        {#if role}
          <RoleIcon {role} />
        {/if}
        <span class="label">{host.name}</span>
        {#if environment === "production"}
          <span class="env production" data-host-environment="production"
            >PROD</span
          >
        {:else if environment}
          <span
            class="env {environment}"
            data-host-environment={environment}
            aria-hidden="true"
          ></span>
        {/if}
        <span class="address">{host.address}</span>
      </button>
      {@render detailsControl(
        { kind: "host", id: host.id },
        host.id,
        host.name,
      )}
    </li>
  {/each}
</ul>

<style>
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  ul.top {
    padding: 0 var(--space-1-5);
  }

  li {
    position: relative;
  }

  /* Over the row's right end (its address) while it is hovered. */
  .row-details {
    position: absolute;
    top: 2px;
    right: var(--space-1);
    width: 24px;
    height: 24px;
    min-height: 0;
    padding: 0;
    justify-content: center;
    background: var(--surface-raised);
    color: var(--muted);
    opacity: 0;
  }

  li:hover > .row-details,
  li:focus-within > .row-details {
    opacity: 1;
  }

  .row-details:hover:not(:disabled) {
    color: var(--text);
    background: var(--selected);
  }

  button {
    display: flex;
    align-items: center;
    gap: var(--space-1-5);
    width: 100%;
    min-height: 28px;
    border: 0;
    border-radius: var(--radius-md);
    background: none;
    padding: var(--space-0-5) var(--space-2);
    text-align: left;
    white-space: nowrap;
    user-select: none;
  }

  button:hover:not(:disabled) {
    background: var(--surface-raised);
  }

  button:focus-visible {
    outline-offset: -2px;
  }

  .selected,
  .selected:hover:not(:disabled) {
    background: var(--selected);
    box-shadow: inset 2px 0 var(--accent);
  }

  .drop-target,
  .drop-target:hover:not(:disabled) {
    background: color-mix(in srgb, var(--accent) 25%, transparent);
  }

  .twisty {
    width: 14px;
    flex: none;
    display: inline-flex;
    color: var(--muted);
  }

  .label {
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .folder .label {
    font-weight: var(--weight-medium);
    color: var(--muted);
  }

  /* The host's environment, told apart by shape as well as colour: a
     production label, a staging dot, a lab ring. */
  .env {
    flex: none;
  }

  .env.production {
    padding: 0 var(--space-1);
    border-radius: var(--radius-sm);
    background: var(--env-production);
    color: var(--on-env-production);
    font-size: var(--text-2xs);
    font-weight: var(--weight-semibold);
    line-height: 1.4;
  }

  .env.staging,
  .env.lab {
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 50%;
  }

  .env.staging {
    background: var(--env-staging);
  }

  .env.lab {
    border: 2px solid var(--env-lab);
  }

  .address {
    margin-left: auto;
    padding-left: var(--space-2);
    color: var(--muted);
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
  }
</style>
