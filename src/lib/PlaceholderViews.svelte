<script lang="ts">
  // The Jobs and Config backups side views until their own views exist:
  // each says what it is about and opens what the app has for it today
  // (the saved jobs and the jobs view, config history and backups).
  import SavedJobsPanel from "./SavedJobsPanel.svelte";
  import {
    inventory,
    openBackup,
    openConfigHistory,
    selectedHost,
  } from "./inventory.svelte";
  import { openJobsView, runningCount } from "./jobs.svelte";

  interface Props {
    view: "jobs" | "backups";
  }

  let { view }: Props = $props();

  const workspace = $derived(inventory.workspace);
  const host = $derived(selectedHost());
  const running = $derived(runningCount());

  const TITLES = {
    jobs: "Jobs",
    backups: "Config backups",
  };
</script>

<div class="placeholder" data-placeholder-view={view}>
  <header><h2>{TITLES[view]}</h2></header>

  {#if !workspace}
    <p class="hint">
      Open a workspace to see its {TITLES[view].toLowerCase()}.
    </p>
  {:else if view === "jobs"}
    <div class="body">
      <p class="hint">
        {running > 0
          ? `${running} job${running === 1 ? "" : "s"} running.`
          : "Runs of snippets, scripts and backups on many hosts at once."}
      </p>
      <button
        data-open-jobs
        data-jobs-running={running}
        title="Past runs, their output, search, compare and save"
        onclick={() => openJobsView()}
        >{running > 0 ? `${running} running: open runs` : "Past runs…"}</button
      >
    </div>
    <SavedJobsPanel />
  {:else}
    <div class="body">
      <p class="hint">
        Back up the configuration of a host or folder from its menu in Hosts;
        each host keeps its earlier versions.
      </p>
      {#if host}
        <button
          data-backups-history
          title="Earlier versions of this host's configuration"
          onclick={() => openConfigHistory(host.id)}
          >Config history of {host.name}…</button
        >
      {:else}
        <p class="hint">Select a host in Hosts to see its config history.</p>
      {/if}
      <button
        data-backups-all
        title="Back up the configuration of every host of the workspace"
        onclick={() =>
          openBackup({ kind: "folder", path: "" }, workspace.tree.name)}
        >Back up every host…</button
      >
    </div>
  {/if}
</div>

<style>
  .placeholder {
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
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
  }

  .body {
    display: grid;
    gap: var(--space-2);
    padding: var(--space-3);
  }

  .body button {
    justify-self: start;
  }

  .hint {
    margin: 0;
    color: var(--muted);
  }

  .placeholder > .hint {
    margin: var(--space-3);
  }
</style>
