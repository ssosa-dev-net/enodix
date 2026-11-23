<script lang="ts">
  // The highlight rules and the highlighting switch, in their tab (#72; one
  // at a time), so a pattern can be tried on a terminal beside it.
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
    /** Closes the tab. */
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

  // Escape leaves the form; the tab closes as any tab does.
  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && editing) {
      editing = null;
      error = null;
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="view" data-view="highlights" {onkeydown}>
  <section class="inner" aria-labelledby="highlights-title">
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
        <button data-rule="close" onclick={onclose}>Close</button>
      </div>
    {/if}
  </section>
</div>

<style>
  .view {
    height: 100%;
    min-height: 0;
    overflow: auto;
    background: var(--bg);
  }

  .inner {
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
    color: var(--muted);
    font-size: var(--text-sm);
  }

  .error {
    color: var(--danger);
    overflow-wrap: anywhere;
    white-space: pre-wrap;
  }

  .check {
    display: flex;
    align-items: center;
    gap: var(--space-1-5);
    font-size: var(--text-sm);
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

  /* An off rule is quieter, still readable (a colour, not opacity). */
  li.off .text strong,
  li.off .text .mono {
    color: var(--muted);
  }

  .swatch {
    flex: none;
    width: 0.9rem;
    height: 0.9rem;
    border-radius: var(--radius-sm);
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
    min-height: var(--control-height-small);
    padding: 0 var(--space-2);
  }

  .fields {
    display: grid;
    grid-template-columns: auto minmax(0, 1fr);
    align-items: center;
    gap: var(--space-2) var(--space-3);
    font-size: var(--text-sm);
  }

  .color {
    display: flex;
    align-items: center;
    gap: var(--space-2);
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
