<script lang="ts">
  // Setting up the app's own Python environment: the Python found and where
  // the environment goes, the libraries to tick, then a confirmation listing
  // everything that will be installed; nothing is installed before
  // "Install". The setup's output streams in while it runs. A tab of its
  // own (#73; one at a time); closing the tab while the setup runs cancels
  // it.
  import { onDestroy } from "svelte";
  import {
    cancelSetup,
    findPython,
    planSetup,
    refreshPython,
    runSetup,
    versionText,
    type Library,
    type SetupOffer,
    type SetupPlan,
  } from "./python.svelte";

  interface Props {
    /** Closes the tab. */
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  type Step = "finding" | "offer" | "confirm" | "running" | "done" | "failed";

  /** Lines of output kept on screen. */
  const KEPT_LINES = 300;

  let step = $state<Step>("finding");
  let offer = $state<SetupOffer | null>(null);
  let plan = $state<SetupPlan | null>(null);
  let ticked = $state<Library[]>(["nornir", "netmiko"]);
  let error = $state<string | null>(null);
  let doing = $state("");
  let output = $state<string[]>([]);
  let result = $state("");
  let cancelling = $state(false);
  let outputBox = $state<HTMLElement>();

  findPython().then(
    (found) => {
      offer = found;
      step = "offer";
    },
    (failure) => {
      error = String(failure);
      step = "failed";
    },
  );

  $effect(() => {
    // Keeps the newest output in view.
    void output.length;
    outputBox?.scrollTo({ top: outputBox.scrollHeight });
  });

  function toggle(library: Library, on: boolean) {
    ticked = on
      ? [...ticked.filter((other) => other !== library), library]
      : ticked.filter((other) => other !== library);
  }

  async function confirm() {
    error = null;
    try {
      plan = await planSetup(ticked);
      step = "confirm";
    } catch (failure) {
      error = String(failure);
    }
  }

  async function install() {
    step = "running";
    output = [];
    doing = "";
    try {
      const view = await runSetup((progress) => {
        if (progress.kind === "step") {
          doing = progress.description;
          output = [...output, `> ${progress.description}`].slice(-KEPT_LINES);
        } else {
          output = [...output, progress.line].slice(-KEPT_LINES);
        }
      });
      result = view.status.message;
      step = "done";
    } catch (failure) {
      error = String(failure);
      step = "failed";
      void refreshPython();
    }
  }

  function cancel() {
    cancelling = true;
    void cancelSetup();
  }

  // The tab closed while the setup runs: nobody would see it end.
  onDestroy(() => {
    if (step === "running" && !cancelling) void cancelSetup();
  });
</script>

<div class="setup" data-view="python-setup" data-step={step}>
  <section aria-labelledby="python-setup-title">
    <h2 id="python-setup-title">The app's Python environment</h2>

    {#if step === "finding"}
      <p>Looking for Python 3.10 or newer…</p>
    {:else if step === "offer" && offer}
      <dl>
        <dt>Python found</dt>
        <dd>
          Python {versionText(offer.base.version)},
          <code data-python-base>{offer.base.executable}</code>
        </dd>
        <dt>Environment</dt>
        <dd>
          <code data-python-environment>{offer.environment}</code>
          {#if offer.existing}(already there: it is updated){/if}
        </dd>
      </dl>
      <p>
        The system's Python is not changed: the app makes a virtual environment
        of its own and installs its package from the copy shipped with the app.
        Tick the libraries to install as well:
      </p>
      <div class="libraries">
        {#each offer.libraries as offered (offered.library)}
          <label class="check">
            <input
              type="checkbox"
              name={`python-library-${offered.library}`}
              checked={ticked.includes(offered.library)}
              onchange={(event) =>
                toggle(offered.library, event.currentTarget.checked)}
            />
            <span><strong>{offered.label}</strong>: {offered.purpose}</span>
          </label>
        {/each}
      </div>
      {#if error}<p class="error" role="alert">{error}</p>{/if}
      <div class="actions">
        <button type="button" onclick={onclose}>Cancel</button>
        <button
          type="button"
          data-python-setup="continue"
          onclick={() => void confirm()}>Continue…</button
        >
      </div>
    {:else if step === "confirm" && plan}
      <p>
        {plan.existing ? "Update" : "Create"} the environment
        <code>{plan.environment}</code> with Python {versionText(
          plan.base.version,
        )}, then install:
      </p>
      <ul class="installs">
        {#each plan.installs as planned (planned.name)}
          <li
            data-python-install={planned.name}
            data-source={planned.source.kind}
          >
            <code>{planned.name}</code>
            {planned.source.kind === "wheel"
              ? "— the app's package, from the file shipped with the app"
              : "— from the package index (with what it needs)"}
          </li>
        {/each}
      </ul>
      <p class="hint">
        Python features will then use <code>{plan.interpreter}</code>.
      </p>
      <div class="actions">
        <button
          type="button"
          data-python-setup="back"
          onclick={() => (step = "offer")}>Back</button
        >
        <!-- svelte-ignore a11y_autofocus -->
        <button
          type="button"
          autofocus
          data-python-setup="install"
          onclick={() => void install()}>Install</button
        >
      </div>
    {:else if step === "running"}
      <p data-python-doing>{doing || "Starting…"}</p>
      <pre bind:this={outputBox} data-python-output>{output.join("\n")}</pre>
      <div class="actions">
        <button
          type="button"
          data-python-setup="cancel"
          disabled={cancelling}
          onclick={cancel}>{cancelling ? "Cancelling…" : "Cancel"}</button
        >
      </div>
    {:else if step === "done"}
      <p data-python-setup-result="done">{result}</p>
      {#if output.length}
        <details>
          <summary>Output</summary>
          <pre data-python-output>{output.join("\n")}</pre>
        </details>
      {/if}
      <div class="actions">
        <!-- svelte-ignore a11y_autofocus -->
        <button type="button" autofocus onclick={onclose}>Close</button>
      </div>
    {:else if step === "failed"}
      <p class="error" role="alert" data-python-setup-result="failed">
        {error}
      </p>
      {#if output.length}
        <pre data-python-output>{output.join("\n")}</pre>
      {/if}
      <div class="actions">
        {#if offer}
          <button
            type="button"
            data-python-setup="back"
            onclick={() => {
              error = null;
              step = "offer";
            }}>Back</button
          >
        {/if}
        <!-- svelte-ignore a11y_autofocus -->
        <button type="button" autofocus onclick={onclose}>Close</button>
      </div>
    {/if}
  </section>
</div>

<style>
  .setup {
    height: 100%;
    min-height: 0;
    overflow: auto;
    background: var(--bg);
    font-size: var(--text-sm);
  }

  section {
    max-width: 48rem;
    display: grid;
    align-content: start;
    gap: var(--space-3);
    padding: var(--space-3);
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
  }

  p {
    margin: 0;
    overflow-wrap: anywhere;
  }

  dl {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    gap: var(--space-1) var(--space-3);
    margin: 0;
  }

  dt {
    color: var(--muted);
  }

  dd {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .libraries {
    display: grid;
    gap: var(--space-1-5);
  }

  .check {
    display: flex;
    align-items: baseline;
    gap: var(--space-1-5);
  }

  .installs {
    margin: 0;
    padding-left: var(--space-4);
    display: grid;
    gap: var(--space-0-5);
  }

  .hint {
    color: var(--muted);
  }

  pre {
    margin: 0;
    max-height: 16rem;
    overflow: auto;
    padding: var(--space-2);
    background: var(--input);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-size: var(--text-xs);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  .error {
    color: var(--danger);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
