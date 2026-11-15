<script lang="ts">
  // The Preferences view's Python section: which interpreter the Python
  // features use, and the two ways to choose one (the app's own
  // environment, set up in its own tab, or the user's interpreter).
  import {
    pickInterpreter,
    python,
    refreshPython,
    turnOffPython,
    useInterpreter,
    versionText,
  } from "./python.svelte";
  import { openView } from "./views";

  let choosing = $state(false);
  let path = $state("");
  let field = $state<HTMLInputElement>();

  void refreshPython();

  const status = $derived(python.view?.status ?? null);

  $effect(() => {
    if (choosing) field?.focus();
  });

  function startChoosing() {
    path = status && status.kind !== "notSet" ? status.interpreter : "";
    choosing = true;
  }

  async function browse() {
    const picked = await pickInterpreter();
    if (picked) path = picked;
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (path.trim() === "") return;
    await useInterpreter(path.trim());
    if (!python.error) choosing = false;
  }
</script>

<section class="python" data-python-section>
  <h3>Python</h3>
  {#if status}
    <p class="status" data-python-status={status.kind}>
      {status.message}
    </p>
    {#if status.kind !== "notSet"}
      <p class="interpreter">
        {status.setup === "managed"
          ? "The app's environment"
          : "Your interpreter"}:
        <code data-python-interpreter>{status.interpreter}</code>
        {#if status.kind === "ready"}(Python {versionText(status.version)}){/if}
      </p>
    {/if}
  {:else if python.busy}
    <p class="status">Checking…</p>
  {/if}

  {#if choosing}
    <form class="own" onsubmit={submit} data-python-form>
      <label for="python-interpreter">Interpreter</label>
      <input
        id="python-interpreter"
        name="python-interpreter"
        bind:this={field}
        bind:value={path}
        placeholder="The path of python.exe (or python)"
        autocomplete="off"
        spellcheck="false"
      />
      <span></span>
      <span class="buttons">
        <button type="button" onclick={() => void browse()}>Browse…</button>
        <button type="button" onclick={() => (choosing = false)}>Cancel</button>
        <button
          type="submit"
          data-python="use"
          disabled={python.busy || path.trim() === ""}>Use it</button
        >
      </span>
      <span></span>
      <p class="hint">Nothing is installed into it.</p>
    </form>
  {:else}
    <div class="buttons">
      <button
        type="button"
        data-python="set-up"
        onclick={() => openView("python-setup")}
        >Create the app's environment…</button
      >
      <button type="button" data-python="use-own" onclick={startChoosing}
        >Use my interpreter…</button
      >
      {#if status && status.kind !== "notSet"}
        <button
          type="button"
          data-python="check"
          disabled={python.busy}
          onclick={() => void refreshPython()}>Check again</button
        >
        <button
          type="button"
          data-python="turn-off"
          disabled={python.busy}
          onclick={() => void turnOffPython()}>Turn off</button
        >
      {/if}
    </div>
  {/if}

  {#if python.error}
    <p class="error" role="alert" data-python-error>{python.error}</p>
  {/if}
</section>

<style>
  .python {
    display: grid;
    gap: var(--space-2);
    padding-top: var(--space-2);
    border-top: 1px solid var(--border);
    font-size: var(--text-sm);
  }

  h3 {
    margin: 0;
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
  }

  p {
    margin: 0;
    overflow-wrap: anywhere;
  }

  .status[data-python-status="problem"] {
    color: var(--danger);
  }

  .interpreter,
  .hint {
    color: var(--muted);
    font-size: var(--text-xs);
  }

  .own {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: var(--space-1-5) var(--space-3);
  }

  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1-5);
  }

  .error {
    color: var(--danger);
  }
</style>
