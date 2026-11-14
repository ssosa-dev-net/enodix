<script lang="ts">
  import CopyButton from "./CopyButton.svelte";
  import { move, remove, renameTo, saveHost } from "./inventory.svelte";
  import { HOST_TEXT_FIELDS, type CustomField, type Host } from "./workspace";

  interface Props {
    host: Host;
    onconnect: (host: Host) => void;
  }

  let { host, onconnect }: Props = $props();

  const item = $derived({ kind: "host" as const, id: host.id });

  let newTag = $state("");
  let newKey = $state("");
  let newValue = $state("");

  /** Saves the host with some fields changed. */
  function save(changes: Partial<Host> | ((current: Host) => Partial<Host>)) {
    return saveHost(host.id, changes);
  }

  type Field = HTMLInputElement | HTMLTextAreaElement;

  /**
   * Saves a field edited in `input`; afterwards the input shows what the
   * file holds, which is the old value if the edit was refused.
   */
  async function commit(
    event: Event,
    changes: (
      value: string,
    ) => Partial<Host> | ((current: Host) => Partial<Host>),
    stored: () => string,
  ) {
    const input = event.currentTarget as Field;
    await save(changes(input.value));
    input.value = stored();
  }

  function addTag(event: SubmitEvent) {
    event.preventDefault();
    const tag = newTag;
    if (tag.trim() === "") return;
    save((current) => ({ tags: [...current.tags, tag] }));
    newTag = "";
  }

  /** Changes the custom field named `key`, wherever it is by then. */
  function changeCustom(key: string, change: Partial<CustomField>) {
    return (current: Host) => ({
      custom: current.custom.map((field) =>
        field.key === key ? { ...field, ...change } : field,
      ),
    });
  }

  function addCustom(event: SubmitEvent) {
    event.preventDefault();
    const field = { key: newKey, value: newValue };
    save((current) => ({ custom: [...current.custom, field] })).then(() => {
      if (host.custom.some((saved) => saved.key === field.key.trim())) {
        newKey = "";
        newValue = "";
      }
    });
  }
</script>

<div class="details" aria-label="Host details">
  <header>
    <h2>{host.name}</h2>
    <span class="file" title="The host's file in the workspace">{host.id}</span>
    <div class="actions">
      <button onclick={() => onconnect(host)}>Connect</button>
      <button onclick={() => move(item)}>Move…</button>
      <button class="danger" onclick={() => remove(item)}>Delete…</button>
    </div>
  </header>

  <section>
    <label class="field">
      <span>Name</span>
      <span class="row">
        <input
          name="name"
          value={host.name}
          spellcheck="false"
          onchange={async (event) => {
            const input = event.currentTarget;
            await renameTo(item, input.value);
            input.value = host.name;
          }}
        />
        <CopyButton value={host.name} what="name" />
      </span>
    </label>
    <label class="field">
      <span>Management address</span>
      <span class="row">
        <input
          name="address"
          value={host.address}
          spellcheck="false"
          onchange={(event) =>
            commit(
              event,
              (address) => ({ address }),
              () => host.address,
            )}
        />
        <CopyButton value={host.address} what="management address" />
      </span>
    </label>
    <label class="field">
      <span>Port</span>
      <span class="row">
        <input
          name="port"
          type="number"
          min="1"
          max="65535"
          placeholder="default"
          value={host.port ?? ""}
          onchange={(event) =>
            commit(
              event,
              (port) => ({ port: port.trim() === "" ? null : Number(port) }),
              () => host.port?.toString() ?? "",
            )}
        />
        <CopyButton value={host.port?.toString() ?? ""} what="port" />
      </span>
    </label>
    {#each HOST_TEXT_FIELDS as [key, label] (key)}
      <label class="field">
        <span>{label}</span>
        <span class="row">
          <input
            name={key}
            value={host[key] ?? ""}
            spellcheck="false"
            onchange={(event) =>
              commit(
                event,
                (value) => ({ [key]: value }),
                () => host[key] ?? "",
              )}
          />
          <CopyButton value={host[key] ?? ""} what={label.toLowerCase()} />
        </span>
      </label>
    {/each}
    <label class="field">
      <span>Description</span>
      <span class="row">
        <textarea
          name="description"
          rows="3"
          value={host.description ?? ""}
          onchange={(event) =>
            commit(
              event,
              (description) => ({ description }),
              () => host.description ?? "",
            )}></textarea>
        <CopyButton value={host.description ?? ""} what="description" />
      </span>
    </label>
  </section>

  <section>
    <h3>
      Tags
      <CopyButton value={host.tags.join(", ")} what="tags" />
    </h3>
    {#if host.tags.length > 0}
      <ul class="tags">
        {#each host.tags as tag (tag)}
          <li class="tag">
            {tag}
            <button
              class="remove"
              aria-label="Remove tag {tag}"
              onclick={() =>
                save((current) => ({
                  tags: current.tags.filter((t) => t !== tag),
                }))}>×</button
            >
          </li>
        {/each}
      </ul>
    {/if}
    <form class="row" onsubmit={addTag}>
      <input
        name="new-tag"
        placeholder="Add a tag"
        spellcheck="false"
        bind:value={newTag}
      />
      <button type="submit" class="small" disabled={newTag.trim() === ""}
        >Add</button
      >
    </form>
  </section>

  <section>
    <h3>Custom fields</h3>
    {#each host.custom as field (field.key)}
      <div class="row custom">
        <input
          class="key"
          aria-label="Field name"
          value={field.key}
          spellcheck="false"
          onchange={(event) =>
            commit(
              event,
              (key) => changeCustom(field.key, { key }),
              () => field.key,
            )}
        />
        <input
          class="value"
          aria-label="{field.key} value"
          data-custom={field.key}
          value={field.value}
          spellcheck="false"
          onchange={(event) =>
            commit(
              event,
              (value) => changeCustom(field.key, { value }),
              () => field.value,
            )}
        />
        <CopyButton value={field.value} what={field.key} />
        <button
          class="remove"
          aria-label="Remove field {field.key}"
          onclick={() =>
            save((current) => ({
              custom: current.custom.filter((f) => f.key !== field.key),
            }))}>×</button
        >
      </div>
    {/each}
    <form class="row custom" onsubmit={addCustom}>
      <input
        class="key"
        name="new-key"
        placeholder="Name"
        spellcheck="false"
        bind:value={newKey}
      />
      <input
        class="value"
        name="new-value"
        placeholder="Value"
        spellcheck="false"
        bind:value={newValue}
      />
      <button type="submit" class="small" disabled={newKey.trim() === ""}
        >Add</button
      >
    </form>
  </section>
</div>

<style>
  .details {
    display: grid;
    gap: 1rem;
    padding: 0.8rem;
    font-size: 0.85rem;
  }

  header {
    display: grid;
    gap: 0.3rem;
  }

  h2 {
    margin: 0;
    font-size: 1.05rem;
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  h3 {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 0 0 0.4rem;
    font-size: 0.8rem;
    font-weight: 600;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .file {
    color: var(--muted);
    font-size: 0.75rem;
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    margin-top: 0.3rem;
  }

  .actions button,
  .small {
    padding: 0.2rem 0.6rem;
    font-size: 0.8rem;
  }

  section {
    display: grid;
    gap: 0.45rem;
  }

  .field {
    display: grid;
    gap: 0.15rem;
    font-size: 0.75rem;
    color: var(--muted);
  }

  .row {
    display: flex;
    align-items: center;
    gap: 0.25rem;
  }

  .row input,
  .row textarea {
    flex: 1;
    width: 100%;
  }

  textarea {
    resize: vertical;
  }

  .custom .key {
    flex: 0 1 40%;
  }

  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .tag {
    display: flex;
    align-items: center;
    padding: 0.05rem 0 0.05rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface);
  }

  .remove {
    padding: 0 0.45rem;
    border: 0;
    background: none;
    color: var(--muted);
  }

  .remove:hover:not(:disabled) {
    color: var(--danger);
  }

  .danger:hover:not(:disabled) {
    color: var(--danger);
  }
</style>
