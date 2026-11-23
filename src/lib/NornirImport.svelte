<script lang="ts">
  // The Nornir import, in its tab (#71; one at a time, as the shell keeps
  // one previewed inventory): files, preview, report. The tree stays in
  // view beside it and updates when the import writes.
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
    /** The folder the hosts go to: the last one the import was opened on
     * (it becomes the target again while no preview is shown). */
    folder: string;
    /** Closes the tab. */
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

  // Opened again on another folder: that folder becomes the target, unless
  // a preview (made for the target it shows) is on screen.
  $effect(() => {
    const opened = folder;
    untrack(() => {
      if (step === "files") target = opened;
    });
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
</script>

<div class="import" data-view="nornir-import" data-step={step}>
  <div class="inner">
    <h2>Import hosts from a Nornir inventory</h2>

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
          disabled={busy || counts.create + confirmed.length === 0}
          title={counts.create + confirmed.length === 0
            ? "Nothing to import: no new host, and no existing one ticked"
            : undefined}
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
      {#if report.created.length > 0}
        <p class="note" data-report-next>
          Double-click a host to connect. So that they log in without asking
          each time, name a credential profile in the Credential profile setting
          of their folder (its details), or in Workspace settings at the top of
          the Hosts view.
        </p>
      {/if}
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
  .import {
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
    font-size: var(--text-xl);
    font-weight: var(--weight-semibold);
  }

  h3 {
    margin: var(--space-1-5) 0 var(--space-0-5);
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    color: var(--text);
  }

  form {
    display: grid;
    gap: var(--space-1);
  }

  .row {
    display: flex;
    gap: var(--space-1-5);
    align-items: center;
  }

  .row input {
    flex: 1;
    min-width: 0;
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
    gap: var(--space-2);
    align-items: center;
    margin-top: var(--space-1);
  }

  .note {
    margin: 0;
    color: var(--muted);
    line-height: 1.5;
  }

  .note.warning {
    color: var(--warning);
    line-height: 1.5;
  }

  details ul,
  section ul {
    margin: 0;
    padding: 0 var(--space-2) var(--space-1) var(--space-5);
  }

  details {
    max-height: 10rem;
    overflow: auto;
  }

  section {
    padding: 0 var(--space-2);
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
