<script lang="ts">
  // The Jobs side view (#69): the jobs running here at the top with their
  // progress, the workspace's saved jobs, then every kept run, newest
  // first. A run opens in a tab (its job's own tab if that is still open);
  // a running one can be cancelled from here.
  import Square from "@lucide/svelte/icons/square";
  import SavedJobsPanel from "./SavedJobsPanel.svelte";
  import {
    cancelJob,
    listRuns,
    stepTitle,
    type RunList,
    type StoredRun,
  } from "./jobs";
  import { liveJob, liveJobs } from "./jobs.svelte";
  import { runTitles } from "./jobTabs.svelte";
  import { inventory, jobTargetLabel } from "./inventory.svelte";
  import { openRunTab } from "./views";

  const workspace = $derived(inventory.workspace);

  let list = $state<RunList | null>(null);
  /** Runs the shell says are still running (by id). */
  let running = $state<Set<string>>(new Set());
  let error = $state<string | null>(null);

  async function refresh() {
    try {
      const runs = await listRuns();
      list = runs;
      running = new Set(runs.running);
      for (const entry of runs.runs)
        runTitles[entry.id] = stepTitle(entry.step);
      error = null;
    } catch (reason) {
      error = String(reason);
    }
  }

  // Refetched whenever a job starts or finishes, a run is deleted, and as
  // the workspace changes.
  $effect(() => {
    void liveJobs.revision;
    if (!inventory.workspace?.root) {
      list = null;
      return;
    }
    void refresh();
  });

  function isRunning(id: string) {
    const job = liveJob(id);
    return job ? !job.finished : running.has(id);
  }

  /** "running", "ended" or "interrupted" (the app closed first). */
  function runState(entry: StoredRun) {
    if (isRunning(entry.id)) return "running";
    return entry.ended ? "ended" : "interrupted";
  }

  const runningRuns = $derived(
    list?.runs.filter((entry) => runState(entry) === "running") ?? [],
  );
  const pastRuns = $derived(
    list?.runs.filter((entry) => runState(entry) !== "running") ?? [],
  );

  /** How far a run got: live while its job runs here, else as kept. */
  function progress(entry: StoredRun): string {
    const job = liveJob(entry.id);
    const states = entry.hosts.map(
      (host) =>
        (job && !job.finished ? job.states[host.id] : undefined) ?? host.state,
    );
    const count = (kind: string) =>
      states.filter((state) => state.kind === kind).length;
    // A file already there needed nothing: it counts as done.
    const done = count("done") + count("alreadyThere");
    const failed = count("failed");
    const cancelled = count("cancelled");
    const state = runState(entry);
    if (state === "running") {
      const ended = done + failed + cancelled;
      return `running, ${ended} of ${states.length} finished${failed ? `, ${failed} failed` : ""}`;
    }
    if (state === "interrupted") return "interrupted";
    return `${done} done${failed ? `, ${failed} failed` : ""}${cancelled ? `, ${cancelled} cancelled` : ""}`;
  }

  function when(stamp: string) {
    const date = new Date(stamp);
    return Number.isNaN(date.getTime()) ? stamp : date.toLocaleString();
  }

  function cancel(id: string) {
    const job = liveJob(id);
    if (!job) return;
    cancelJob(job.id).catch((reason) => (error = String(reason)));
  }
</script>

{#snippet row(entry: StoredRun)}
  {@const state = runState(entry)}
  <li>
    <button
      class="run"
      data-run={entry.id}
      data-run-state={state}
      title="Open this run in a tab: output, search, compare, save"
      onclick={() => openRunTab(entry.id)}
    >
      <span class="title">{stepTitle(entry.step)}</span>
      <span class="meta">{jobTargetLabel(entry.target)}</span>
      <span class="meta">{when(entry.started)} · {progress(entry)}</span>
    </button>
    {#if state === "running" && liveJob(entry.id)}
      <button
        class="small"
        data-run-cancel={entry.id}
        title="Cancel this job"
        aria-label={`Cancel ${stepTitle(entry.step)}`}
        onclick={() => cancel(entry.id)}
        ><Square size={13} aria-hidden="true" /></button
      >
    {/if}
  </li>
{/snippet}

<div class="jobs" data-jobs-view>
  <header data-window-bar data-tauri-drag-region="deep"><h2>Jobs</h2></header>

  {#if !workspace}
    <p class="hint" data-needs-workspace>
      Jobs run on the hosts of a workspace: open or create one to see its saved
      jobs and runs here.
    </p>
  {:else}
    {#if runningRuns.length > 0}
      <section class="group" data-runs-running={runningRuns.length}>
        <h3>Running</h3>
        <ul>
          {#each runningRuns as entry (entry.id)}
            {@render row(entry)}
          {/each}
        </ul>
      </section>
    {/if}

    <SavedJobsPanel />

    <section class="group past" data-runs>
      <h3>Past runs</h3>
      {#if list === null}
        <p class="hint">Loading…</p>
      {:else if list.runs.length === 0 && list.damaged.length === 0}
        <p class="hint" data-runs-empty>
          No job has run in this workspace yet. Run a snippet, a script or a
          backup on a folder, a saved filter or a host from its menu in Hosts;
          every run is kept here until you delete it.
        </p>
      {:else}
        <ul aria-label="Runs, newest first">
          {#each pastRuns as entry (entry.id)}
            {@render row(entry)}
          {/each}
          {#each list.damaged as entry (entry.id)}
            <li class="damaged" data-run-damaged={entry.id}>
              <span class="title">{entry.id}</span>
              <span class="meta">Cannot be read: {entry.reason}</span>
            </li>
          {/each}
        </ul>
      {/if}
      {#if error}
        <p class="error" role="alert" data-runs-error>{error}</p>
      {/if}
    </section>
  {/if}
</div>

<style>
  .jobs {
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

  h3 {
    margin: 0;
    padding: var(--space-1-5) var(--space-3) var(--space-0-5);
    color: var(--muted);
    font-size: var(--text-2xs);
    font-weight: var(--weight-semibold);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .group {
    display: flex;
    flex-direction: column;
    min-height: 0;
  }

  .past {
    flex: 1 1 auto;
    border-top: 1px solid var(--border);
  }

  .past ul {
    overflow: auto;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0 0 var(--space-1-5);
  }

  li {
    display: flex;
    align-items: center;
    gap: var(--space-0-5);
    padding: 0 var(--space-2) 0 var(--space-1-5);
  }

  .run {
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

  .run:hover {
    background: var(--surface-raised);
  }

  .run span {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .title {
    font-weight: var(--weight-semibold);
  }

  .run[data-run-state="running"] .title {
    color: var(--accent);
  }

  .meta {
    color: var(--muted);
    font-size: var(--text-2xs);
  }

  li.damaged {
    flex-direction: column;
    align-items: flex-start;
    padding: var(--space-0-5) var(--space-3);
  }

  li.damaged .meta {
    color: var(--danger);
  }

  .small {
    flex: none;
    width: var(--control-height-small);
    height: var(--control-height-small);
    min-height: 0;
    padding: 0;
    border-color: transparent;
    background: none;
    color: var(--muted);
  }

  .small:hover {
    color: var(--danger);
  }

  .hint {
    margin: var(--space-1-5) var(--space-3);
    color: var(--muted);
    font-size: var(--text-xs);
  }

  .error {
    margin: var(--space-1-5) var(--space-3);
    color: var(--danger);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }
</style>
