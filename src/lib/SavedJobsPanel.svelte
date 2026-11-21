<script lang="ts">
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import ChevronRight from "@lucide/svelte/icons/chevron-right";
  import X from "@lucide/svelte/icons/x";
  // The workspace's saved jobs (`_jobs.yaml`), in the Jobs side view: one
  // click plans one and opens a job tab at its pre-flight. A job whose snippet, script, folder,
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

  /**
   * What the job says of answers the jobs file holds but it never uses,
   * their names looking like secrets (#126).
   */
  function ignoredText(names: string[]): string {
    const listed = names.map((name) => `{{${name}}}`).join(", ");
    return names.length === 1
      ? `The answer to ${listed} in _jobs.yaml is no longer used (the name looks like a secret): it is asked at each run. Remove it from the file; the app cannot remove it from the git history.`
      : `The answers to ${listed} in _jobs.yaml are no longer used (the names look like secrets): they are asked at each run. Remove them from the file; the app cannot remove them from the git history.`;
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
      <span class="caret" aria-hidden="true"
        >{#if open}<ChevronDown size={14} />{:else}<ChevronRight
            size={14}
          />{/if}</span
      >
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
                {#if job.ignored_answers?.length}
                  <span
                    class="ignored"
                    data-saved-job-ignored={job.ignored_answers.join(",")}
                    >{ignoredText(job.ignored_answers)}</span
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
                onclick={() => removeJob(job.name)}
                ><X size={13} aria-hidden="true" /></button
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
    flex: 0 1 auto;
    min-height: 0;
    display: flex;
    flex-direction: column;
    border-top: 1px solid var(--border);
  }

  header {
    display: flex;
    align-items: center;
    gap: var(--space-1-5);
    padding: var(--space-1) var(--space-2);
    background: var(--surface);
  }

  .toggle {
    flex: 1;
    display: flex;
    align-items: center;
    gap: var(--space-1-5);
    border-color: transparent;
    background: none;
    padding: var(--space-0-5) var(--space-0-5);
    font-weight: var(--weight-semibold);
    text-align: left;
  }

  .caret {
    width: 0.8rem;
    color: var(--muted);
  }

  .count {
    color: var(--muted);
    font-weight: normal;
    font-size: var(--text-xs);
  }

  .body {
    min-height: 0;
    overflow: auto;
    padding: var(--space-0-5) 0 var(--space-1-5);
  }

  .hint {
    margin: var(--space-1-5) var(--space-2);
    color: var(--muted);
    font-size: var(--text-xs);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
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
    padding: var(--space-0-5) var(--space-1-5);
  }

  .run:not(:disabled):hover {
    background: var(--surface-raised);
  }

  .run span {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .meta {
    color: var(--muted);
    font-size: var(--text-2xs);
  }

  .problem {
    color: var(--danger);
    font-size: var(--text-2xs);
  }

  .run .ignored {
    color: var(--warning);
    font-size: var(--text-2xs);
    white-space: normal;
  }

  li.broken .name {
    color: var(--muted);
  }

  .small {
    flex: none;
    padding: var(--space-0-5) var(--space-1-5);
    font-size: var(--text-2xs);
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
    margin: var(--space-1-5) var(--space-2);
    color: var(--danger);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }
</style>
