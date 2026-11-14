<script lang="ts">
  import { untrack } from "svelte";
  import {
    cancelJob,
    planJob,
    startJob,
    type HostState,
    type JobEvent,
    type JobPlan,
    type JobTarget,
  } from "./jobs";
  import { inventory } from "./inventory.svelte";

  interface Props {
    /** The hosts to run on. */
    target: JobTarget;
    /** What the target is called (a folder, a saved filter, a host). */
    label: string;
    onclose: () => void;
  }

  let { target, label, onclose }: Props = $props();

  type Step = "snippet" | "variables" | "preflight" | "running";

  const snippets = $derived(inventory.workspace?.snippets ?? []);
  let step = $state<Step>("snippet");
  let snippet = $state(untrack(() => snippets[0]?.name ?? ""));
  let answers = $state<Record<string, string>>({});
  let plan = $state<JobPlan | null>(null);
  let productionConfirmed = $state(false);
  let error = $state<string | null>(null);
  let busy = $state(false);

  /** The running (or finished) job. */
  let jobId = $state<number | null>(null);
  let finished = $state(false);
  let states = $state<Record<string, HostState>>({});
  let outputs = $state<Record<string, string>>({});

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

  /** Plans with the answers so far; asks for missing variables first. */
  function preflight() {
    return attempt(async () => {
      const planned = await planJob(target, {
        kind: "snippet",
        name: snippet,
        answers: { ...answers },
      });
      plan = planned;
      productionConfirmed = false;
      if (planned.missingVariables.length > 0) {
        for (const name of planned.missingVariables) answers[name] ??= "";
        step = "variables";
      } else {
        step = "preflight";
      }
    });
  }

  function onevent(event: JobEvent) {
    if (event.kind === "state") {
      states[event.host] = event.state;
    } else if (event.kind === "output") {
      outputs[event.host] = (outputs[event.host] ?? "") + event.text;
    } else {
      for (const host of event.hosts) {
        states[host.id] = host.state;
        outputs[host.id] = host.output;
      }
      finished = true;
    }
  }

  function start() {
    return attempt(async () => {
      states = {};
      outputs = {};
      finished = false;
      jobId = await startJob(onevent);
      step = "running";
    });
  }

  const ready = $derived(
    plan !== null &&
      plan.hosts.length > 0 &&
      plan.missingVariables.length === 0 &&
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
    running: "Running",
    done: "Done",
    failed: "Failed",
    cancelled: "Cancelled",
  };

  const counts = $derived.by(() => {
    const count = { done: 0, failed: 0, cancelled: 0 };
    for (const state of Object.values(states)) {
      if (state.kind in count) count[state.kind as keyof typeof count] += 1;
    }
    return count;
  });

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && !busy) {
      event.preventDefault();
      onclose();
    }
  }
</script>

<svelte:window {onkeydown} />

<div class="backdrop">
  <div
    class="dialog"
    role="dialog"
    aria-modal="true"
    aria-labelledby="job-title"
    data-dialog="job"
    data-step={step}
  >
    <h2 id="job-title">Run a snippet on {label}</h2>

    {#if step === "snippet"}
      {#if snippets.length === 0}
        <p class="note">
          The workspace has no snippets yet: add one in the snippets panel
          first.
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
            session (your terminals are not touched), and gets the snippet line
            by line, waiting for its prompt after each. Nothing is sent before
            you start.
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
    {:else if step === "preflight" && plan}
      <p class="note" data-job-summary>
        {plan.hosts.length === 1
          ? "1 host runs"
          : `${plan.hosts.length} hosts run`},
        {plan.skipped.length} skipped.
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
                <td class="outcome">Runs</td>
                <td>{host.name}</td>
                <td>{host.address}</td>
                <td class="details">
                  Platform {host.platform}{host.environment === "production"
                    ? " · production"
                    : ""}
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
            Run on {plan.production.length === 1
              ? "the production host"
              : `${plan.production.length} production hosts`}:
            <strong data-job-production-names
              >{productionNames(plan).join(", ")}</strong
            >
          </span>
        </label>
      {/if}
      <div class="actions">
        <button type="button" onclick={() => (step = "snippet")}>Back</button>
        <button
          type="button"
          class="primary"
          data-job="start"
          disabled={busy || !ready}
          onclick={start}
          >Run on {plan.hosts.length}
          {plan.hosts.length === 1 ? "host" : "hosts"}</button
        >
      </div>
    {:else if step === "running" && plan}
      <p
        class="note"
        data-job-progress
        data-finished={finished ? "" : undefined}
      >
        {finished ? "Finished" : "Running"}: {counts.done} done, {counts.failed}
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
            </summary>
            <pre data-job-output={host.id}>{outputs[host.id] ?? ""}</pre>
          </details>
        {/each}
      </div>
      <div class="actions">
        {#if !finished && jobId !== null}
          {@const id = jobId}
          <button
            type="button"
            data-job="cancel"
            onclick={() => cancelJob(id).catch((e) => (error = String(e)))}
            >Cancel job</button
          >
        {/if}
        <button type="button" class="primary" data-job="close" onclick={onclose}
          >{finished ? "Close" : "Hide"}</button
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
          onclick={onclose}>Cancel</button
        >
      </footer>
    {/if}
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 22;
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding: 6vh 1rem 1rem;
    background: rgb(0 0 0 / 0.35);
  }

  .dialog {
    width: min(60rem, 100%);
    max-height: 88vh;
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

  h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
  }

  form {
    display: grid;
    gap: 0.4rem;
  }

  label {
    display: grid;
    gap: 0.2rem;
  }

  input,
  select {
    padding: 0.3rem 0.5rem;
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

  .text {
    max-height: 8rem;
    overflow: auto;
  }

  .scroll {
    overflow: auto;
    min-height: 0;
    border: 1px solid var(--border);
    border-radius: 4px;
  }

  table {
    width: 100%;
    border-collapse: collapse;
  }

  th,
  td {
    text-align: left;
    padding: 0.25rem 0.5rem;
    border-bottom: 1px solid var(--border);
    vertical-align: middle;
  }

  th {
    position: sticky;
    top: 0;
    background: var(--surface);
    font-weight: 600;
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

  tr.production .outcome {
    color: var(--env-production, var(--danger));
    font-weight: 600;
  }

  .production-confirm {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    padding: 0.4rem 0.5rem;
    border: 1px solid var(--env-production, var(--danger));
    border-radius: 4px;
  }

  .results details {
    border-bottom: 1px solid var(--border);
  }

  .results summary {
    display: flex;
    gap: 0.6rem;
    align-items: baseline;
    padding: 0.3rem 0.5rem;
    cursor: pointer;
  }

  .results pre {
    margin: 0 0.5rem 0.4rem;
    max-height: 20rem;
    overflow: auto;
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

  .state.cancelled {
    color: var(--warning);
  }

  .note {
    margin: 0;
    color: var(--muted);
  }

  .actions,
  footer {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
  }

  .error {
    margin: 0;
    color: var(--danger);
    overflow-wrap: anywhere;
  }
</style>
