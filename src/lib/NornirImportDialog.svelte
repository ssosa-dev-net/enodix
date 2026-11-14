<script lang="ts">
  import { untrack } from "svelte";
  import { inventory, reveal, runNornirImport } from "./inventory.svelte";
  import {
    allFolders,
    pickNornirFile,
    previewNornir,
    type ImportReport,
    type NornirPreview,
    type NornirPreviewHost,
    type ReportEntry,
  } from "./workspace";

  interface Props {
    /** The folder the hosts go to at first. */
    folder: string;
    onclose: () => void;
  }

  let { folder, onclose }: Props = $props();

  type Step = "files" | "preview" | "report";
  type InventoryFile = "hosts" | "groups" | "defaults";
  const FILES: [InventoryFile, string, string][] = [
    ["hosts", "Hosts file", "hosts.yaml (required)"],
    ["groups", "Groups file", "groups.yaml (optional)"],
    ["defaults", "Defaults file", "defaults.yaml (optional)"],
  ];

  /** The built-in fields a Nornir host fills, as the interface names them. */
  const FIELD_LABELS: Partial<Record<string, string>> = {
    platform: "Platform",
  };

  let step = $state<Step>("files");
  let paths = $state<Record<InventoryFile, string>>({
    hosts: "",
    groups: "",
    defaults: "",
  });
  let target = $state(untrack(() => folder));
  let preview = $state<NornirPreview | null>(null);
  /** Existing hosts the user confirmed to replace, by id. */
  let replace = $state<Record<string, boolean>>({});
  let report = $state<ImportReport | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let hostsInput = $state<HTMLInputElement>();

  $effect(() => {
    hostsInput?.focus();
  });

  const folders = $derived(
    inventory.workspace
      ? allFolders(inventory.workspace.tree).map((f) => f.path)
      : [],
  );

  const existing = $derived(
    preview?.hosts.flatMap((host) =>
      host.outcome.kind === "exists" ? [host.outcome.id] : [],
    ) ?? [],
  );
  const confirmed = $derived(existing.filter((id) => replace[id]));
  const counts = $derived.by(() => {
    const count = { create: 0, exists: 0, skip: 0, fail: 0 };
    for (const host of preview?.hosts ?? []) count[host.outcome.kind] += 1;
    return count;
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

  async function browse(file: InventoryFile, label: string) {
    const picked = await pickNornirFile(label);
    if (picked !== null) paths[file] = picked;
  }

  const optional = (path: string) => (path.trim() ? path.trim() : null);

  function showPreview() {
    return attempt(async () => {
      preview = await previewNornir(
        paths.hosts.trim(),
        optional(paths.groups),
        optional(paths.defaults),
        target,
      );
      replace = {};
      step = "preview";
    });
  }

  function run() {
    return attempt(async () => {
      report = await runNornirImport(confirmed);
      if (target) reveal(target, true);
      step = "report";
    });
  }

  function summary(entry: NornirPreviewHost): string {
    const host = entry.host;
    if (!host) return "";
    return [
      ...(host.port !== null ? [`Port: ${host.port}`] : []),
      ...host.fields.map(
        (f) => `${FIELD_LABELS[f.field] ?? f.field}: ${f.value}`,
      ),
      ...(host.tags.length ? [`Tags: ${host.tags.join(", ")}`] : []),
      ...host.custom.map((c) => `${c.key}: ${c.value}`),
    ].join(" · ");
  }

  function describe(entry: ReportEntry): string {
    const where = entry.line === null ? "" : `Line ${entry.line} · `;
    const what = entry.host ?? entry.name;
    return `${where}${what}${entry.reason ? ` — ${entry.reason}` : ""}`;
  }

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
    aria-labelledby="nornir-import-title"
    data-dialog="nornir-import"
    data-step={step}
  >
    <h2 id="nornir-import-title">Import hosts from a Nornir inventory</h2>

    {#if step === "files"}
      <form
        onsubmit={(event) => {
          event.preventDefault();
          if (paths.hosts.trim()) void showPreview();
        }}
      >
        {#each FILES as [file, label, placeholder] (file)}
          <label for="nornir-{file}">{label}</label>
          <div class="row">
            {#if file === "hosts"}
              <input
                id="nornir-{file}"
                name="nornir-{file}"
                bind:this={hostsInput}
                bind:value={paths[file]}
                {placeholder}
                autocomplete="off"
                spellcheck="false"
              />
            {:else}
              <input
                id="nornir-{file}"
                name="nornir-{file}"
                bind:value={paths[file]}
                {placeholder}
                autocomplete="off"
                spellcheck="false"
              />
            {/if}
            <button type="button" onclick={() => browse(file, label)}
              >Browse…</button
            >
          </div>
        {/each}
        <label class="target">
          Import into
          <select name="nornir-target" bind:value={target}>
            {#each folders as path (path)}
              <option value={path}
                >{path === ""
                  ? `${inventory.workspace?.tree.name} (top)`
                  : path}</option
              >
            {/each}
          </select>
        </label>
        <p class="note">
          Hostname, port and platform fill the host's own fields; groups become
          tags and lend their values, then the defaults; entries under
          <code>data</code> become custom fields. Passwords and usernames are never
          imported.
        </p>
        <div class="actions">
          <button
            type="submit"
            class="primary"
            data-nornir="preview"
            disabled={busy || !paths.hosts.trim()}>Preview</button
          >
        </div>
      </form>
    {:else if step === "preview" && preview}
      <p class="note" data-preview-summary>
        {counts.create} new, {counts.exists} already in the workspace,
        {counts.fail} with errors. Nothing is written until you import.
      </p>
      {#if preview.skipped.length > 0}
        <details
          class="note warning"
          data-preview-skipped={preview.skipped.length}
        >
          <summary
            >{preview.skipped.length} values in the files are not imported</summary
          >
          <ul>
            {#each preview.skipped as entry, index (index)}
              <li data-preview-skipped-entry={entry.name}>{describe(entry)}</li>
            {/each}
          </ul>
        </details>
      {/if}
      {#if existing.length > 0}
        <label class="note">
          <input
            type="checkbox"
            data-replace-all
            checked={confirmed.length === existing.length}
            onchange={(event) => {
              const on = event.currentTarget.checked;
              replace = Object.fromEntries(existing.map((id) => [id, on]));
            }}
          />
          Replace the values of all {existing.length} existing hosts (unticked ones
          are skipped)
        </label>
      {/if}
      <div class="scroll">
        <table class="preview">
          <thead>
            <tr>
              <th>Line</th><th></th><th>Name</th><th>Address</th><th>Details</th
              >
            </tr>
          </thead>
          <tbody>
            {#each preview.hosts as entry, index (index)}
              <tr
                data-preview-host={entry.name}
                data-outcome={entry.outcome.kind}
                class={entry.outcome.kind}
              >
                <td>{entry.line ?? ""}</td>
                <td class="outcome">
                  {#if entry.outcome.kind === "create"}New
                  {:else if entry.outcome.kind === "exists"}
                    {@const id = entry.outcome.id}
                    <label title="Replace {id} with these values">
                      <input
                        type="checkbox"
                        data-replace={id}
                        bind:checked={
                          () => replace[id] ?? false, (on) => (replace[id] = on)
                        }
                      />
                      Replace
                    </label>
                  {:else}Error{/if}
                </td>
                <td>{entry.name}</td>
                <td>{entry.host?.address ?? ""}</td>
                <td class="details">
                  {#if entry.outcome.kind === "fail" || entry.outcome.kind === "skip"}
                    {entry.outcome.reason}
                  {:else if entry.outcome.kind === "exists"}
                    exists as {entry.outcome.id}{summary(entry)
                      ? ` · ${summary(entry)}`
                      : ""}
                  {:else}
                    {summary(entry)}
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      <div class="actions">
        <button type="button" onclick={() => (step = "files")}>Back</button>
        <button
          type="button"
          class="primary"
          data-nornir="import"
          disabled={busy}
          onclick={run}
          >Import {counts.create + confirmed.length}
          {counts.create + confirmed.length === 1 ? "host" : "hosts"}</button
        >
      </div>
    {:else if step === "report" && report}
      <div class="scroll" data-import-report>
        {#each [["created", "Created", report.created], ["updated", "Replaced", report.updated], ["skipped", "Skipped", report.skipped], ["failed", "Failed", report.failed]] as const as [key, title, entries] (key)}
          <section data-report={key} data-count={entries.length}>
            <h3>{title}: {entries.length}</h3>
            {#if entries.length > 0}
              <ul>
                {#each entries as entry, index (index)}
                  <li data-report-entry={entry.name}>{describe(entry)}</li>
                {/each}
              </ul>
            {/if}
          </section>
        {/each}
        {#if report.reloadError}
          <p class="error" data-report-reload-error>
            The hosts were written, but the workspace could not be reloaded:
            {report.reloadError}
          </p>
        {/if}
      </div>
      <div class="actions">
        <button
          type="button"
          class="primary"
          data-nornir="close"
          onclick={onclose}>Close</button
        >
      </div>
    {/if}

    {#if error}
      <p class="error" role="alert" data-nornir-error>{error}</p>
    {/if}
    {#if step !== "report"}
      <footer>
        <button
          type="button"
          data-nornir="cancel"
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

  h3 {
    margin: 0.4rem 0 0.2rem;
    font-size: 0.9rem;
  }

  form {
    display: grid;
    gap: 0.25rem;
  }

  .row {
    display: flex;
    gap: 0.4rem;
    align-items: center;
  }

  .row input {
    flex: 1;
    min-width: 0;
  }

  input,
  select {
    padding: 0.3rem 0.5rem;
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

  tr.fail td {
    color: var(--danger);
  }

  tr.exists .outcome {
    color: var(--warning);
  }

  .outcome {
    white-space: nowrap;
  }

  .target {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    margin-top: 0.3rem;
  }

  .note {
    margin: 0;
    color: var(--muted);
  }

  .note.warning {
    color: var(--warning);
  }

  details ul,
  section ul {
    margin: 0;
    padding: 0 0.5rem 0.3rem 1.4rem;
  }

  details {
    max-height: 10rem;
    overflow: auto;
  }

  section {
    padding: 0 0.5rem;
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
