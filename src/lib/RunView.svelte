<script lang="ts">
  // A kept run in a tab of its own (#69): its hosts and their output (live
  // while its job runs here), search, compare, save to a folder, delete,
  // run again and save as job. The Jobs side view lists the runs.
  import { untrack } from "svelte";
  import {
    cancelJob,
    compareOutputs,
    deleteRun,
    exportRun,
    isFinal,
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
  import { runTitles } from "./jobTabs.svelte";
  import { inventory, openJob } from "./inventory.svelte";
  import SaveJobForm from "./SaveJobForm.svelte";
  import { sizeText } from "./files";
  import { pickWorkspaceFolder } from "./workspace";
  import { openView } from "./views";
  import { detachRun, dockBack } from "./windows";

  interface Props {
    /** The run's id. */
    run: string;
    /** Closes the tab (once the run is deleted, or shown in a window of
     * its own). */
    onclose: () => void;
    /**
     * Shown in a window of its own (#77): it docks back, and the actions
     * that open tabs or change the workspace (review, run again, save as
     * job, delete) wait for the main window.
     */
    detached?: boolean;
  }

  let { run: selected, onclose, detached = false }: Props = $props();

  let list = $state<RunList | null>(null);
  /** Runs the shell says are still running (by id). */
  let running = $state<Set<string>>(new Set());
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
  // The tab's name, once known.
  $effect(() => {
    if (run) runTitles[run.id] = stepTitle(run.step);
  });
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
      // Already there is done too: nothing needed sending.
      const kind =
        stateOf(entry, h.id).kind === "alreadyThere"
          ? "done"
          : stateOf(entry, h.id).kind;
      if (kind in count) count[kind as keyof typeof count] += 1;
    }
    return count;
  }

  /** "running", "ended" or "interrupted" (the app closed first). */
  function runState(entry: StoredRun) {
    if (isRunning(entry.id)) return "running";
    return entry.ended ? "ended" : "interrupted";
  }

  /** A run's title: its snippet, its script, or "Config backup". */
  function runTitle(entry: StoredRun) {
    return stepTitle(entry.step);
  }

  function targetLabel(entry: StoredRun) {
    if (entry.target.kind === "folder") {
      return entry.target.path === ""
        ? "the whole workspace"
        : `folder ${entry.target.path}`;
    }
    if (entry.target.kind === "filter") {
      return `the hosts of ${entry.target.name}`;
    }
    const names = (entry.targets ?? entry.hosts).map((h) => h.name);
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
    alreadyThere: "Already there",
    failed: "Failed",
    cancelled: "Cancelled",
  };

  /** A state's label in `entry`: a file job says "Sending" and "Sent". */
  function stateLabel(entry: StoredRun, state: HostState): string {
    if (entry.step.kind === "sendFile") {
      if (state.kind === "running") return "Sending";
      if (state.kind === "done") return "Sent";
    }
    return STATE_LABELS[state.kind];
  }

  /** What a file run did to the file a device had of that name. */
  function overwriteText(entry: StoredRun): string {
    if (entry.step.kind !== "sendFile") return "";
    return entry.step.overwrite
      ? "overwritten if there"
      : "kept if there (not overwritten)";
  }

  /** A file run's destination, for the user. */
  function destinationText(entry: StoredRun): string {
    if (entry.step.kind !== "sendFile") return "";
    return entry.step.destination || "each host's platform file folder";
  }

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
          label: `the same ${run.step.kind === "script" ? "script" : "host"}, run at ${when(entry.started)}`,
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
      // The Jobs side view lists it no more.
      liveJobs.revision += 1;
      onclose();
    });
  }

  function again() {
    const id = selected;
    const entry = run;
    if (!id || !entry) return;
    return attempt(async () => {
      const { target, plan } = await planAgain(id);
      openJob(target, targetLabel(entry), plan);
    });
  }

  function detach() {
    const id = selected;
    if (!id) return;
    return attempt(async () => {
      await detachRun(id);
      onclose();
    });
  }

  function dock() {
    return attempt(() => dockBack());
  }

  function cancel() {
    const job = live;
    if (!job) return;
    return attempt(() => cancelJob(job.id));
  }
</script>

<div class="run-tab" data-view="run" data-run-tab={selected}>
  {#if list === null}
    <p class="note">Loading…</p>
  {:else if !run}
    <p class="note" data-run-gone>
      This run is no longer kept: it was deleted, or it belongs to another
      workspace.
    </p>
  {:else}
    <header>
      <h2>{runTitle(run)}</h2>
      <span class="meta"
        >{targetLabel(run)} · {when(run.started)} · {runState(run) === "running"
          ? "running"
          : runState(run) === "interrupted"
            ? "interrupted"
            : `${counts(run).done} done`}{counts(run).failed
          ? `, ${counts(run).failed} failed`
          : ""}{counts(run).cancelled
          ? `, ${counts(run).cancelled} cancelled`
          : ""}</span
      >
    </header>
    <section class="run" data-run-details={run.id}>
      {#if run.cancel_reason}
        <!-- Not cancelled by the user (#124). -->
        <p class="note" data-run-cancel-reason>
          Cancelled: {run.cancel_reason}.
        </p>
      {/if}
      <div class="actions">
        {#if isRunning(run.id) && live}
          <button
            type="button"
            data-run-action="cancel"
            disabled={busy}
            onclick={cancel}>Cancel job</button
          >
        {/if}
        {#if detached}
          <button
            type="button"
            class="primary"
            data-run-action="dock"
            title="Show the run as a tab in the main window; this window closes"
            disabled={busy}
            onclick={dock}>Dock back</button
          >
        {:else}
          <button
            type="button"
            data-run-action="detach"
            title="Show the run in a window of its own; this tab closes"
            disabled={busy}
            onclick={detach}>Open in window</button
          >
        {/if}
        {#if !detached && run.step.kind === "enrich" && run.ended}
          {@const id = run.id}
          <button
            type="button"
            data-run-action="review"
            title="What the devices reported, to fill in"
            onclick={() => {
              openView("enrich", id);
            }}>Review…</button
          >
        {/if}
        {#if !detached}
          <button
            type="button"
            data-run-action="again"
            disabled={busy}
            title={run.step.kind === "script"
              ? "Plan the same script for the same target, as the workspace is now"
              : run.step.kind === "sendFile"
                ? "Plan sending the same file to the same target, as the workspace is now"
                : "Plan the same snippet on the same target, as the workspace is now"}
            onclick={again}>Run again…</button
          >
          {#key run.id}
            <SaveJobForm
              step={run.step}
              target={run.target}
              suggested={`${runTitle(run)} on ${targetLabel(run)}`}
            />
          {/key}
        {/if}
        {#if !detached && !isRunning(run.id)}
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

      {#if run.step.kind === "sendFile"}
        {@const file = live?.file ?? run.file}
        <dl class="file" data-run-file={file?.name ?? ""}>
          <dt>File</dt>
          <dd data-run-file-name>
            {file?.name ?? run.step.file}{file
              ? ` · ${sizeText(file.size)} (${file.size} bytes)`
              : ""}
          </dd>
          <dt>Destination</dt>
          <dd data-run-file-destination>{destinationText(run)}</dd>
          <dt>Overwrite</dt>
          <dd data-run-file-overwrite={run.step.overwrite ? "yes" : "no"}>
            {overwriteText(run)}
          </dd>
          {#if file?.hashes}
            <dt>MD5</dt>
            <dd class="hash" data-run-file-md5>{file.hashes.md5}</dd>
            <dt>SHA-256</dt>
            <dd class="hash" data-run-file-sha256>{file.hashes.sha256}</dd>
          {/if}
        </dl>
      {/if}

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
              >{runState(run) === "interrupted" && !isFinal(state)
                ? "Not finished"
                : stateLabel(run, state)}</span
            >
            <span>{entry.name}</span>
            {#if state.kind === "failed"}
              <span class="reason" data-run-reason>{state.reason}</span>
            {/if}
            {#if entry.destination}
              <span class="backup" data-run-destination={entry.destination}
                >{entry.destination}</span
              >
            {/if}
            {#if (live?.bytes[entry.id] ?? entry.bytes) !== undefined}
              {@const bytes = (live?.bytes[entry.id] ?? entry.bytes)!}
              <span class="backup" data-run-bytes={bytes}
                >{sizeText(bytes)} sent</span
              >
            {/if}
            {#if (live?.exits[entry.id] ?? entry.exit_status) !== undefined}
              <span
                class="backup"
                data-run-exit-status={live?.exits[entry.id] ??
                  entry.exit_status}
                >exit status {live?.exits[entry.id] ?? entry.exit_status}</span
              >
            {/if}
            {#if live?.backups[entry.id] ?? entry.backup}
              {@const outcome = (live?.backups[entry.id] ?? entry.backup)!}
              <span class="backup" data-run-backup={outcome.kind}
                >{outcomeText(outcome)}</span
              >
            {/if}
          </button>
        {/each}
        {#each run.targets ?? [] as entry (entry.id)}
          <div class="skipped" data-run-target={entry.id}>
            <span class="state">In inventory</span>
            <span>{entry.name}</span>
            <span class="reason">{entry.address}</span>
          </div>
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
          data-diff-changes={diff.filter((line) => line.kind !== "same").length}
        >
          {#each diff as line, index (index)}
            <div class="line {line.kind}" data-diff-line={line.kind}>
              <span class="number">{line.kind === "added" ? "" : line.old}</span
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

<style>
  .run-tab {
    height: 100%;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3);
    background: var(--bg);
    overflow: auto;
    font-size: var(--text-sm);
  }

  header {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    align-items: baseline;
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
  }

  .hosts button.selected {
    background: var(--surface);
  }

  .meta {
    color: var(--muted);
    font-size: var(--text-sm);
  }

  .run {
    display: flex;
    flex-direction: column;
    gap: var(--space-1-5);
  }

  .actions,
  .row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1-5);
    align-items: center;
  }

  .row input {
    flex: 1;
    min-width: 10rem;
  }

  .row label {
    display: flex;
    gap: var(--space-1);
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
    border-radius: var(--radius-sm);
    max-height: 12rem;
    overflow: auto;
  }

  .hosts > button,
  .hosts > .skipped {
    display: flex;
    gap: var(--space-2);
    align-items: baseline;
    justify-content: normal;
    min-height: 0;
    padding: var(--space-1) var(--space-2);
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
    font-weight: var(--weight-semibold);
  }

  .file {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: var(--space-0-5) var(--space-2);
    margin: 0;
  }

  .file dt {
    color: var(--muted);
  }

  .file dd {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .file .hash {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
  }

  .state.done,
  .state.alreadyThere {
    color: var(--ok);
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
    padding: var(--space-1-5) var(--space-2);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    background: var(--surface);
    border-radius: var(--radius-sm);
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
    gap: var(--space-1);
  }

  .hits li {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: var(--space-0-5) var(--space-2);
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
    min-height: 0;
    padding: 0;
    color: var(--accent, inherit);
    text-decoration: underline;
    cursor: pointer;
  }

  .diff {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    max-height: 22rem;
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
