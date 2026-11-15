<script lang="ts">
  // A host's config history, in its tab (#70; one tab per host): the kept
  // versions, the text of one, a diff of any two, copy, save to a folder
  // and back up now. It refetches whenever a backup keeps or checks a
  // configuration. It can open in a window of its own and dock back (#78).
  import { untrack } from "svelte";
  import {
    configVersion,
    configVersions,
    diffConfigs,
    exportConfig,
    versionTime,
    type ConfigVersion,
    type DiffLine,
  } from "./jobs";
  import { liveJobs } from "./jobs.svelte";
  import { inventory, openBackup } from "./inventory.svelte";
  import { findHost, pickWorkspaceFolder } from "./workspace";
  import { backUpInMainWindow, detachConfigs, dockBack } from "./windows";

  interface Props {
    /** The host whose history is shown. */
    host: string;
    /** Shown in a window of its own (#78): it docks back, and Back up now
     * opens the backup's tab in the main window. */
    detached?: boolean;
    /** Closes its tab (after it opened in a window). */
    onclose?: () => void;
  }

  let { host, detached = false, onclose }: Props = $props();

  const name = $derived(
    (inventory.workspace && findHost(inventory.workspace.tree, host)?.name) ??
      host,
  );

  let versions = $state<ConfigVersion[] | null>(null);
  /** The version whose text is shown. */
  let selected = $state<string | null>(null);
  let text = $state("");
  /** The version `text` holds ("" while it loads). */
  let textOf = $state("");
  /** The two versions to diff: older, then newer. */
  let older = $state("");
  let newer = $state("");
  let diff = $state<DiffLine[] | null>(null);
  let exportFolder = $state("");
  let notice = $state<string | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function attempt(action: () => Promise<void>) {
    if (busy) return;
    busy = true;
    error = null;
    notice = null;
    try {
      await action();
    } catch (reason) {
      error = String(reason);
    } finally {
      busy = false;
    }
  }

  // Refetched when a backup kept or checked a configuration.
  $effect(() => {
    void liveJobs.configs;
    const id = host;
    configVersions(id).then(
      (list) => {
        if (host !== id) return;
        untrack(() => {
          const newest = list[0]?.id ?? "";
          // A new version (or the first list): show it, and compare it
          // with the one before.
          if (newest !== (versions?.[0]?.id ?? null)) {
            selected = newest || null;
            newer = newest;
            older = list[1]?.id ?? newest;
            diff = null;
          }
          if (!list.some((v) => v.id === selected)) selected = newest || null;
          if (!list.some((v) => v.id === newer)) newer = newest;
          if (!list.some((v) => v.id === older)) older = list[1]?.id ?? newest;
          versions = list;
        });
      },
      (reason) => (error = String(reason)),
    );
  });

  // The selected version's text.
  $effect(() => {
    const id = host;
    const version = selected;
    text = "";
    textOf = "";
    if (!version) return;
    configVersion(id, version).then(
      (body) => {
        if (host === id && selected === version) {
          text = body;
          textOf = version;
        }
      },
      (reason) => (error = String(reason)),
    );
  });

  // A diff shown belongs to the versions it was made of.
  $effect(() => {
    void older;
    void newer;
    untrack(() => (diff = null));
  });

  function compare() {
    const id = host;
    if (!older || !newer) return;
    return attempt(async () => {
      diff = await diffConfigs(id, older, newer);
    });
  }

  function copy() {
    return attempt(async () => {
      await navigator.clipboard.writeText(text);
      notice = `Version ${versionTime(selected ?? "")} copied`;
    });
  }

  async function browse() {
    const folder = await pickWorkspaceFolder(
      "Save the configuration to a folder",
    );
    if (folder) exportFolder = folder;
  }

  function save() {
    const id = host;
    const version = selected;
    if (!version || !exportFolder.trim()) return;
    return attempt(async () => {
      const copy = await exportConfig(id, version, exportFolder.trim());
      notice = `Saved to ${copy}`;
    });
  }

  function backUp() {
    if (detached) void attempt(() => backUpInMainWindow());
    else openBackup({ kind: "hosts", ids: [host] }, name);
  }

  function detach() {
    return attempt(async () => {
      await detachConfigs(host);
      onclose?.();
    });
  }

  function dock() {
    return attempt(() => dockBack());
  }
</script>

<div class="history" data-view="config-history" data-host={host}>
  <div class="inner">
    <header>
      <h2>Config history of {name}</h2>
      {#if versions !== null && versions.length > 0}
        <span class="meta"
          >{versions.length}
          {versions.length === 1 ? "version" : "versions"}</span
        >
      {/if}
      <span class="spacer"></span>
      {#if detached}
        <button
          type="button"
          class="primary"
          data-config-action="dock"
          title="Show the history as a tab in the main window; this window closes"
          disabled={busy}
          onclick={dock}>Dock back</button
        >
      {:else}
        <button
          type="button"
          data-config-action="detach"
          title="Show the history in a window of its own; this tab closes"
          disabled={busy}
          onclick={detach}>Open in window</button
        >
      {/if}
    </header>
    <p class="note" data-config-sensitive>
      Backups are kept on this computer, outside the workspace, unencrypted, and
      never deleted automatically. They are as sensitive as the configurations
      themselves: passwords and keys in a configuration are in its backups too.
    </p>

    {#if versions === null}
      <p class="note">Loading…</p>
    {:else if versions.length === 0}
      <p class="note" data-config-empty>
        No configuration of {name} has been backed up yet.
      </p>
      <div class="row">
        <button type="button" data-config-action="back-up" onclick={backUp}
          >Back up now…</button
        >
      </div>
    {:else}
      <div class="columns">
        <ul class="versions" aria-label="Versions, newest first">
          {#each versions as version (version.id)}
            <li>
              <button
                type="button"
                class:selected={version.id === selected}
                data-config-version={version.id}
                onclick={() => (selected = version.id)}
              >
                <span class="title">{versionTime(version.id)}</span>
                <span class="meta"
                  >{version.lines}
                  {version.lines === 1 ? "line" : "lines"}</span
                >
              </button>
            </li>
          {/each}
        </ul>

        <section class="version">
          <div class="row">
            <button
              type="button"
              data-config-action="copy"
              disabled={busy || !selected || textOf !== selected}
              onclick={copy}>Copy text</button
            >
            <button type="button" data-config-action="back-up" onclick={backUp}
              >Back up now…</button
            >
          </div>
          <pre class="text" data-config-text={textOf}>{text}</pre>

          <form
            class="row"
            data-config-export
            onsubmit={(event) => {
              event.preventDefault();
              void save();
            }}
          >
            <input
              name="config-export-folder"
              placeholder="Save this version to a folder"
              bind:value={exportFolder}
              autocomplete="off"
              spellcheck="false"
            />
            <button type="button" onclick={browse}>Browse…</button>
            <button
              type="submit"
              data-config-action="save"
              disabled={busy || !selected || !exportFolder.trim()}
              >Save to…</button
            >
          </form>

          <div class="row">
            <label>
              Compare
              <select name="config-old" bind:value={older}>
                {#each versions as version (version.id)}
                  <option value={version.id}>{versionTime(version.id)}</option>
                {/each}
              </select>
            </label>
            <label>
              with
              <select name="config-new" bind:value={newer}>
                {#each versions as version (version.id)}
                  <option value={version.id}>{versionTime(version.id)}</option>
                {/each}
              </select>
            </label>
            <button
              type="button"
              data-config-action="diff"
              disabled={busy || !older || !newer || older === newer}
              onclick={compare}>Compare</button
            >
          </div>
          {#if diff !== null}
            <div
              class="diff"
              data-diff
              data-diff-changes={diff.filter((line) => line.kind !== "same")
                .length}
            >
              {#each diff as line, index (index)}
                <div class="line {line.kind}" data-diff-line={line.kind}>
                  <span class="number"
                    >{line.kind === "added" ? "" : line.old}</span
                  >
                  <span class="number"
                    >{line.kind === "removed" ? "" : line.new}</span
                  >
                  <span class="sign"
                    >{line.kind === "added"
                      ? "+"
                      : line.kind === "removed"
                        ? "−"
                        : " "}</span
                  >
                  <span class="text">{line.text}</span>
                </div>
              {/each}
            </div>
          {/if}
        </section>
      </div>
    {/if}

    {#if notice}
      <p class="note" data-config-notice>{notice}</p>
    {/if}
    {#if error}
      <p class="error" role="alert" data-config-error>{error}</p>
    {/if}
  </div>
</div>

<style>
  .history {
    height: 100%;
    min-height: 0;
    overflow: hidden;
    background: var(--bg);
    font-size: var(--text-sm);
  }

  .inner {
    height: 100%;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3);
  }

  header {
    display: flex;
    gap: var(--space-2);
    align-items: baseline;
  }

  .spacer {
    flex: 1;
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
  }

  .columns {
    display: grid;
    grid-template-columns: minmax(12rem, 16rem) 1fr;
    gap: var(--space-2);
    min-height: 0;
    flex: 1;
    overflow: hidden;
  }

  @media (max-width: 720px) {
    .columns {
      grid-template-columns: 1fr;
      overflow: auto;
    }
  }

  .versions {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }

  .versions li > button {
    display: grid;
    gap: var(--space-0-5);
    width: 100%;
    padding: var(--space-1-5) var(--space-2);
    text-align: left;
    background: none;
    border: none;
    border-bottom: 1px solid var(--border);
    border-radius: 0;
    color: inherit;
  }

  .versions button.selected {
    background: var(--surface);
  }

  .title {
    font-weight: var(--weight-semibold);
  }

  .meta {
    color: var(--muted);
    font-size: var(--text-sm);
  }

  .version {
    display: flex;
    flex-direction: column;
    gap: var(--space-1-5);
    min-height: 0;
    overflow: auto;
  }

  .row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1-5);
    align-items: center;
  }

  .row input {
    flex: 1;
    min-width: 10rem;
    padding: var(--space-1) var(--space-2);
  }

  .row label {
    display: flex;
    gap: var(--space-1);
    align-items: center;
  }

  pre {
    margin: 0;
    padding: var(--space-1-5) var(--space-2);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    background: var(--surface);
    border-radius: var(--radius-sm);
  }

  .text {
    min-height: 6rem;
    max-height: 40vh;
    overflow: auto;
  }

  .diff {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    max-height: 40vh;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }

  .diff .line {
    display: grid;
    grid-template-columns: 3.5em 3.5em 1.2em 1fr;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .diff .number {
    color: var(--muted);
    text-align: right;
    padding-right: 0.4em;
  }

  .diff .added {
    background: color-mix(in srgb, var(--ok) 18%, transparent);
  }

  .diff .removed {
    background: color-mix(in srgb, var(--danger) 18%, transparent);
  }

  .note {
    margin: 0;
    color: var(--muted);
  }

  .error {
    margin: 0;
    color: var(--danger);
    overflow-wrap: anywhere;
  }
</style>
