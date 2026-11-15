<script lang="ts">
  import CopyButton from "./CopyButton.svelte";
  import SettingsPanel from "./SettingsPanel.svelte";
  import TagEditor from "./TagEditor.svelte";
  import {
    effectiveSettings,
    inventory,
    move,
    newFolder,
    newHost,
    openBackup,
    openEnrich,
    openCsvImport,
    openJob,
    openScript,
    openNornirImport,
    remove,
    renameTo,
    saveFolderSettings,
    setFolderDescription,
    setFolderTags,
  } from "./inventory.svelte";
  import { allHosts, type Folder } from "./workspace";

  interface Props {
    folder: Folder;
  }

  let { folder }: Props = $props();

  const item = $derived({ kind: "folder" as const, path: folder.path });
  const isRoot = $derived(folder.path === "");
  const effective = effectiveSettings(() => item);
  const hostIds = $derived(
    inventory.workspace
      ? allHosts(inventory.workspace.tree).map((host) => host.id)
      : [],
  );

  function countHosts(folder: Folder): number {
    return (
      folder.hosts.length +
      folder.folders.reduce((sum, child) => sum + countHosts(child), 0)
    );
  }
  const hosts = $derived(countHosts(folder));
</script>

<div class="details" aria-label="Folder details">
  <header>
    <h2>{folder.name}</h2>
    <span class="file">
      {isRoot ? inventory.workspace?.root : folder.path}
      · {hosts === 1 ? "1 host" : `${hosts} hosts`}
    </span>
    <div class="actions">
      <button onclick={() => newFolder(folder.path)}>New folder…</button>
      <button onclick={() => newHost(folder.path)}>New host…</button>
      <button data-folder-import onclick={() => openCsvImport(folder.path)}
        >Import CSV…</button
      >
      <button
        data-folder-nornir-import
        onclick={() => openNornirImport(folder.path)}>Import Nornir…</button
      >
      <button
        data-folder-run-snippet
        title="Run a saved snippet on every host in this folder, as a job"
        onclick={() =>
          openJob({ kind: "folder", path: folder.path }, folder.name)}
        >Run snippet…</button
      >
      <button
        data-folder-run-script
        title="Run a Python script of the workspace for every host in this folder, as a job"
        onclick={() =>
          openScript({ kind: "folder", path: folder.path }, folder.name)}
        >Run script…</button
      >
      <button
        data-folder-back-up
        title="Fetch the configuration of every host in this folder and keep each in its host's config history if it changed"
        onclick={() =>
          openBackup({ kind: "folder", path: folder.path }, folder.name)}
        >Back up configs</button
      >
      <button
        data-folder-enrich
        title="Ask every host in this folder for its vendor, model, serial number and OS version (NAPALM), then review what to fill in"
        onclick={() =>
          openEnrich({ kind: "folder", path: folder.path }, folder.name)}
        >Fill in from devices…</button
      >
      {#if !isRoot}
        <button onclick={() => move(item)}>Move…</button>
        <button class="danger" onclick={() => remove(item)}>Delete</button>
      {/if}
    </div>
  </header>

  {#if !isRoot}
    <label class="field">
      <span>Name</span>
      <span class="row">
        <input
          name="folder-name"
          value={folder.name}
          spellcheck="false"
          onchange={(event) => renameTo(item, event.currentTarget.value)}
        />
        <CopyButton value={folder.name} what="name" />
      </span>
    </label>
  {/if}
  <label class="field">
    <span>Description</span>
    <span class="row">
      <textarea
        name="folder-description"
        rows="4"
        value={folder.description ?? ""}
        onchange={(event) =>
          setFolderDescription(folder.path, event.currentTarget.value)}
      ></textarea>
      <CopyButton value={folder.description ?? ""} what="description" />
    </span>
  </label>

  <TagEditor
    tags={folder.tags}
    effective={effective.current?.tags ?? null}
    {item}
    onchange={(change) => setFolderTags(folder.path, change)}
  />

  <SettingsPanel
    {item}
    settings={folder.settings}
    effective={effective.current}
    {hostIds}
    onchange={(change) => saveFolderSettings(folder.path, change)}
  />
</div>

<style>
  .details {
    display: grid;
    gap: var(--space-4);
    padding: var(--space-3);
    font-size: var(--text-sm);
  }

  header {
    display: grid;
    gap: var(--space-1);
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }

  .file {
    color: var(--muted);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1-5);
    margin-top: var(--space-1);
  }

  .actions button {
    padding: var(--space-0-5) var(--space-2);
    font-size: var(--text-xs);
  }

  .danger:hover:not(:disabled) {
    color: var(--danger);
  }

  .field {
    display: grid;
    gap: var(--space-0-5);
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--space-1);
  }

  .row input,
  .row textarea {
    flex: 1;
    width: 100%;
  }

  textarea {
    resize: vertical;
  }
</style>
