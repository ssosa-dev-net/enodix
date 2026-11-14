<script lang="ts">
  import { untrack } from "svelte";
  import {
    cancelJob,
    compareOutputs,
    deleteRun,
    exportRun,
    listRuns,
    planAgain,
    runOutput,
    outcomeText,
    searchRun,
    stepTitle,
    type DiffLine,
    type HostState,
    type RunList,
    type SearchHit,
    type StoredRun,
  } from "./jobs";
  import { liveJob, liveJobs } from "./jobs.svelte";
  import { inventory, openJob } from "./inventory.svelte";
  import { pickWorkspaceFolder } from "./workspace";

  interface Props {
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  let list = $state<RunList | null>(null);
  /** Runs the shell says are still running (by id). */
  let running = $state<Set<string>>(new Set());
  let selected = $state<string | null>(untrack(() => liveJobs.select));
  let error = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let busy = $state(false);

  /** The host whose output is shown. */
  let host = $state<string | null>(null);
  let output = $state("");
  let query = $state("");
  let hits = $state<SearchHit[] | null>(null);
  /** The two outputs to compare: a host of this run, and "<run>|<host>" (neither can hold a "|"). */
  let compareOld = $state("");
  let compareNew = $state("");
  let diff = $state<DiffLine[] | null>(null);
  let exportFolder = $state("");
  let confirmingDelete = $state(false);

  const run = $derived(list?.runs.find((entry) => entry.id === selected));
  const live = $derived(selected ? liveJob(selected) : undefined);
  const isRunning = (id: string) =>
    running.has(id) || (liveJob(id) !== undefined && !liveJob(id)!.finished);

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

  async function refresh() {
    try {
      const runs = await listRuns();
      list = runs;
      running = new Set(runs.running);
      if (!runs.runs.some((entry) => entry.id === selected)) {
        selected = runs.runs[0]?.id ?? null;
      }
    } catch (reason) {
      error = String(reason);
    }
  }

  // Refetched whenever a job starts or finishes, and as the workspace
  // changes.
  $effect(() => {
    void liveJobs.revision;
    void inventory.workspace?.root;
    void refresh();
  });

  // A new run selected: its first host, no search, no diff.
  const runId = $derived(run?.id);
  $effect(() => {
    void runId;
    untrack(() => {
      const current = run;
      host = current?.hosts[0]?.id ?? null;
      hits = null;
      diff = null;
      confirmingDelete = false;
      compareOld = current?.hosts[0]?.id ?? "";
      compareNew = "";
    });
  });

  // The shown host's output: live while its job runs here, else from disk.
  $effect(() => {
    const id = selected;
    const shown = host;
    const job = live;
    if (!id || !shown) {
      output = "";
      return;
    }
    if (job && !job.finished) {
      output = job.outputs[shown] ?? "";
      return;
    }
    void liveJobs.revision;
    runOutput({ run: id, host: shown }).then(
      (text) => {
        if (selected === id && host === shown) output = text;
      },
      (reason) => (error = String(reason)),
    );
  });

  /** A host's state: live while its job runs here, else as kept. */
  function stateOf(entry: StoredRun, id: string): HostState {
    const job = liveJob(entry.id);
    if (job && !job.finished && job.states[id]) return job.states[id];
    return entry.hosts.find((h) => h.id === id)?.state ?? { kind: "waiting" };
  }

  function counts(entry: StoredRun) {
    const count = { done: 0, failed: 0, cancelled: 0 };
    for (const h of entry.hosts) {
      const kind = stateOf(entry, h.id).kind;
      if (kind in count) count[kind as keyof typeof count] += 1;
    }
    return count;
  }

  /** "running", "ended" or "interrupted" (the app closed first). */
  function runState(entry: StoredRun) {
    if (isRunning(entry.id)) return "running";
    return entry.ended ? "ended" : "interrupted";
  }

  /** A run's title: its snippet, or "Config backup". */
  function runTitle(entry: StoredRun) {
    return stepTitle(entry.step);
  }

  function targetLabel(entry: StoredRun) {
    if (entry.target.kind === "folder") {
      return entry.target.path === ""
        ? "the whole workspace"
        : `folder ${entry.target.path}`;
    }
    const names = entry.hosts.map((h) => h.name);
    const all = names.length + entry.skipped.length;
    return all <= 3 && names.length === all ? names.join(", ") : `${all} hosts`;
  }

  function when(stamp: string) {
    const date = new Date(stamp);
    return Number.isNaN(date.getTime()) ? stamp : date.toLocaleString();
  }

  const STATE_LABELS: Record<HostState["kind"], string> = {
    waiting: "Waiting",
    connecting: "Connecting",
    running: "Running",
    done: "Done",
    failed: "Failed",
    cancelled: "Cancelled",
  };

  function search() {
    const id = selected;
    if (!id) return;
    return attempt(async () => {
      hits = query.trim() ? await searchRun(id, query) : null;
    });
  }

  /** What the second output can be: another host of this run, or the
   * first one's host in another run. */
  const compareChoices = $derived.by(() => {
    if (!run || !list) return [];
    const choices: { value: string; label: string }[] = [];
    for (const other of run.hosts) {
      if (other.id !== compareOld) {
        choices.push({
          value: `${run.id}|${other.id}`,
          label: `${other.name} in this run`,
        });
      }
    }
    for (const entry of list.runs) {
      // Two runs of the same step: what changed on the host.
      if (entry.id === run.id || runTitle(entry) !== runTitle(run)) {
        continue;
      }
      if (entry.hosts.some((h) => h.id === compareOld)) {
        choices.push({
          value: `${entry.id}|${compareOld}`,
          label: `the same host, run at ${when(entry.started)}`,
        });
      }
    }
    return choices;
  });

  function compare() {
    if (!run || !compareOld || !compareNew) return;
    const [otherRun, otherHost] = compareNew.split("|");
    const id = run.id;
    return attempt(async () => {
      diff = await compareOutputs(
        { run: id, host: compareOld },
        { run: otherRun, host: otherHost },
      );
    });
  }

  async function browse() {
    const folder = await pickWorkspaceFolder("Save the job run to a folder");
    if (folder) exportFolder = folder;
  }

  function save() {
    const id = selected;
    if (!id || !exportFolder.trim()) return;
    return attempt(async () => {
      const copy = await exportRun(id, exportFolder.trim());
      notice = `Saved to ${copy}`;
    });
  }

  function remove() {
    const id = selected;
    if (!id) return;
    return attempt(async () => {
      await deleteRun(id);
      confirmingDelete = false;
      selected = null;
      await refresh();
    });
  }

  function again() {
    const id = selected;
    const entry = run;
    if (!id || !entry) return;
    return attempt(async () => {
      const { target, plan } = await planAgain(id);
      openJob(target, targetLabel(entry), plan);
      onclose();
    });
  }

  function cancel() {
    const job = live;
    if (!job) return;
    return attempt(() => cancelJob(job.id));
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && !busy) {
      event.preventDefault();
      onclose();
    }
  }

  let firstButton = $state<HTMLElement>();
  $effect(() => {
    firstButton?.focus();
  });
</script>

<svelte:window {onkeydown} />

<div class="backdrop">
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="jobs-title"
    data-dialog="jobs"
  >
    <header>
      <h2 id="jobs-title">Jobs</h2>
      <button
        type="button"
        data-jobs="close"
        bind:this={firstButton}
        onclick={onclose}>Close</button
      >
    </header>

    {#if list === null}
      <p class="note">Loading…</p>
    {:else if list.runs.length === 0 && list.damaged.length === 0}
      <p class="note" data-runs-empty>
        No job has run in this workspace yet. Run a snippet on a folder, a saved
        filter or a host; every run is kept here until you delete it.
      </p>
    {:else}
      <div class="columns">
        <ul class="runs" aria-label="Runs, newest first" data-runs>
          {#each list.runs as entry (entry.id)}
            {@const count = counts(entry)}
            <li>
              <button
                type="button"
                class:selected={entry.id === selected}
                data-run={entry.id}
                data-run-state={runState(entry)}
                onclick={() => (selected = entry.id)}
              >
                <span class="title">{runTitle(entry)}</span>
                <span class="meta">{targetLabel(entry)}</span>
                <span class="meta"
                  >{when(entry.started)} · {runState(entry) === "running"
                    ? "running"
                    : runState(entry) === "interrupted"
                      ? "interrupted"
                      : `${count.done} done`}{count.failed
                    ? `, ${count.failed} failed`
                    : ""}{count.cancelled
                    ? `, ${count.cancelled} cancelled`
                    : ""}</span
                >
              </button>
            </li>
          {/each}
          {#each list.damaged as entry (entry.id)}
            <li class="damaged" data-run-damaged={entry.id}>
              <span class="title">{entry.id}</span>
              <span class="meta">Cannot be read: {entry.reason}</span>
            </li>
          {/each}
        </ul>

        {#if run}
          <section class="run" data-run-details={run.id}>
            <div class="actions">
              {#if isRunning(run.id) && live}
                <button
                  type="button"
                  data-run-action="cancel"
                  disabled={busy}
                  onclick={cancel}>Cancel job</button
                >
              {/if}
              <button
                type="button"
                data-run-action="again"
                disabled={busy}
                title="Plan the same snippet on the same target, as the workspace is now"
                onclick={again}>Run again…</button
              >
              {#if !isRunning(run.id)}
                {#if confirmingDelete}
                  <span class="confirm">Delete this run from disk?</span>
                  <button
                    type="button"
                    data-run-action="keep"
                    onclick={() => (confirmingDelete = false)}>Keep</button
                  >
                  <button
                    type="button"
                    class="danger"
                    data-run-action="confirm-delete"
                    disabled={busy}
                    onclick={remove}>Delete</button
                  >
                {:else}
                  <button
                    type="button"
                    data-run-action="delete"
                    onclick={() => (confirmingDelete = true)}>Delete…</button
                  >
                {/if}
              {/if}
            </div>

            <div class="hosts">
              {#each run.hosts as entry (entry.id)}
                {@const state = stateOf(run, entry.id)}
                <button
                  type="button"
                  class:selected={entry.id === host}
                  data-run-host={entry.id}
                  data-host-state={state.kind}
                  onclick={() => (host = entry.id)}
                >
                  <span class="state {state.kind}"
                    >{runState(run) === "interrupted" &&
                    !["done", "failed", "cancelled"].includes(state.kind)
                      ? "Not finished"
                      : STATE_LABELS[state.kind]}</span
                  >
                  <span>{entry.name}</span>
                  {#if state.kind === "failed"}
                    <span class="reason" data-run-reason>{state.reason}</span>
                  {/if}
                  {#if live?.backups[entry.id] ?? entry.backup}
                    {@const outcome = (live?.backups[entry.id] ??
                      entry.backup)!}
                    <span class="backup" data-run-backup={outcome.kind}
                      >{outcomeText(outcome)}</span
                    >
                  {/if}
                </button>
              {/each}
              {#each run.skipped as entry (entry.id)}
                <div class="skipped" data-run-skipped={entry.id}>
                  <span class="state">Skipped</span>
                  <span>{entry.name}</span>
                  <span class="reason">{entry.reason}</span>
                </div>
              {/each}
            </div>

            <pre class="output" data-run-output={host ?? ""}>{output}</pre>

            <form
              class="row"
              data-run-search
              onsubmit={(event) => {
                event.preventDefault();
                void search();
              }}
            >
              <input
                name="run-search"
                placeholder="Search every host's output"
                bind:value={query}
                autocomplete="off"
                spellcheck="false"
              />
              <button type="submit" data-run-action="search" disabled={busy}
                >Search</button
              >
            </form>
            {#if hits !== null}
              {#if hits.length === 0}
                <p class="note" data-search-none>No host shows “{query}”.</p>
              {:else}
                <ul class="hits" data-search-hits={hits.length}>
                  {#each hits as hit (hit.host)}
                    <li data-search-hit={hit.host} data-count={hit.count}>
                      <button
                        type="button"
                        class="link"
                        onclick={() => (host = hit.host)}>{hit.name}</button
                      >
                      <span class="meta"
                        >{hit.count}
                        {hit.count === 1 ? "line" : "lines"}</span
                      >
                      <pre>{hit.lines
                          .map((line) => `${line.number}: ${line.text}`)
                          .join("\n")}</pre>
                    </li>
                  {/each}
                </ul>
              {/if}
            {/if}

            <div class="row">
              <label>
                Compare
                <select name="compare-old" bind:value={compareOld}>
                  {#each run.hosts as entry (entry.id)}
                    <option value={entry.id}>{entry.name}</option>
                  {/each}
                </select>
              </label>
              <label>
                with
                <select name="compare-new" bind:value={compareNew}>
                  <option value="" disabled>Choose…</option>
                  {#each compareChoices as choice (choice.value)}
                    <option value={choice.value}>{choice.label}</option>
                  {/each}
                </select>
              </label>
              <button
                type="button"
                data-run-action="compare"
                disabled={busy || !compareOld || !compareNew}
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

            <form
              class="row"
              data-run-export
              onsubmit={(event) => {
                event.preventDefault();
                void save();
              }}
            >
              <input
                name="run-export-folder"
                placeholder="Save to folder"
                bind:value={exportFolder}
                autocomplete="off"
                spellcheck="false"
              />
              <button type="button" onclick={browse}>Browse…</button>
              <button
                type="submit"
                data-run-action="save"
                disabled={busy || !exportFolder.trim()}>Save to…</button
              >
            </form>
          </section>
        {/if}
      </div>
    {/if}

    {#if notice}
      <p class="note" data-run-notice>{notice}</p>
    {/if}
    {#if error}
      <p class="error" role="alert" data-run-error>{error}</p>
    {/if}
    <p class="note">
      Runs are kept on this computer, outside the workspace, until you delete
      them. Device output can hold secrets the device shows.
    </p>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 21;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding: 4vh 1rem 1rem;
    background: rgb(0 0 0 / 0.35);
  }

  .dialog {
    width: min(76rem, 100%);
    max-height: 92vh;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    padding: 0.8rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.45);
    overflow: hidden;
    font-size: 0.85rem;
  }

  header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }

  h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
  }

  .columns {
    display: grid;
    grid-template-columns: minmax(14rem, 20rem) 1fr;
    gap: 0.6rem;
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

  .runs {
    list-style: none;
    margin: 0;
    padding: 0;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: 4px;
  }

  .runs li > button,
  .runs li.damaged {
    display: grid;
    gap: 0.1rem;
    width: 100%;
    padding: 0.35rem 0.5rem;
    text-align: left;
    background: none;
    border: none;
    border-bottom: 1px solid var(--border);
    border-radius: 0;
    color: inherit;
  }

  .runs button.selected,
  .hosts button.selected {
    background: var(--surface);
  }

  .title {
    font-weight: 600;
  }

  .meta {
    color: var(--muted);
    font-size: 0.9em;
  }

  .damaged .meta {
    color: var(--danger);
  }

  .run {
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
    min-height: 0;
    overflow: auto;
  }

  .actions,
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    align-items: center;
  }

  .row input {
    flex: 1;
    min-width: 10rem;
    padding: 0.3rem 0.5rem;
  }

  .row label {
    display: flex;
    gap: 0.3rem;
    align-items: center;
  }

  .confirm {
    color: var(--danger);
  }

  .danger {
    color: var(--danger);
  }

  .hosts {
    display: grid;
    border: 1px solid var(--border);
    border-radius: 4px;
    max-height: 12rem;
    overflow: auto;
  }

  .hosts > button,
  .hosts > .skipped {
    display: flex;
    gap: 0.6rem;
    align-items: baseline;
    padding: 0.25rem 0.5rem;
    text-align: left;
    background: none;
    border: none;
    border-bottom: 1px solid var(--border);
    border-radius: 0;
    color: inherit;
  }

  .skipped {
    color: var(--muted);
  }

  .state {
    min-width: 6rem;
    font-weight: 600;
  }

  .state.done {
    color: var(--success, inherit);
  }

  .state.failed,
  .reason {
    color: var(--danger);
  }

  .backup {
    color: var(--muted);
  }

  .skipped .reason {
    color: var(--muted);
  }

  .state.cancelled {
    color: var(--warning);
  }

  pre {
    margin: 0;
    padding: 0.4rem 0.5rem;
    font-family: var(--mono, monospace);
    font-size: 0.8rem;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    background: var(--surface);
    border-radius: 4px;
  }

  .output {
    min-height: 6rem;
    max-height: 22rem;
    overflow: auto;
  }

  .hits {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.3rem;
  }

  .hits li {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 0.2rem 0.5rem;
    align-items: baseline;
  }

  .hits pre {
    grid-column: 1 / -1;
    max-height: 8rem;
    overflow: auto;
  }

  .link {
    background: none;
    border: none;
    padding: 0;
    color: var(--accent, inherit);
    text-decoration: underline;
    cursor: pointer;
  }

  .diff {
    font-family: var(--mono, monospace);
    font-size: 0.8rem;
    max-height: 22rem;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: 4px;
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
    background: color-mix(in srgb, var(--success, #2a7) 18%, transparent);
  }

  .diff .removed {
    background: color-mix(in srgb, var(--danger, #c33) 18%, transparent);
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
