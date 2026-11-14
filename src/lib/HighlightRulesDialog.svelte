<script lang="ts">
  import { currentRules } from "./highlight.svelte";
  import {
    deleteHighlightRule,
    inventory,
    saveHighlightRule,
  } from "./inventory.svelte";
  import {
    changePreferences,
    highlightColor,
    isDark,
    preferences,
  } from "./preferences.svelte";
  import {
    HIGHLIGHT_COLOR_NAMES,
    type HighlightRule,
    type ListedRule,
  } from "./workspace";

  interface Props {
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  const rules = $derived(currentRules());
  const editable = $derived(inventory.workspace !== null);

  /** The rule being edited (`previous`: its name; null for a new one). */
  let editing = $state<{
    previous: string | null;
    builtIn: boolean;
    rule: HighlightRule;
  } | null>(null);
  let error = $state<string | null>(null);
  /** The custom colour field shows when the colour is not a palette name. */
  const custom = $derived(
    editing !== null &&
      !(HIGHLIGHT_COLOR_NAMES as readonly string[]).includes(
        editing.rule.color,
      ),
  );

  const SOURCES: Record<ListedRule["source"], string> = {
    builtIn: "built in",
    changed: "built in, changed",
    workspace: "workspace",
  };

  function swatch(color: string) {
    return highlightColor(color, isDark());
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

  function toggle(listed: ListedRule, enabled: boolean) {
    void run(() =>
      saveHighlightRule(listed.rule.name, { ...listed.rule, enabled }),
    );
  }

  async function save() {
    if (!editing) return;
    const { previous, rule } = editing;
    if (await run(() => saveHighlightRule(previous, rule))) editing = null;
  }

  function edit(listed: ListedRule) {
    error = null;
    editing = {
      previous: listed.rule.name,
      builtIn: listed.source !== "workspace",
      rule: { ...listed.rule },
    };
  }

  function add() {
    error = null;
    editing = {
      previous: null,
      builtIn: false,
      rule: { name: "", pattern: "", color: "yellow", enabled: true },
    };
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    if (editing) {
      editing = null;
      error = null;
    } else onclose();
  }
</script>

<svelte:window {onkeydown} />

<div
  class="backdrop"
  role="dialog"
  aria-modal="true"
  aria-labelledby="highlights-title"
  data-dialog="highlights"
>
  <section>
    <h2 id="highlights-title">Highlighting</h2>
    <label class="check">
      <input
        type="checkbox"
        name="highlighting"
        checked={preferences.current.highlighting}
        onchange={(event) =>
          changePreferences({ highlighting: event.currentTarget.checked })}
      />
      Highlight keywords in terminal output (on this computer)
    </label>
    <p>
      Only the colours on screen change: what is sent, received and logged stays
      the same. Rules are kept in the workspace (<code>_highlights.yaml</code>);
      the first rule wins where two overlap.
    </p>
    {#if error}
      <p class="error" role="alert" data-rule-error-message>{error}</p>
    {/if}

    {#if editing}
      {@const rule = editing.rule}
      <form
        class="fields"
        data-rule-form
        onsubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <label for="rule-name">Name</label>
        <input
          id="rule-name"
          name="rule-name"
          bind:value={rule.name}
          disabled={editing.builtIn}
        />
        <label for="rule-pattern">Pattern</label>
        <input
          id="rule-pattern"
          name="rule-pattern"
          class="mono"
          spellcheck="false"
          placeholder={"(?i)\\bdown\\b"}
          bind:value={rule.pattern}
        />
        <label for="rule-color">Colour</label>
        <span class="color">
          <span class="swatch" style:background={swatch(rule.color)}></span>
          <select
            id="rule-color"
            name="rule-color"
            value={custom ? "custom" : rule.color}
            onchange={(event) => {
              const value = event.currentTarget.value;
              rule.color = value === "custom" ? swatch(rule.color) : value;
            }}
          >
            {#each HIGHLIGHT_COLOR_NAMES as name (name)}
              <option value={name}>{name.replace("-", " ")}</option>
            {/each}
            <option value="custom">Other colour…</option>
          </select>
          {#if custom}
            <input
              type="color"
              name="rule-custom-color"
              aria-label="Colour"
              bind:value={rule.color}
            />
          {/if}
        </span>
        <span></span>
        <label class="check">
          <input
            type="checkbox"
            name="rule-enabled"
            bind:checked={rule.enabled}
          />
          On
        </label>
        <small class="hint">
          A regular expression; <code>(?i)</code> ignores case, <code>\b</code>
          marks a word's edge.
        </small>
        <div class="actions">
          <button
            type="button"
            onclick={() => {
              editing = null;
              error = null;
            }}>Cancel</button
          >
          <button type="submit" data-rule="save">Save</button>
        </div>
      </form>
    {:else}
      <ul>
        {#each rules as listed (listed.rule.name)}
          <li
            data-highlight-rule={listed.rule.name}
            data-highlight-source={listed.source}
            class:off={!listed.rule.enabled || listed.error}
          >
            <input
              type="checkbox"
              name="rule-enabled"
              aria-label={`Highlight ${listed.rule.name}`}
              checked={listed.rule.enabled}
              disabled={!editable}
              onchange={(event) => toggle(listed, event.currentTarget.checked)}
            />
            <span class="swatch" style:background={swatch(listed.rule.color)}
            ></span>
            <span class="text">
              <strong>{listed.rule.name}</strong>
              <small class="mono" title={listed.rule.pattern}
                >{listed.rule.pattern}</small
              >
              {#if listed.error}
                <small class="error" data-rule-error>{listed.error}</small>
              {/if}
            </span>
            <small class="source">{SOURCES[listed.source]}</small>
            {#if editable}
              <button data-rule="edit" onclick={() => edit(listed)}>Edit</button
              >
              {#if listed.source === "workspace"}
                <button
                  data-rule="delete"
                  onclick={() =>
                    void run(() => deleteHighlightRule(listed.rule.name))}
                  >Delete</button
                >
              {:else if listed.source === "changed"}
                <button
                  data-rule="reset"
                  title="Back to the built-in rule"
                  onclick={() =>
                    void run(() => deleteHighlightRule(listed.rule.name))}
                  >Reset</button
                >
              {/if}
            {/if}
          </li>
        {/each}
      </ul>
      {#if !editable}
        <p>Open a workspace to add rules or change these.</p>
      {/if}
      <div class="actions">
        {#if editable}
          <button data-rule="new" onclick={add}>New rule…</button>
        {/if}
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
    padding: 1rem;
    background: rgb(0 0 0 / 0.45);
  }

  section {
    width: min(40rem, 100%);
    max-height: 100%;
    overflow: auto;
    display: grid;
    gap: 0.8rem;
    padding: 1.2rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.4);
  }

  h2 {
    margin: 0;
    font-size: 1.05rem;
    font-weight: 600;
  }

  p {
    margin: 0;
    color: var(--muted);
    font-size: 0.85rem;
  }

  .error {
    color: var(--danger);
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .check {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.85rem;
  }

  .mono {
    font-family: "Cascadia Mono", Consolas, monospace;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.3rem;
  }

  li {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.4rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    font-size: 0.85rem;
  }

  li.off .text {
    opacity: 0.6;
  }

  .swatch {
    flex: none;
    width: 0.9rem;
    height: 0.9rem;
    border-radius: 3px;
    border: 1px solid var(--border);
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
    padding: 0.2rem 0.6rem;
  }

  .fields {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: 0.5rem 0.8rem;
    font-size: 0.85rem;
  }

  .color {
    display: flex;
    align-items: center;
    gap: 0.5rem;
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
    gap: 0.6rem;
  }
</style>
