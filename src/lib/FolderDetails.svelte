<script lang="ts">
  import Archive from "@lucide/svelte/icons/archive";
  import FileCode from "@lucide/svelte/icons/file-code";
  import FileInput from "@lucide/svelte/icons/file-input";
  import FileUp from "@lucide/svelte/icons/file-up";
  import FolderInput from "@lucide/svelte/icons/folder-input";
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import Play from "@lucide/svelte/icons/play";
  import ScanSearch from "@lucide/svelte/icons/scan-search";
  import ServerPlus from "@lucide/svelte/icons/server-plus";
  import Table from "@lucide/svelte/icons/table";
  import Trash from "@lucide/svelte/icons/trash";
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
    openSendFile,
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
    <div class="actions more" aria-label="Actions">
      <button
        title="New folder in this folder"
        onclick={() => newFolder(folder.path)}
        ><FolderPlus size={15} aria-hidden="true" /><span
          class="visually-hidden">New folder…</span
        ></button
      >
      <button
        title="New host in this folder"
        onclick={() => newHost(folder.path)}
        ><ServerPlus size={15} aria-hidden="true" /><span
          class="visually-hidden">New host…</span
        ></button
      >
      <button
        data-folder-import
        title="Import hosts from a CSV file into this folder"
        onclick={() => openCsvImport(folder.path)}
        ><Table size={15} aria-hidden="true" /><span class="visually-hidden"
          >Import CSV…</span
        ></button
      >
      <button
        data-folder-nornir-import
        title="Import hosts from a Nornir inventory into this folder"
        onclick={() => openNornirImport(folder.path)}
        ><FileInput size={15} aria-hidden="true" /><span class="visually-hidden"
          >Import Nornir…</span
        ></button
      >
      <button
        data-folder-run-snippet
        title="Run a saved snippet on every host in this folder, as a job"
        onclick={() =>
          openJob({ kind: "folder", path: folder.path }, folder.name)}
        ><Play size={15} aria-hidden="true" /><span class="visually-hidden"
          >Run snippet…</span
        ></button
      >
      <button
        data-folder-run-script
        title="Run a Python script of the workspace for every host in this folder, as a job"
        onclick={() =>
          openScript({ kind: "folder", path: folder.path }, folder.name)}
        ><FileCode size={15} aria-hidden="true" /><span class="visually-hidden"
          >Run script…</span
        ></button
      >
      <button
        data-folder-send-file
        title="Send a file from this computer to every host in this folder, as a job"
        onclick={() =>
          openSendFile({ kind: "folder", path: folder.path }, folder.name)}
        ><FileUp size={15} aria-hidden="true" /><span class="visually-hidden"
          >Send file…</span
        ></button
      >
      <button
        data-folder-back-up
        title="Fetch the configuration of every host in this folder and keep each in its host's config history if it changed"
        onclick={() =>
          openBackup({ kind: "folder", path: folder.path }, folder.name)}
        ><Archive size={15} aria-hidden="true" /><span class="visually-hidden"
          >Back up configs</span
        ></button
      >
      <button
        data-folder-enrich
        title="Ask every host in this folder for its vendor, model, serial number and OS version (NAPALM), then review what to fill in"
        onclick={() =>
          openEnrich({ kind: "folder", path: folder.path }, folder.name)}
        ><ScanSearch size={15} aria-hidden="true" /><span
          class="visually-hidden">Fill in from devices…</span
        ></button
      >
      {#if !isRoot}
        <button title="Move this folder" onclick={() => move(item)}
          ><FolderInput size={15} aria-hidden="true" /><span
            class="visually-hidden">Move…</span
          ></button
        >
        <button
          class="danger"
          title="Delete this folder"
          onclick={() => remove(item)}
          ><Trash size={15} aria-hidden="true" /><span class="visually-hidden"
            >Delete</span
          ></button
        >
      {/if}
    </div>
  </header>

  <section aria-label="Device" data-details-section="device">
    <h3>Device</h3>
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
    <SettingsPanel
      part="device"
      {item}
      settings={folder.settings}
      effective={effective.current}
      {hostIds}
      onchange={(change) => saveFolderSettings(folder.path, change)}
    />
  </section>

  <section aria-label="Connection" data-details-section="connection">
    <h3>Connection</h3>
    <SettingsPanel
      part="connection"
      {item}
      settings={folder.settings}
      effective={effective.current}
      {hostIds}
      onchange={(change) => saveFolderSettings(folder.path, change)}
    />
  </section>

  <div data-details-section="tags">
    <TagEditor
      tags={folder.tags}
      effective={effective.current?.tags ?? null}
      {item}
      onchange={(change) => setFolderTags(folder.path, change)}
    />
  </div>

  <section aria-label="Description" data-details-section="description">
    <h3>Description</h3>
    <span class="row">
      <textarea
        name="folder-description"
        aria-label="Description"
        rows="4"
        value={folder.description ?? ""}
        onchange={(event) =>
          setFolderDescription(folder.path, event.currentTarget.value)}
      ></textarea>
      <CopyButton value={folder.description ?? ""} what="description" />
    </span>
  </section>
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

  section {
    display: grid;
    gap: var(--space-1-5);
  }

  h3 {
    margin: 0 0 var(--space-1-5);
    font-size: var(--text-xs);
    font-weight: var(--weight-semibold);
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1-5);
    margin-top: var(--space-1);
  }

  .actions button {
    min-height: var(--control-height-small);
    padding: 0 var(--space-2);
    font-size: var(--text-xs);
  }

  /* One quiet row of icons: each button keeps its name for screen readers
     and shows it as its tooltip. */
  .actions.more {
    gap: var(--space-0-5);
  }

  .actions.more button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: var(--control-height);
    height: var(--control-height);
    padding: 0;
    border-color: transparent;
    background: none;
    color: var(--muted);
  }

  .actions.more button:hover,
  .actions.more button:focus-visible {
    background: var(--surface-raised);
    color: var(--text);
  }

  .actions.more button.danger:hover,
  .actions.more button.danger:focus-visible {
    color: var(--danger);
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
