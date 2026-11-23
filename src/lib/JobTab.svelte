<script lang="ts">
  // A job tab (#69): the job from its start (the snippet or script to run,
  // the variables no host fills, the pre-flight list) to its progress and
  // results. Closing the tab while the job runs leaves it running: the Jobs
  // side view lists it, and opening it there shows its run.
  import { onDestroy, untrack } from "svelte";
  import { open as openDialog } from "@tauri-apps/plugin-dialog";
  import { sizeText } from "./files";
  import {
    cancelJob,
    DEFAULT_PARALLEL,
    listScripts,
    MAX_PARALLEL,
    outcomeText,
    planJob,
    SCRIPT_OUTPUT,
    stepTitle,
    type HostState,
    type JobPlan,
    type JobStep,
    type SaveOutcome,
    type ShownPlan,
  } from "./jobs";
  import { forgetFinished, launchJob, type LiveJob } from "./jobs.svelte";
  import { jobDrafts, showPlan, started } from "./jobTabs.svelte";
  import { inventory } from "./inventory.svelte";
  import SaveJobForm from "./SaveJobForm.svelte";
  import { openView } from "./views";
  import { detachRun } from "./windows";
  import { python, pythonOff, refreshPython } from "./python.svelte";

  interface Props {
    /** The tab's subject: the key of its draft (see `jobTabs.svelte.ts`). */
    subject: string;
    /** Closes the tab; `quiet`: without saying a running job goes on. */
    onclose: (quiet?: boolean) => void;
  }

  let { subject, onclose }: Props = $props();

  const draft = untrack(() => jobDrafts[subject]);
  const target = draft.target;
  const label = draft.label;

  type Step =
    "snippet" | "script" | "file" | "variables" | "preflight" | "running";

  const snippets = $derived(inventory.workspace?.snippets ?? []);
  const initial = draft.planned;
  /** Whether the job backs up configs (fixed for the tab's life). */
  const backingUp = draft.kind === "backup";
  /** Whether the job runs a script (fixed for the tab's life). */
  const scripting = draft.kind === "script";
  /** Whether the job fills in hosts from devices (fixed too). */
  const enriching = draft.kind === "enrich";
  /** Whether a Python process runs (a script, or filling in). */
  const usesPython = scripting || enriching;
  /** Whether the job sends a file (fixed too). */
  const sending = draft.kind === "file";
  const initialStep = initial?.step.kind === "snippet" ? initial.step : null;
  const initialFile = initial?.step.kind === "sendFile" ? initial.step : null;
  let step = $state<Step>(
    backingUp || enriching
      ? "preflight"
      : initial === null
        ? scripting
          ? "script"
          : sending
            ? "file"
            : "snippet"
        : initial.missingVariables.length > 0
          ? "variables"
          : "preflight",
  );
  /** The step to go back to from pre-flight. */
  const firstStep: Step = scripting ? "script" : sending ? "file" : "snippet";

  /** The local file to send, its destination folder and overwrite. */
  let filePath = $state(initialFile?.file ?? "");
  let destination = $state(initialFile?.destination ?? "");
  let overwrite = $state(initialFile?.overwrite ?? false);

  async function browseFile() {
    const chosen = await openDialog({
      title: "Choose the file to send",
      multiple: false,
      directory: false,
    });
    if (typeof chosen === "string") filePath = chosen;
  }

  /** The workspace's scripts, once listed. */
  let scripts = $state<string[] | null>(null);
  let scriptPath = $state(
    initial?.step.kind === "script" ? initial.step.path : "",
  );
  /** Why a script cannot run now (no usable Python), with how to fix it. */
  const scriptOff = $derived(usesPython ? pythonOff() : null);
  // The interpreter may have changed since the app started.
  if (usesPython) void refreshPython();
  if (scripting) {
    listScripts()
      .then((found) => {
        scripts = found;
        if (!scriptPath) scriptPath = found[0] ?? "";
      })
      .catch((reason) => {
        scripts = [];
        error = String(reason);
      });
  }

  /** Opens Preferences (its Python section) in a tab of its own. */
  function openPreferences() {
    openView("preferences");
  }
  let snippet = $state(
    untrack(() => initialStep?.name ?? snippets[0]?.name ?? ""),
  );
  let answers = $state<Record<string, string>>({
    ...(initialStep?.answers ?? {}),
    ...Object.fromEntries(
      (initial?.missingVariables ?? []).map((name) => [name, ""]),
    ),
  });
  /** The plan the pre-flight shows: the shell keeps it under its id, and
   * Start runs exactly it (#121). */
  let plan = $state<ShownPlan | null>(initial);
  let productionConfirmed = $state(false);
  /** Whether the shell refused to start the plan shown (then it can be
   * planned again from here). */
  let refused = $state(false);
  /** How many hosts run at once. */
  let parallel = $state<number | null>(
    initial?.defaultParallel ?? DEFAULT_PARALLEL,
  );
  const parallelValid = $derived(
    parallel !== null &&
      Number.isInteger(parallel) &&
      parallel >= 1 &&
      parallel <= MAX_PARALLEL,
  );
  let error = $state<string | null>(null);
  let busy = $state(false);

  /** The running (or finished) job, kept beyond this tab. */
  let live = $state<LiveJob | null>(null);
  const finished = $derived(live?.finished ?? false);
  const states = $derived<Record<string, HostState>>(live?.states ?? {});
  const outputs = $derived<Record<string, string>>(live?.outputs ?? {});
  const backups = $derived<Record<string, SaveOutcome>>(live?.backups ?? {});
  const progress = $derived<Record<string, [number, number | null]>>(
    live?.progress ?? {},
  );

  /** Closes the tab; a running job goes on (see the Jobs side view). */
  function close(quiet = false) {
    onclose(quiet);
  }

  // A finished job's progress is no longer needed here: its run is kept.
  onDestroy(() => {
    if (live?.finished) forgetFinished();
  });

  let firstInput = $state<HTMLElement>();
  $effect(() => {
    void step;
    firstInput?.focus();
  });

  async function attempt(action: () => Promise<void>) {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await action();
    } catch (reason) {
      error = String(reason);
    } finally {
      busy = false;
    }
  }

  /** The step the job runs. */
  function jobStep(): JobStep {
    if (backingUp) return { kind: "configBackup" };
    if (enriching) return { kind: "enrich" };
    if (scripting) return { kind: "script", path: scriptPath };
    if (sending) {
      return {
        kind: "sendFile",
        file: filePath.trim(),
        destination: destination.trim(),
        overwrite,
      };
    }
    return { kind: "snippet", name: snippet, answers: { ...answers } };
  }

  // A config backup has nothing to choose first: straight to pre-flight.
  if ((backingUp || enriching) && initial === null) void preflight();

  /** Plans with the answers so far; asks for missing variables first. */
  function preflight() {
    return attempt(async () => {
      const planned = await planJob(target, jobStep());
      // The plan shown before is forgotten by the shell.
      showPlan(subject, planned.id);
      // A number the user changed is kept when planning again.
      if (plan === null || parallel === plan.defaultParallel) {
        parallel = planned.defaultParallel;
      }
      plan = planned;
      productionConfirmed = false;
      refused = false;
      if (planned.missingVariables.length > 0) {
        for (const name of planned.missingVariables) answers[name] ??= "";
        step = "variables";
      } else {
        step = "preflight";
      }
    });
  }

  function start() {
    return attempt(async () => {
      if (!plan) return;
      try {
        live = await launchJob(
          plan,
          // What this pre-flight's question confirmed: the shell refuses a
          // plan whose production hosts are not all named.
          productionConfirmed ? [...plan.production] : [],
          target,
          label,
          parallel ?? plan.defaultParallel,
        );
      } catch (reason) {
        refused = true;
        throw reason;
      }
      started(subject, live.run);
      step = "running";
    });
  }

  /** Planned hosts the run would refuse for a host key (#129). */
  const untrustedKeys = $derived(
    plan?.hosts.filter((host) => host.hostKeys).length ?? 0,
  );

  const ready = $derived(
    plan !== null &&
      plan.hosts.length > 0 &&
      plan.missingVariables.length === 0 &&
      parallelValid &&
      scriptOff === null &&
      (plan.production.length === 0 || productionConfirmed),
  );

  /** The production hosts that will run, by name (their id if names clash). */
  function productionNames(plan: JobPlan): string[] {
    return plan.production.map((id) => {
      const host = plan.hosts.find((entry) => entry.id === id);
      const name = host?.name ?? id;
      const clash =
        plan.hosts.filter((entry) => entry.name === name).length > 1;
      return clash ? `${name} (${id})` : name;
    });
  }

  const STATE_LABELS: Record<HostState["kind"], string> = {
    waiting: "Waiting",
    connecting: "Connecting",
    running: sending ? "Sending" : "Running",
    done: sending ? "Sent" : "Done",
    alreadyThere: "Already there",
    failed: "Failed",
    cancelled: "Cancelled",
  };

  /** A host's progress while sending, for the user: "1.2 MiB of 4.0 MiB (30 %)". */
  function progressText(done: number, total: number | null): string {
    if (total === null || total === 0) return sizeText(done);
    const percent = Math.floor((done / total) * 100);
    return `${sizeText(done)} of ${sizeText(total)} (${percent} %)`;
  }

  const counts = $derived.by(() => {
    const count = { done: 0, alreadyThere: 0, failed: 0, cancelled: 0 };
    for (const state of Object.values(states)) {
      if (state.kind in count) count[state.kind as keyof typeof count] += 1;
    }
    return count;
  });
</script>

<div
  class="job-tab"
  role="region"
  aria-labelledby="job-title-{subject}"
  data-view="job"
  data-step={step}
  data-plan={plan?.id}
>
  <h2 id="job-title-{subject}">
    {backingUp
      ? `Back up configs of ${label}`
      : enriching
        ? `Fill in ${label} from devices`
        : scripting
          ? `Run a script for ${label}`
          : sending
            ? `Send a file to ${label}`
            : `Run a snippet on ${label}`}
  </h2>

  {#if scriptOff !== null && step !== "running"}
    <div class="python-off" role="status" data-job-python-off>
      <p>{python.busy ? "Checking the Python interpreter…" : scriptOff}</p>
      <button
        type="button"
        data-job-python-preferences
        onclick={openPreferences}>Open Preferences</button
      >
    </div>
  {/if}

  {#if step === "file"}
    <form
      data-job-file-form
      onsubmit={(event) => {
        event.preventDefault();
        void preflight();
      }}
    >
      <label>
        File
        <span class="file-row">
          <input
            name="job-file"
            bind:this={firstInput}
            bind:value={filePath}
            placeholder="The file on this computer"
            autocomplete="off"
            spellcheck="false"
          />
          <button type="button" data-job="browse-file" onclick={browseFile}
            >Browse…</button
          >
        </span>
      </label>
      <label>
        Destination folder
        <input
          name="job-destination"
          bind:value={destination}
          placeholder="Empty: each host's platform file folder"
          autocomplete="off"
          spellcheck="false"
        />
      </label>
      <label class="check">
        <input type="checkbox" name="job-overwrite" bind:checked={overwrite} />
        <span>Overwrite a file of that name already on the device</span>
      </label>
      <p class="note" data-job-file-note>
        The file keeps its name. A typed folder applies to every host and may
        use the host variables snippets use, such as <code
          >{"/images/{{site}}"}</code
        >. Each host logs in with its own credential profile over a file
        connection of its own (SFTP, or SCP where the device has no SFTP).
        Without overwrite, a host that already has a file of that name and size
        counts as done, and one with another size fails. Nothing is sent before
        you start.
      </p>
      <div class="actions">
        <button
          type="submit"
          class="primary"
          data-job="plan"
          disabled={busy || !filePath.trim()}>Next</button
        >
      </div>
    </form>
  {:else if step === "script"}
    {#if scripts === null}
      <p class="note">Looking for scripts…</p>
    {:else if scripts.length === 0}
      <p class="note" data-job-no-scripts>
        The workspace has no Python scripts yet: put a <code>.py</code> file in its
        folder (or a folder below it) first.
      </p>
    {:else}
      <form
        onsubmit={(event) => {
          event.preventDefault();
          void preflight();
        }}
      >
        <label>
          Script
          <select
            name="job-script"
            bind:this={firstInput}
            bind:value={scriptPath}
          >
            {#each scripts as path (path)}
              <option value={path}>{path}</option>
            {/each}
          </select>
        </label>
        <p class="note" data-job-script-note>
          The script runs once, on this computer, with the Python chosen in
          Preferences, in the workspace folder. Its inventory (the enodix
          package and its Nornir plugin) holds only the hosts of this job, with
          the logins of their credential profiles; it connects to them itself.
          Nothing runs before you start.
        </p>
        <div class="actions">
          <button
            type="submit"
            class="primary"
            data-job="plan"
            disabled={busy || !scriptPath}>Next</button
          >
        </div>
      </form>
    {/if}
  {:else if step === "snippet"}
    {#if snippets.length === 0}
      <p class="note">
        The workspace has no snippets yet: add one in the snippets panel first.
      </p>
    {:else}
      <form
        onsubmit={(event) => {
          event.preventDefault();
          void preflight();
        }}
      >
        <label>
          Snippet
          <select
            name="job-snippet"
            bind:this={firstInput}
            bind:value={snippet}
          >
            {#each snippets as entry (entry.name)}
              <option value={entry.name}>{entry.name}</option>
            {/each}
          </select>
        </label>
        <pre class="text" data-job-snippet-text>{snippets.find(
            (entry) => entry.name === snippet,
          )?.text ?? ""}</pre>
        <p class="note">
          Each host logs in with its own credential profile, over its own
          session (your terminals are not touched), and gets the snippet line by
          line, waiting for its prompt after each. Nothing is sent before you
          start.
        </p>
        <div class="actions">
          <button
            type="submit"
            class="primary"
            data-job="plan"
            disabled={busy || !snippet}>Next</button
          >
        </div>
      </form>
    {/if}
  {:else if step === "variables" && plan}
    <form
      onsubmit={(event) => {
        event.preventDefault();
        void preflight();
      }}
    >
      <p class="note">
        Some hosts do not fill these variables of {snippet}. The values given
        here apply to every host that lacks them.
      </p>
      {#each plan.missingVariables as name, index (name)}
        <label>
          {name}
          {#if index === 0}
            <input
              name="job-variable-{index}"
              data-variable={name}
              bind:this={firstInput}
              bind:value={answers[name]}
              autocomplete="off"
              spellcheck="false"
            />
          {:else}
            <input
              name="job-variable-{index}"
              data-variable={name}
              bind:value={answers[name]}
              autocomplete="off"
              spellcheck="false"
            />
          {/if}
        </label>
      {/each}
      <div class="actions">
        <button type="button" onclick={() => (step = "snippet")}>Back</button>
        <button type="submit" class="primary" data-job="plan" disabled={busy}
          >Next</button
        >
      </div>
    </form>
  {:else if step === "preflight" && !plan}
    <p class="note">{busy ? "Working out which hosts can run…" : ""}</p>
  {:else if step === "preflight" && plan}
    {#if backingUp}
      <p class="note" data-job-backup-note>
        Each host logs in with its own credential profile and shows its
        configuration with its platform's show-config command. A new version is
        kept in the host's config history only if the configuration changed.
        Backups are kept outside the workspace, unencrypted: they are as
        sensitive as the configurations themselves.
      </p>
    {/if}
    {#if enriching}
      <p class="note" data-job-enrich-note>
        Each host is asked for its vendor, model, serial number and OS version
        with NAPALM (in the Python chosen in Preferences), logging in with its
        own credential profile. Nothing is written yet: you see what the devices
        report and choose what to fill in afterwards.
      </p>
    {/if}
    <p class="note" data-job-summary>
      {#if enriching}
        {plan.hosts.length === 1
          ? "1 host is asked"
          : `${plan.hosts.length} hosts are asked`},
        {plan.skipped.length} skipped.
      {:else if scripting}
        {plan.step.kind === "script" ? plan.step.path : ""} runs for
        {plan.hosts.length === 1 ? "1 host" : `${plan.hosts.length} hosts`},
        {plan.skipped.length} skipped.
      {:else if sending && plan.file}
        <strong data-job-file-name>{plan.file.name}</strong>
        (<span data-job-file-size={plan.file.size}
          >{sizeText(plan.file.size)}</span
        >) goes to
        {plan.hosts.length === 1 ? "1 host" : `${plan.hosts.length} hosts`},
        {plan.skipped.length} skipped{plan.step.kind === "sendFile" &&
        plan.step.overwrite
          ? "; a file of that name is overwritten"
          : ""}.
      {:else}
        {plan.hosts.length === 1
          ? "1 host runs"
          : `${plan.hosts.length} hosts run`},
        {plan.skipped.length} skipped.
      {/if}
      {#if untrustedKeys > 0}
        <span class="untrusted" data-job-untrusted={untrustedKeys}
          >{untrustedKeys === 1
            ? "1 host has a host key not trusted yet and will be refused."
            : `${untrustedKeys} hosts have a host key not trusted yet and will be refused.`}</span
        >
      {/if}
    </p>
    <div class="scroll">
      <table>
        <thead>
          <tr><th></th><th>Host</th><th>Address</th><th>Details</th></tr>
        </thead>
        <tbody>
          {#each plan.hosts as host (host.id)}
            <tr
              data-job-host={host.id}
              class:production={host.environment === "production"}
            >
              <td class="outcome"
                >{enriching
                  ? "Asked"
                  : scripting
                    ? "In inventory"
                    : sending
                      ? "Gets it"
                      : "Runs"}</td
              >
              <td>{host.name}</td>
              <td>{host.address}</td>
              <td class="details" data-job-destination={host.destination}>
                {[
                  host.destination ? `To ${host.destination}` : "",
                  host.platform ? `Platform ${host.platform}` : "",
                  host.environment === "production" ? "production" : "",
                ]
                  .filter(Boolean)
                  .join(" · ")}
                {#if host.hostKeys}
                  <span
                    class="host-key-warning"
                    data-job-host-key={host.hostKeys.keys.some(
                      (key) => key.jumpHost,
                    )
                      ? "jumpHost"
                      : "device"}>{host.hostKeys.message}</span
                  >
                {/if}
              </td>
            </tr>
          {/each}
          {#each plan.skipped as host (host.id)}
            <tr
              class="skipped"
              data-job-skipped={host.id}
              data-skip-reason={host.reason.kind}
            >
              <td class="outcome">Skipped</td>
              <td>{host.name}</td>
              <td></td>
              <td class="details">{host.message}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
    {#if plan.production.length > 0}
      <label class="production-confirm" data-job-production>
        <input
          type="checkbox"
          name="job-confirm-production"
          bind:this={firstInput}
          bind:checked={productionConfirmed}
        />
        <span>
          {enriching
            ? "Ask"
            : scripting
              ? "Run the script for"
              : sending
                ? "Send to"
                : "Run on"}
          {plan.production.length === 1
            ? "the production host"
            : `${plan.production.length} production hosts`}:
          <strong data-job-production-names
            >{productionNames(plan).join(", ")}</strong
          >
        </span>
      </label>
    {/if}
    {#if !usesPython}
      <label class="parallel" data-job-parallel>
        <span>Hosts at once</span>
        <input
          type="number"
          name="job-parallel"
          min="1"
          max={MAX_PARALLEL}
          step="1"
          bind:value={parallel}
          aria-invalid={!parallelValid}
        />
        <span class="hint">1 to {MAX_PARALLEL}</span>
      </label>
    {/if}
    {#if !backingUp && !enriching}
      <div class="save-row">
        <SaveJobForm
          step={plan.step}
          {target}
          suggested={`${stepTitle(plan.step)} on ${label}`}
        />
      </div>
    {/if}
    <div class="actions">
      {#if !backingUp && !enriching}
        <button type="button" onclick={() => (step = firstStep)}>Back</button>
      {/if}
      {#if refused}
        <button
          type="button"
          data-job="replan"
          disabled={busy}
          onclick={preflight}>Plan again</button
        >
      {/if}
      <button
        type="button"
        class="primary"
        data-job="start"
        disabled={busy || !ready}
        onclick={start}
        >{enriching
          ? "Ask"
          : scripting
            ? "Run the script for"
            : sending
              ? "Send to"
              : "Run on"}
        {plan.hosts.length}
        {plan.hosts.length === 1 ? "host" : "hosts"}</button
      >
    </div>
  {:else if step === "running" && plan && usesPython}
    {@const state = states[SCRIPT_OUTPUT] ?? { kind: "waiting" }}
    {@const exit = live?.exits[SCRIPT_OUTPUT]}
    <p class="note" data-job-progress data-finished={finished ? "" : undefined}>
      {finished
        ? state.kind === "done"
          ? "Finished: the script succeeded"
          : state.kind === "cancelled"
            ? "Finished: cancelled"
            : "Finished: the script failed"
        : "Running"}{exit !== undefined ? ` (exit status ${exit})` : ""}.
      {#if enriching && finished}
        Review what the devices reported before anything is written.
      {/if}
    </p>
    <div class="scroll results">
      <details
        data-job-result={SCRIPT_OUTPUT}
        data-host-state={state.kind}
        data-exit-status={exit}
        open
      >
        <summary>
          <span class="state {state.kind}">{STATE_LABELS[state.kind]}</span>
          <span
            >{plan.step.kind === "script"
              ? plan.step.path
              : stepTitle(plan.step)}</span
          >
          {#if state.kind === "failed"}
            <span class="reason" data-job-reason>{state.reason}</span>
          {/if}
        </summary>
        <pre data-job-output={SCRIPT_OUTPUT}>{outputs[SCRIPT_OUTPUT] ??
            ""}</pre>
      </details>
    </div>
    <div class="actions">
      {#if !finished && live !== null}
        {@const id = live.id}
        <button
          type="button"
          data-job="cancel"
          onclick={() => cancelJob(id).catch((e) => (error = String(e)))}
          >Cancel job</button
        >
      {/if}
      {#if live !== null && enriching && finished}
        {@const run = live.run}
        <button
          type="button"
          class="primary"
          data-job="review"
          onclick={() => {
            // The review takes the job tab's place in its pane.
            openView("enrich", run);
            close(true);
          }}>Review what the devices reported</button
        >
      {/if}
      {#if live !== null}
        {@const run = live.run}
        <button
          type="button"
          data-job="view"
          title="The kept run, with search, compare, save and run again"
          onclick={() => {
            openView("run", run);
            close(true);
          }}>Open run</button
        >
        <button
          type="button"
          data-job="detach"
          title="Show the run in a window of its own; this tab closes"
          onclick={() =>
            detachRun(run).then(
              () => close(true),
              (reason) => (error = String(reason)),
            )}>Open in window</button
        >
      {/if}
      <button
        type="button"
        class="primary"
        data-job="close"
        title={finished
          ? "Close this tab; the run is kept in Jobs"
          : "Close this tab; the job goes on running (see Jobs)"}
        onclick={() => close()}>Close tab</button
      >
    </div>
  {:else if step === "running" && plan}
    <p class="note" data-job-progress data-finished={finished ? "" : undefined}>
      {finished ? "Finished" : "Running"}: {counts.done}
      {sending ? "sent" : "done"}{counts.alreadyThere
        ? `, ${counts.alreadyThere} already there`
        : ""}, {counts.failed}
      failed{counts.cancelled ? `, ${counts.cancelled} cancelled` : ""} of
      {plan.hosts.length}.
    </p>
    <div class="scroll results">
      {#each plan.hosts as host (host.id)}
        {@const state = states[host.id] ?? { kind: "waiting" }}
        <details
          data-job-result={host.id}
          data-host-state={state.kind}
          open={plan.hosts.length === 1}
        >
          <summary>
            <span class="state {state.kind}">{STATE_LABELS[state.kind]}</span>
            <span>{host.name}</span>
            {#if state.kind === "failed"}
              <span class="reason" data-job-reason>{state.reason}</span>
            {/if}
            {#if backups[host.id]}
              <span
                class="backup"
                data-job-backup={backups[host.id].kind}
                data-version={backups[host.id].version}
                >{outcomeText(backups[host.id])}</span
              >
            {/if}
            {#if sending && state.kind === "running" && progress[host.id]}
              {@const [done, total] = progress[host.id]}
              <span
                class="sending"
                data-job-sending={host.id}
                data-done={done}
                data-total={total}
              >
                <progress value={done} max={total ?? undefined}></progress>
                {progressText(done, total)}
              </span>
            {/if}
            {#if sending && live?.bytes[host.id] !== undefined}
              <span class="backup" data-job-bytes={live.bytes[host.id]}
                >{sizeText(live.bytes[host.id])}</span
              >
            {/if}
          </summary>
          <pre data-job-output={host.id}>{outputs[host.id] ?? ""}</pre>
        </details>
      {/each}
    </div>
    <div class="actions">
      {#if !finished && live !== null}
        {@const id = live.id}
        <button
          type="button"
          data-job="cancel"
          onclick={() => cancelJob(id).catch((e) => (error = String(e)))}
          >Cancel job</button
        >
      {/if}
      {#if live !== null}
        {@const run = live.run}
        <button
          type="button"
          data-job="view"
          title="The kept run, with search, compare, save and run again"
          onclick={() => {
            openView("run", run);
            close(true);
          }}>Open run</button
        >
        <button
          type="button"
          data-job="detach"
          title="Show the run in a window of its own; this tab closes"
          onclick={() =>
            detachRun(run).then(
              () => close(true),
              (reason) => (error = String(reason)),
            )}>Open in window</button
        >
      {/if}
      <button
        type="button"
        class="primary"
        data-job="close"
        title={finished
          ? "Close this tab; the run is kept in Jobs"
          : "Close this tab; the job goes on running (see Jobs)"}
        onclick={() => close()}>Close tab</button
      >
    </div>
  {/if}

  {#if error}
    <p class="error" role="alert" data-job-error>{error}</p>
  {/if}
  {#if step !== "running"}
    <footer>
      <button
        type="button"
        data-job="dismiss"
        disabled={busy}
        onclick={() => close()}>Cancel</button
      >
    </footer>
  {/if}
</div>

<style>
  .job-tab {
    height: 100%;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3);
    background: var(--bg);
    overflow: auto;
    font-size: var(--text-sm);
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
  }

  form {
    display: grid;
    gap: var(--space-1-5);
  }

  label {
    display: grid;
    gap: var(--space-0-5);
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
    max-height: 8rem;
    overflow: auto;
  }

  .scroll {
    overflow: auto;
    min-height: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }

  table {
    width: 100%;
    border-collapse: collapse;
  }

  th,
  td {
    text-align: left;
    padding: var(--space-1) var(--space-2);
    border-bottom: 1px solid var(--border);
    vertical-align: middle;
  }

  th {
    position: sticky;
    top: 0;
    background: var(--surface);
    font-weight: var(--weight-semibold);
  }

  .details {
    color: var(--muted);
    overflow-wrap: anywhere;
  }

  .outcome {
    white-space: nowrap;
  }

  tr.skipped td {
    color: var(--muted);
  }

  .host-key-warning {
    display: block;
    color: var(--warning);
  }

  .untrusted {
    color: var(--warning);
  }

  tr.production .outcome {
    color: var(--env-production);
    font-weight: var(--weight-semibold);
  }

  .parallel {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2);
  }
  .parallel input {
    width: 5rem;
  }
  .parallel input[aria-invalid="true"] {
    border-color: var(--danger);
  }
  .parallel .hint {
    color: var(--muted, inherit);
    font-size: var(--text-sm);
  }
  .production-confirm {
    display: flex;
    gap: var(--space-2);
    align-items: center;
    padding: var(--space-1-5) var(--space-2);
    border: 1px solid var(--env-production);
    border-radius: var(--radius-sm);
  }

  .results details {
    border-bottom: 1px solid var(--border);
  }

  .results summary {
    display: flex;
    gap: var(--space-2);
    align-items: baseline;
    padding: var(--space-1) var(--space-2);
    cursor: pointer;
  }

  .results pre {
    margin: 0 var(--space-2) var(--space-1-5);
    max-height: 20rem;
    overflow: auto;
  }

  .state {
    min-width: 6rem;
    font-weight: var(--weight-semibold);
  }

  .state.done,
  .state.alreadyThere {
    color: var(--ok);
  }

  .sending {
    display: inline-flex;
    gap: var(--space-1-5);
    align-items: center;
    color: var(--muted);
  }

  .file-row {
    display: flex;
    gap: var(--space-1-5);
  }

  .file-row input {
    flex: 1 1 auto;
    min-width: 0;
  }

  .check {
    display: flex;
    gap: var(--space-2);
    align-items: center;
  }

  .state.failed,
  .reason {
    color: var(--danger);
  }

  .backup {
    color: var(--muted);
  }

  .state.cancelled {
    color: var(--warning);
  }

  .note {
    margin: 0;
    color: var(--muted);
  }

  .python-off {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    align-items: center;
    justify-content: space-between;
    padding: var(--space-1-5) var(--space-2);
    border: 1px solid var(--warning, var(--border));
    border-radius: var(--radius-sm);
  }

  .python-off p {
    margin: 0;
    flex: 1 1 20rem;
  }

  .save-row {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    align-items: center;
  }

  .actions,
  footer {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }

  .error {
    margin: 0;
    color: var(--danger);
    overflow-wrap: anywhere;
  }
</style>
