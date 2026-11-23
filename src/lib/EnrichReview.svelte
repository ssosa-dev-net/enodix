<script lang="ts">
  // The review of what a "Fill in from devices" run's devices reported, in
  // its tab (#73; one per run): what is new, what would replace a typed
  // value (only on the hosts ticked), then the report. Nothing is written
  // before "Fill in". Cancel and Close close the tab.
  import { untrack } from "svelte";
  import { applyEnrich, revealHost } from "./inventory.svelte";
  import {
    previewEnrich,
    type EnrichField,
    type EnrichPreview,
    type EnrichPreviewHost,
    type EnrichReport,
  } from "./workspace";

  interface Props {
    /** The kept "Fill in from devices" run to review. */
    run: string;
    /** Closes the tab. */
    onclose: () => void;
  }

  let { run, onclose }: Props = $props();

  const FIELD_LABELS: Record<EnrichField, string> = {
    vendor: "Vendor",
    model: "Model",
    serial_number: "Serial number",
    os_version: "OS version (custom field os_version)",
  };

  const OUTCOME_LABELS: Record<EnrichPreviewHost["outcome"]["kind"], string> = {
    changes: "Fills in",
    unchanged: "Unchanged",
    skipped: "Skipped",
    failed: "Failed",
  };

  let preview = $state<EnrichPreview | null>(null);
  let report = $state<EnrichReport | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);
  /** The hosts whose typed values the user agreed to replace. */
  let replace = $state<Record<string, boolean>>({});

  const step = $derived(
    report !== null ? "report" : preview !== null ? "preview" : "loading",
  );

  function replaces(host: EnrichPreviewHost): boolean {
    return host.changes.some((change) => change.kind === "replace");
  }

  const replaceable = $derived(preview?.hosts.filter(replaces) ?? []);
  const allTicked = $derived(
    replaceable.length > 0 && replaceable.every((host) => replace[host.id]),
  );
  const writes = $derived(
    preview?.hosts.some((host) =>
      host.changes.some(
        (change) => change.kind === "new" || replace[host.id] === true,
      ),
    ) ?? false,
  );

  untrack(() => {
    previewEnrich(run)
      .then((found) => (preview = found))
      .catch((reason) => (error = String(reason)));
  });

  async function apply() {
    if (busy) return;
    busy = true;
    error = null;
    try {
      report = await applyEnrich(
        run,
        Object.keys(replace).filter((id) => replace[id]),
      );
    } catch (reason) {
      error = String(reason);
    } finally {
      busy = false;
    }
  }

  function tickAll(ticked: boolean) {
    for (const host of replaceable) replace[host.id] = ticked;
  }

  const REPORT_SECTIONS = [
    ["filled", "Filled in"],
    ["unchanged", "Unchanged"],
    ["skipped", "Skipped"],
    ["failed", "Failed"],
  ] as const;
</script>

<div class="review" data-view="enrich" data-run={run} data-step={step}>
  <section class="inner" aria-labelledby="enrich-title">
    <h2 id="enrich-title">Fill in from devices</h2>

    {#if step === "loading"}
      <p class="note">{error ? "" : "Comparing what the devices reported…"}</p>
    {:else if step === "preview" && preview}
      <p class="note" data-enrich-note>
        Nothing has been written yet. Applying fills every empty field below; a
        typed value is replaced only on the hosts you tick.
      </p>
      <div class="scroll">
        <table>
          <thead>
            <tr><th>Replace</th><th>Host</th><th></th><th>Details</th></tr>
          </thead>
          <tbody>
            {#each preview.hosts as host (host.id)}
              <tr
                data-enrich-host={host.id}
                data-outcome={host.outcome.kind}
                class={host.outcome.kind}
              >
                <td>
                  {#if replaces(host)}
                    <input
                      type="checkbox"
                      data-enrich-replace={host.id}
                      aria-label="Replace the typed values of {host.name}"
                      bind:checked={replace[host.id]}
                    />
                  {/if}
                </td>
                <td>
                  <button
                    type="button"
                    class="link"
                    title="Show in the tree"
                    onclick={() => revealHost(host.id)}>{host.name}</button
                  >
                </td>
                <td class="outcome">{OUTCOME_LABELS[host.outcome.kind]}</td>
                <td class="details">
                  {#if host.outcome.kind === "skipped" || host.outcome.kind === "failed"}
                    <span data-enrich-reason>{host.outcome.reason}</span>
                  {:else if host.outcome.kind === "unchanged"}
                    The device reports what the host already has.
                  {:else}
                    <ul>
                      {#each host.changes as change (change.field)}
                        <li
                          data-enrich-change={change.field}
                          data-change={change.kind}
                          class:kept={change.kind === "replace" &&
                            !replace[host.id]}
                        >
                          <span class="field">{FIELD_LABELS[change.field]}</span
                          >
                          {#if change.kind === "replace"}
                            <span class="old">{change.current}</span> →
                          {/if}
                          <span class="new">{change.value}</span>
                          {#if change.kind === "replace"}
                            <span class="hint"
                              >{replace[host.id]
                                ? "(replaced)"
                                : "(kept unless ticked)"}</span
                            >
                          {/if}
                        </li>
                      {/each}
                    </ul>
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      {#if replaceable.length > 1}
        <label class="tick-all">
          <input
            type="checkbox"
            data-enrich-replace-all
            checked={allTicked}
            onchange={(event) => tickAll(event.currentTarget.checked)}
          />
          Replace typed values on all {replaceable.length} hosts
        </label>
      {/if}
      <div class="actions">
        <button type="button" data-enrich="cancel" onclick={onclose}
          >Cancel</button
        >
        <button
          type="button"
          class="primary"
          data-enrich="apply"
          disabled={busy || !writes}
          onclick={apply}>Fill in</button
        >
      </div>
    {:else if step === "report" && report}
      {#if report.reloadError}
        <p class="error">
          The hosts were written, but the workspace could not be reloaded: {report.reloadError}
        </p>
      {/if}
      <div class="scroll report">
        {#each REPORT_SECTIONS as [key, title] (key)}
          {@const entries = report[key]}
          <section data-enrich-report={key} data-count={entries.length}>
            <h3>{title} ({entries.length})</h3>
            {#if entries.length > 0}
              <ul>
                {#each entries as entry (entry.id)}
                  <li data-enrich-reported={entry.id}>
                    <strong>{entry.name}</strong>
                    {#if entry.fields.length > 0}
                      : {entry.fields
                        .map((field) => FIELD_LABELS[field])
                        .join(", ")}
                    {/if}
                    {#if entry.kept.length > 0}
                      <span class="hint"
                        >(kept typed {entry.kept
                          .map((field) => FIELD_LABELS[field])
                          .join(", ")})</span
                      >
                    {/if}
                    {#if entry.reason}
                      <span class="details">— {entry.reason}</span>
                    {/if}
                  </li>
                {/each}
              </ul>
            {/if}
          </section>
        {/each}
      </div>
      <div class="actions">
        <button
          type="button"
          class="primary"
          data-enrich="close"
          onclick={onclose}>Close</button
        >
      </div>
    {/if}

    {#if error}
      <p class="error" role="alert" data-enrich-error>{error}</p>
      {#if step === "loading"}
        <div class="actions">
          <button type="button" data-enrich="close" onclick={onclose}
            >Close</button
          >
        </div>
      {/if}
    {/if}
  </section>
</div>

<style>
  .review {
    height: 100%;
    min-height: 0;
    overflow: hidden;
    background: var(--bg);
    font-size: var(--text-sm);
  }

  .inner {
    height: 100%;
    max-width: 70rem;
    box-sizing: border-box;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    padding: var(--space-3);
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
  }

  h3 {
    margin: var(--space-1) 0;
    font-size: var(--text-sm);
  }

  .scroll {
    overflow: auto;
    min-height: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
  }

  .report {
    padding: 0 var(--space-2);
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
    vertical-align: top;
  }

  th {
    position: sticky;
    top: 0;
    background: var(--surface);
    font-weight: var(--weight-semibold);
  }

  ul {
    margin: 0;
    padding-left: var(--space-4);
  }

  .outcome {
    white-space: nowrap;
  }

  tr.failed .outcome {
    color: var(--danger);
  }

  tr.skipped td,
  tr.unchanged td,
  .details,
  .hint,
  .old {
    color: var(--muted);
  }

  .field {
    font-weight: var(--weight-semibold);
  }

  .old {
    text-decoration: line-through;
  }

  li.kept .new {
    color: var(--muted);
  }

  .link {
    min-height: 0;
    padding: 0;
    border: 0;
    background: none;
    color: inherit;
    text-decoration: underline;
    cursor: pointer;
  }

  .tick-all {
    display: flex;
    gap: var(--space-2);
    align-items: center;
  }

  .note {
    margin: 0;
    color: var(--muted);
  }

  .actions {
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
