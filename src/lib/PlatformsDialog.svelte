<script lang="ts">
  import { deletePlatform, inventory, savePlatform } from "./inventory.svelte";
  import type { ListedPlatform, PlatformDefinition } from "./workspace";

  interface Props {
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  const platforms = $derived(inventory.workspace?.platforms ?? []);

  /** The text fields of a definition, in form order. */
  type Field =
    | "disablePaging"
    | "privilegeCommand"
    | "privilegePrompt"
    | "showConfig"
    | "ignoredLines";
  const FIELDS: readonly {
    key: Field;
    id: string;
    label: string;
    pattern: boolean;
  }[] = [
    {
      key: "disablePaging",
      id: "disable-paging",
      label: "Disable paging",
      pattern: false,
    },
    {
      key: "privilegeCommand",
      id: "privilege-command",
      label: "Privilege command",
      pattern: false,
    },
    {
      key: "privilegePrompt",
      id: "privilege-prompt",
      label: "Privilege password prompt",
      pattern: true,
    },
    {
      key: "showConfig",
      id: "show-config",
      label: "Show config",
      pattern: false,
    },
    {
      key: "ignoredLines",
      id: "ignored-lines",
      label: "Ignored lines",
      pattern: true,
    },
  ];

  /** A definition as the form edits it: every value as text. */
  interface Form {
    name: string;
    otherNames: string;
    prompt: string;
    values: Record<Field, string>;
  }

  /** The definition being edited (`previous`: its name; null for a new one). */
  let editing = $state<{
    previous: string | null;
    builtIn: boolean;
    form: Form;
  } | null>(null);
  let error = $state<string | null>(null);

  const SOURCES: Record<ListedPlatform["source"], string> = {
    builtIn: "built in",
    changed: "built in, changed",
    workspace: "workspace",
  };

  function toForm(definition: PlatformDefinition): Form {
    return {
      name: definition.name,
      otherNames: definition.otherNames.join(", "),
      prompt: definition.prompt,
      values: {
        disablePaging: definition.disablePaging ?? "",
        privilegeCommand: definition.privilegeCommand ?? "",
        privilegePrompt: definition.privilegePrompt ?? "",
        showConfig: definition.showConfig ?? "",
        ignoredLines: definition.ignoredLines ?? "",
      },
    };
  }

  function fromForm(form: Form): PlatformDefinition {
    const value = (text: string) => (text.trim() === "" ? null : text);
    return {
      name: form.name,
      otherNames: form.otherNames
        .split(",")
        .map((name) => name.trim())
        .filter((name) => name !== ""),
      prompt: form.prompt,
      disablePaging: value(form.values.disablePaging),
      privilegeCommand: value(form.values.privilegeCommand),
      privilegePrompt: value(form.values.privilegePrompt),
      showConfig: value(form.values.showConfig),
      ignoredLines: value(form.values.ignoredLines),
    };
  }

  async function run(change: () => Promise<void>): Promise<boolean> {
    try {
      await change();
      error = null;
      return true;
    } catch (failure) {
      error = String(failure);
      return false;
    }
  }

  async function save() {
    if (!editing) return;
    const { previous, form } = editing;
    if (await run(() => savePlatform(previous, fromForm(form)))) editing = null;
  }

  function edit(listed: ListedPlatform) {
    error = null;
    editing = {
      previous: listed.definition.name,
      builtIn: listed.source !== "workspace",
      form: toForm(listed.definition),
    };
  }

  function add() {
    error = null;
    editing = {
      previous: null,
      builtIn: false,
      form: toForm({
        name: "",
        otherNames: [],
        prompt: "",
        disablePaging: null,
        privilegeCommand: null,
        privilegePrompt: null,
        showConfig: null,
        ignoredLines: null,
      }),
    };
  }

  function cancel() {
    editing = null;
    error = null;
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    if (editing) cancel();
    else onclose();
  }

  function focusFirst(element: HTMLInputElement) {
    element.focus();
  }
</script>

<svelte:window {onkeydown} />

<div
  class="backdrop"
  role="dialog"
  aria-modal="true"
  aria-labelledby="platforms-title"
  data-dialog="platforms"
>
  <section>
    <h2 id="platforms-title">Platform definitions</h2>
    <p>
      How jobs drive each kind of device. A host's platform finds a definition
      by its name or one of its other names, ignoring case. Definitions are kept
      in the workspace (<code>_platforms.yaml</code>); a changed built-in keeps
      only what differs.
    </p>
    {#if error}
      <p class="error" role="alert" data-platform-error-message>{error}</p>
    {/if}

    {#if editing}
      {@const form = editing.form}
      <form
        class="fields"
        data-platform-form
        onsubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <label for="platform-name">Name</label>
        <input
          id="platform-name"
          name="platform-name"
          spellcheck="false"
          bind:value={form.name}
          disabled={editing.builtIn}
          {@attach (element) => {
            if (!editing?.builtIn) focusFirst(element);
          }}
        />
        <label for="platform-other-names">Other names</label>
        <input
          id="platform-other-names"
          name="platform-other-names"
          spellcheck="false"
          placeholder="arista_eos, ceos"
          bind:value={form.otherNames}
        />
        <label for="platform-prompt">Prompt</label>
        <input
          id="platform-prompt"
          name="platform-prompt"
          class="mono"
          spellcheck="false"
          placeholder={"[\\w.-]+[>#]"}
          bind:value={form.prompt}
        />
        {#each FIELDS as field (field.key)}
          <label for="platform-{field.id}">{field.label}</label>
          <input
            id="platform-{field.id}"
            name="platform-{field.id}"
            class:mono={field.pattern}
            spellcheck="false"
            placeholder="optional"
            bind:value={form.values[field.key]}
          />
        {/each}
        <small class="hint">
          Only the prompt is required. Prompt, privilege password prompt and
          ignored lines are regular expressions; the others are commands sent to
          the device. Separate other names with commas.
        </small>
        <div class="actions">
          <button type="button" data-platform-action="cancel" onclick={cancel}
            >Cancel</button
          >
          <button type="submit" data-platform-action="save">Save</button>
        </div>
      </form>
    {:else}
      <ul>
        {#each platforms as listed (listed.definition.name)}
          {@const definition = listed.definition}
          <li
            data-platform={definition.name}
            data-platform-source={listed.source}
            class:unusable={listed.error}
          >
            <span class="text">
              <strong>{definition.name}</strong>
              {#if definition.otherNames.length > 0}
                <small data-platform-other-names
                  >also {definition.otherNames.join(", ")}</small
                >
              {/if}
              <small class="mono" title={definition.prompt}
                >prompt {definition.prompt}</small
              >
              {#if listed.error}
                <small class="error" data-platform-error>{listed.error}</small>
              {/if}
            </span>
            <small class="source">{SOURCES[listed.source]}</small>
            <button data-platform-action="edit" onclick={() => edit(listed)}
              >Edit</button
            >
            {#if listed.source === "workspace"}
              <button
                data-platform-action="delete"
                onclick={() => void run(() => deletePlatform(definition.name))}
                >Delete</button
              >
            {:else if listed.source === "changed"}
              <button
                data-platform-action="reset"
                title="Back to the built-in definition"
                onclick={() => void run(() => deletePlatform(definition.name))}
                >Reset</button
              >
            {/if}
          </li>
        {/each}
      </ul>
      <div class="actions">
        <button data-platform-action="new" onclick={add}>New platform…</button>
        <!-- svelte-ignore a11y_autofocus -->
        <button autofocus onclick={onclose}>Close</button>
      </div>
    {/if}
  </section>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 15;
    display: grid;
    place-items: center;
    padding: var(--space-4);
    background: var(--backdrop);
  }

  section {
    width: min(42rem, 100%);
    max-height: 100%;
    overflow: auto;
    display: grid;
    gap: var(--space-3);
    padding: var(--space-4);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
  }

  p {
    margin: 0;
    color: var(--muted);
    font-size: var(--text-sm);
  }

  .error {
    color: var(--danger);
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .mono {
    font-family: var(--font-mono);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--space-1);
  }

  li {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    padding: var(--space-1-5) var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-size: var(--text-sm);
  }

  li.unusable .text strong {
    opacity: 0.6;
  }

  .text {
    flex: 1;
    min-width: 0;
    display: grid;
  }

  .text small {
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .text small.error {
    color: var(--danger);
    white-space: pre-wrap;
  }

  .source {
    color: var(--muted);
    white-space: nowrap;
  }

  li button {
    padding: var(--space-0-5) var(--space-2);
  }

  .fields {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: var(--space-2) var(--space-3);
    font-size: var(--text-sm);
  }

  .hint {
    grid-column: 1 / -1;
    color: var(--muted);
  }

  .fields .actions {
    grid-column: 1 / -1;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
