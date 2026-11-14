<script lang="ts">
  import { untrack } from "svelte";
  import { inventory, reveal, runCsvImport } from "./inventory.svelte";
  import {
    allFolders,
    pickCsvFile,
    previewCsv,
    readCsv,
    type ColumnTarget,
    type CsvFileView,
    type HostField,
    type ImportPreview,
    type ImportReport,
    type PreviewRow,
    type ReportEntry,
  } from "./workspace";

  interface Props {
    /** The folder the hosts go to at first. */
    folder: string;
    onclose: () => void;
  }

  let { folder, onclose }: Props = $props();

  const FIELDS: [HostField, string][] = [
    ["name", "Name"],
    ["address", "Management address"],
    ["vendor", "Vendor"],
    ["platform", "Platform"],
    ["model", "Model"],
    ["serial_number", "Serial number"],
    ["site", "Site"],
    ["building", "Building"],
    ["room", "Room"],
    ["rack", "Rack"],
    ["rack_unit", "Rack unit"],
    ["role", "Role"],
    ["description", "Description"],
  ];
  const fieldLabel = (field: HostField) =>
    FIELDS.find(([key]) => key === field)?.[1] ?? field;

  type Step = "file" | "map" | "preview" | "report";
  let step = $state<Step>("file");
  let path = $state("");
  let file = $state<CsvFileView | null>(null);
  /** Per column: `ignore`, `tags`, `folderPath`, `custom` or `field:<key>`. */
  let choices = $state<string[]>([]);
  /** Per column: the custom field's key, when `custom`. */
  let keys = $state<string[]>([]);
  let target = $state(untrack(() => folder));
  let preview = $state<ImportPreview | null>(null);
  /** Existing hosts the user confirmed to replace, by id. */
  let replace = $state<Record<string, boolean>>({});
  let report = $state<ImportReport | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let fileInput = $state<HTMLInputElement>();

  $effect(() => {
    fileInput?.focus();
  });

  const folders = $derived(
    inventory.workspace
      ? allFolders(inventory.workspace.tree).map((f) => f.path)
      : [],
  );

  function choiceOf(target: ColumnTarget): string {
    return target.kind === "field" ? `field:${target.field}` : target.kind;
  }

  const mapping = $derived(
    choices.map((choice, column): ColumnTarget => {
      if (choice.startsWith("field:")) {
        return { kind: "field", field: choice.slice(6) as HostField };
      }
      if (choice === "custom") return { kind: "custom", key: keys[column] };
      return { kind: choice as "tags" | "folderPath" | "ignore" };
    }),
  );

  const existing = $derived(
    preview?.rows.flatMap((row) =>
      row.outcome.kind === "exists" ? [row.outcome.id] : [],
    ) ?? [],
  );
  const confirmed = $derived(existing.filter((id) => replace[id]));
  const counts = $derived.by(() => {
    const count = { create: 0, exists: 0, skip: 0, fail: 0 };
    for (const row of preview?.rows ?? []) count[row.outcome.kind] += 1;
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

  async function browse() {
    const picked = await pickCsvFile();
    if (picked !== null) path = picked;
  }

  function read() {
    return attempt(async () => {
      const view = await readCsv(path.trim());
      file = view;
      choices = view.suggested.map(choiceOf);
      keys = view.suggested.map((target, column) =>
        target.kind === "custom" ? target.key : view.headers[column],
      );
      step = "map";
    });
  }

  function showPreview() {
    return attempt(async () => {
      preview = await previewCsv(mapping, target);
      replace = {};
      step = "preview";
    });
  }

  function run() {
    return attempt(async () => {
      report = await runCsvImport(mapping, target, confirmed);
      if (target) reveal(target, true);
      step = "report";
    });
  }

  function summary(row: PreviewRow): string {
    const host = row.host;
    if (!host) return "";
    return [
      ...host.fields.map((f) => `${fieldLabel(f.field)}: ${f.value}`),
      ...(host.tags.length ? [`Tags: ${host.tags.join(", ")}`] : []),
      ...host.custom.map((c) => `${c.key}: ${c.value}`),
    ].join(" · ");
  }

  function describe(entry: ReportEntry): string {
    const where = entry.line === null ? "File" : `Row ${entry.line}`;
    const what = entry.host ?? entry.name;
    return `${where}${what ? ` · ${what}` : ""}${entry.reason ? ` — ${entry.reason}` : ""}`;
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
    aria-labelledby="csv-import-title"
    data-dialog="csv-import"
    data-step={step}
  >
    <h2 id="csv-import-title">Import hosts from CSV</h2>

    {#if step === "file"}
      <form
        onsubmit={(event) => {
          event.preventDefault();
          if (path.trim()) void read();
        }}
      >
        <label for="csv-file">CSV file</label>
        <div class="row">
          <input
            id="csv-file"
            name="csv-file"
            bind:this={fileInput}
            bind:value={path}
            placeholder="Path to a .csv file"
            autocomplete="off"
            spellcheck="false"
          />
          <button type="button" onclick={browse}>Browse…</button>
          <button type="submit" data-csv="read" disabled={busy || !path.trim()}
            >Next</button
          >
        </div>
        <p class="note">
          The first row names the columns. Commas or semicolons, quoted values
          and UTF-8 or Windows-1252 text are all fine.
        </p>
      </form>
    {:else if step === "map" && file}
      <p class="note">
        {file.rows === 1 ? "1 row" : `${file.rows} rows`}, separated by
        {file.separator === "comma" ? "commas" : "semicolons"}. Choose what each
        column fills.
      </p>
      <div class="scroll">
        <table class="mapping">
          <thead>
            <tr><th>Column</th><th>Values</th><th>Fills</th></tr>
          </thead>
          <tbody>
            {#each file.headers as header, column (column)}
              <tr data-csv-column={column}>
                <td class="header">{header || `Column ${column + 1}`}</td>
                <td class="sample">
                  {file.sample
                    .map((row) => row[column] ?? "")
                    .filter(Boolean)
                    .join(" · ")}
                </td>
                <td>
                  <div class="row">
                    <select
                      name="csv-column-{column}"
                      aria-label="What {header} fills"
                      bind:value={choices[column]}
                    >
                      <option value="ignore">Ignore</option>
                      <optgroup label="Field">
                        {#each FIELDS as [key, label] (key)}
                          <option value="field:{key}">{label}</option>
                        {/each}
                      </optgroup>
                      <option value="custom">Custom field…</option>
                      <option value="tags">Tags</option>
                      <option value="folderPath">Folder path</option>
                    </select>
                    {#if choices[column] === "custom"}
                      <input
                        name="csv-custom-{column}"
                        aria-label="Custom field name"
                        bind:value={keys[column]}
                        autocomplete="off"
                        spellcheck="false"
                      />
                    {/if}
                  </div>
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      <label class="target">
        Import into
        <select name="csv-target" bind:value={target}>
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
        Tags may be separated by commas, semicolons or |. A folder path such as
        <code>Europe/Madrid</code> is created under the folder imported into. Password
        columns are never imported.
      </p>
      <div class="actions">
        <button type="button" onclick={() => (step = "file")}>Back</button>
        <button
          type="button"
          class="primary"
          data-csv="preview"
          disabled={busy}
          onclick={showPreview}>Preview</button
        >
      </div>
    {:else if step === "preview" && preview}
      <p class="note" data-preview-summary>
        {counts.create} new, {counts.exists} already in the workspace,
        {counts.fail} with errors, {counts.skip} empty. Nothing is written until you
        import.
      </p>
      {#if preview.newFolders.length > 0}
        <p class="note" data-preview-folders>
          New folders: {preview.newFolders.join(", ")}
        </p>
      {/if}
      {#each preview.skippedColumns as column (column.name)}
        <p class="note warning" data-preview-skipped-column={column.name}>
          Column “{column.name}”: {column.reason}.
        </p>
      {/each}
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
              <th>Row</th><th></th><th>Name</th><th>Address</th><th>Folder</th
              ><th>Details</th>
            </tr>
          </thead>
          <tbody>
            {#each preview.rows as row (row.line)}
              <tr
                data-preview-row={row.line}
                data-outcome={row.outcome.kind}
                class={row.outcome.kind}
              >
                <td>{row.line}</td>
                <td class="outcome">
                  {#if row.outcome.kind === "create"}New
                  {:else if row.outcome.kind === "exists"}
                    {@const id = row.outcome.id}
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
                  {:else if row.outcome.kind === "skip"}Skip
                  {:else}Error{/if}
                </td>
                <td>{row.name}</td>
                <td>{row.host?.address ?? ""}</td>
                <td>{row.host?.folder ?? ""}</td>
                <td class="details">
                  {#if row.outcome.kind === "fail" || row.outcome.kind === "skip"}
                    {row.outcome.reason}
                  {:else if row.outcome.kind === "exists"}
                    exists as {row.outcome.id}{summary(row)
                      ? ` · ${summary(row)}`
                      : ""}
                  {:else}
                    {summary(row)}
                  {/if}
                </td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
      <div class="actions">
        <button type="button" onclick={() => (step = "map")}>Back</button>
        <button
          type="button"
          class="primary"
          data-csv="import"
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
                  <li data-report-line={entry.line ?? "file"}>
                    {describe(entry)}
                  </li>
                {/each}
              </ul>
            {/if}
          </section>
        {/each}
        {#if report.createdFolders.length > 0}
          <p class="note" data-report-folders>
            New folders: {report.createdFolders.join(", ")}
          </p>
        {/if}
        {#if report.reloadError}
          <p class="error" data-report-reload-error>
            The hosts were written, but the workspace could not be reloaded:
            {report.reloadError}
          </p>
        {/if}
      </div>
      <div class="actions">
        <button type="button" class="primary" data-csv="close" onclick={onclose}
          >Close</button
        >
      </div>
    {/if}

    {#if error}
      <p class="error" role="alert" data-csv-error>{error}</p>
    {/if}
    {#if step !== "report"}
      <footer>
        <button
          type="button"
          data-csv="cancel"
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

  .header {
    font-weight: 600;
    white-space: nowrap;
  }

  .sample,
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

  tr.skip td {
    color: var(--muted);
  }

  .outcome {
    white-space: nowrap;
  }

  .target {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }

  .note {
    margin: 0;
    color: var(--muted);
  }

  .note.warning {
    color: var(--warning);
  }

  section ul {
    margin: 0;
    padding: 0 0.5rem 0.3rem 1.4rem;
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
