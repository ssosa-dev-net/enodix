<script lang="ts">
  // "Save as job…": saves a step and its target under a name in the
  // workspace's jobs file, from a job tab's pre-flight or a past run's tab.

  import { unsaveable, type JobStep, type JobTarget } from "./jobs";
  import { saveJobAs } from "./inventory.svelte";
  import { secretAnswers } from "./workspace";

  interface Props {
    step: JobStep;
    target: JobTarget;
    /** The name offered. */
    suggested: string;
  }

  let { step, target, suggested }: Props = $props();

  let open = $state(false);
  let name = $state("");
  let error = $state<string | null>(null);
  let saved = $state<string | null>(null);
  let busy = $state(false);
  let input = $state<HTMLInputElement>();

  const refused = $derived(unsaveable(step, target));
  /**
   * The variables whose answers are not saved, their names looking like
   * secrets (#126, the core's rule): asked at each run.
   */
  let asked = $state<string[]>([]);
  /** Whether answers to snippet variables go into the shared file too. */
  const keepsAnswers = $derived(
    step.kind === "snippet" &&
      Object.keys(step.answers).some((variable) => !asked.includes(variable)),
  );

  async function begin() {
    name = suggested;
    error = null;
    saved = null;
    try {
      asked = await secretAnswers(step);
    } catch (reason) {
      error = String(reason);
      return;
    }
    open = true;
    queueMicrotask(() => input?.select());
  }

  async function save() {
    if (busy) return;
    busy = true;
    error = null;
    try {
      await saveJobAs(name, step, target);
      saved = name.trim();
      open = false;
    } catch (reason) {
      error = String(reason);
    } finally {
      busy = false;
    }
  }
</script>

{#if open}
  <form
    class="save"
    data-save-job-form
    onsubmit={(event) => {
      event.preventDefault();
      void save();
    }}
  >
    <label>
      <span>Save as job</span>
      <input
        name="save-job-name"
        bind:this={input}
        bind:value={name}
        autocomplete="off"
        spellcheck="false"
        onkeydown={(event) => {
          if (event.key === "Escape") {
            event.stopPropagation();
            open = false;
          }
        }}
      />
    </label>
    <button type="button" data-save-job="cancel" onclick={() => (open = false)}
      >Cancel</button
    >
    <button
      type="submit"
      class="primary"
      data-save-job="save"
      disabled={busy || !name.trim()}>Save</button
    >
    {#if keepsAnswers}
      <span class="note" data-save-job-answers
        >The variable values given are saved with it, in the workspace file the
        team shares.</span
      >
    {/if}
    {#if asked.length > 0}
      <span class="note" data-save-job-asked={asked.join(",")}
        >Not saved, asked at each run (the names look like secrets): {asked
          .map((variable) => `{{${variable}}}`)
          .join(", ")}.</span
      >
    {/if}
  </form>
{:else}
  <button
    type="button"
    data-save-job="open"
    disabled={refused !== null}
    title={refused ??
      "Save this job under a name in the workspace, to run it again in one click"}
    onclick={() => void begin()}>Save as job…</button
  >
{/if}
{#if saved}
  <span class="saved" role="status" data-save-job-saved={saved}
    >Saved as {saved}</span
  >
{/if}
{#if error}
  <span class="error" role="alert" data-save-job-error>{error}</span>
{/if}

<style>
  .save {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-1-5);
  }

  label {
    display: flex;
    align-items: center;
    gap: var(--space-1-5);
  }

  input {
    padding: var(--space-1) var(--space-1-5);
    min-width: 12rem;
  }

  .note {
    color: var(--muted);
    flex-basis: 100%;
  }

  .saved {
    color: var(--muted);
    align-self: center;
  }

  .error {
    color: var(--danger);
    align-self: center;
    overflow-wrap: anywhere;
  }
</style>
