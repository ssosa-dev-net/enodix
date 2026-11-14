<script lang="ts">
  // The workspace's saved jobs (`_jobs.yaml`): one click plans one and opens
  // the job dialog at its pre-flight. A job whose snippet, script, folder,
  // filter or host is gone says so instead of running.

  import { stepTitle, type ListedSavedJob } from "./jobs";
  import {
    inventory,
    jobTargetLabel,
    removeJob,
    renameJob,
    runSavedJob,
  } from "./inventory.svelte";

  let open = $state(true);
  let error = $state<string | null>(null);
  let busy = $state(false);
  const list = $derived(inventory.workspace?.saved_jobs ?? []);

  /** What a saved job does, on what, for its tooltip. */
  function describe(job: ListedSavedJob): string {
    const what = stepTitle(job.step);
    const on =
      job.target.kind === "folder"
        ? job.target.path === ""
          ? "the whole workspace"
          : `folder ${job.target.path}`
        : jobTargetLabel(job.target);
    return `${what} on ${on}`;
  }

  async function run(job: ListedSavedJob) {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await runSavedJob(job.name);
    } catch (reason) {
      error = `${job.name}: ${String(reason)}`;
    } finally {
      busy = false;
    }
  }
</script>

<section class="saved-jobs" class:open data-saved-jobs>
  <header>
    <button
      class="toggle"
      aria-expanded={open}
      data-saved-jobs-toggle
      onclick={() => (open = !open)}
    >
      <span class="caret" aria-hidden="true">{open ? "▾" : "▸"}</span>
      Saved jobs
      {#if list.length > 0}<span class="count">{list.length}</span>{/if}
    </button>
  </header>
  {#if open}
    <div class="body">
      {#if list.length === 0}
        <p class="hint">
          Save a job from its pre-flight list or from a past run (Save as job…)
          to run it again here in one click. Saved jobs are kept in the
          workspace for the team.
        </p>
      {:else}
        <ul>
          {#each list as job (job.name)}
            <li data-saved-job-row={job.name} class:broken={job.problem}>
              <button
                class="run"
                data-saved-job={job.name}
                disabled={busy}
                title={job.problem
                  ? `Cannot run: ${job.problem}`
                  : `Run ${describe(job)} (pre-flight first)`}
                onclick={() => run(job)}
              >
                <span class="name">{job.name}</span>
                <span class="meta">{describe(job)}</span>
                {#if job.problem}
                  <span class="problem" data-saved-job-problem={job.name}
                    >{job.problem}</span
                  >
                {/if}
              </button>
              <button
                class="small"
                data-saved-job-rename={job.name}
                title="Rename"
                aria-label={`Rename ${job.name}`}
                onclick={() => renameJob(job)}>Rename</button
              >
              <button
                class="small"
                data-saved-job-delete={job.name}
                title="Delete"
                aria-label={`Delete ${job.name}`}
                onclick={() => removeJob(job.name)}>×</button
              >
            </li>
          {/each}
        </ul>
      {/if}
      {#if error}
        <p class="error" role="alert" data-saved-jobs-error>{error}</p>
      {/if}
    </div>
  {/if}
</section>

<style>
  .saved-jobs {
    flex: none;
    display: flex;
    flex-direction: column;
    max-height: 30%;
    border-top: 1px solid var(--border);
  }

  header {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.3rem 0.6rem;
    background: var(--surface);
  }

  .toggle {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 0.35rem;
    border-color: transparent;
    background: none;
    padding: 0.1rem 0.2rem;
    font-weight: 600;
    text-align: left;
  }

  .caret {
    width: 0.8rem;
    color: var(--muted);
  }

  .count {
    color: var(--muted);
    font-weight: normal;
    font-size: 0.75rem;
  }

  .body {
    min-height: 0;
    overflow: auto;
    padding: 0.2rem 0 0.4rem;
  }

  .hint {
    margin: 0.4rem 0.6rem;
    color: var(--muted);
    font-size: 0.8rem;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  li {
    display: flex;
    align-items: center;
    gap: 0.2rem;
    padding: 0 0.6rem 0 0.4rem;
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
    padding: 0.2rem 0.4rem;
  }

  .run:not(:disabled):hover {
    filter: none;
    background: var(--surface);
  }

  .run span {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta {
    color: var(--muted);
    font-size: 0.72rem;
  }

  .problem {
    color: var(--danger);
    font-size: 0.72rem;
  }

  li.broken .name {
    color: var(--muted);
  }

  .small {
    flex: none;
    padding: 0.05rem 0.35rem;
    font-size: 0.72rem;
    border-color: transparent;
    background: none;
    color: var(--muted);
    visibility: hidden;
  }

  li:hover .small,
  li:focus-within .small {
    visibility: visible;
  }

  .error {
    margin: 0.4rem 0.6rem;
    color: var(--danger);
    font-size: 0.8rem;
    overflow-wrap: anywhere;
  }
</style>
