<script lang="ts">
  // The snippet editor tab (#10, item 8): a snippet's name, group and text
  // with room to read it, at least 80 columns without wrapping, line
  // numbers, and the variables found in the text listed beside it.
  // Subject: the snippet's name; null for a new snippet. Saved, a new (or
  // renamed) snippet's tab moves to the snippet's name, so a tab is never
  // about a name that is gone.
  import { onDestroy, untrack } from "svelte";
  import Button from "./Button.svelte";
  import { inventory, saveSnippet } from "./inventory.svelte";
  import {
    NEW_SNIPPET,
    registerSnippet,
    unregisterSnippet,
  } from "./snippetDrafts.svelte";
  import { variablesOf } from "./snippets.svelte";
  import { openView } from "./views";

  interface Props {
    /** The tab that shows this editor, to be asked about when it closes. */
    tab: number;
    /** The snippet's name; null: a new snippet. */
    subject: string | null;
    onclose: () => void;
  }

  let { tab, subject, onclose }: Props = $props();

  const original = untrack(() =>
    subject === null
      ? null
      : ((inventory.workspace?.snippets ?? []).find(
          (s) => s.name === subject,
        ) ?? null),
  );

  // Edited here; the tab starts from the snippet as it was when opened.
  const draft = $state({
    name: original?.name ?? "",
    group: original?.group ?? "",
    text: original?.text ?? "",
  });
  /** What was last saved, to tell whether there is anything to save. */
  let savedAs = $state({ ...draft });
  let previous = $state<string | null>(untrack(() => original?.name ?? null));
  let error = $state<string | null>(null);
  let saving = $state(false);
  let nameInput = $state<HTMLInputElement>();
  let area = $state<HTMLTextAreaElement>();
  let gutter = $state<HTMLElement>();

  const missing = $derived(subject !== null && original === null);
  const changed = $derived(
    draft.name !== savedAs.name ||
      draft.group !== savedAs.group ||
      draft.text !== savedAs.text,
  );
  const lines = $derived(draft.text.split(/\r\n|\n|\r/).length);
  const variables = $derived(variablesOf(draft.text));

  const groups = $derived([
    ...new Set(
      (inventory.workspace?.snippets ?? []).flatMap((s) =>
        s.group === null ? [] : [s.group],
      ),
    ),
  ]);

  // Take the keyboard from whatever had it (the terminal, say).
  $effect(() => {
    nameInput?.focus();
  });

  function syncGutter() {
    if (gutter && area) gutter.scrollTop = area.scrollTop;
  }

  /**
   * Saves the snippet. Resolves to whether it did. A renamed snippet's tab
   * follows its new name unless `follow` is false (the window saves it
   * before closing this very tab, which must not leave another behind).
   */
  async function save(follow = true): Promise<boolean> {
    if (missing) return true;
    if (saving) return false;
    saving = true;
    // What is sent: typing while it saves stays unsaved.
    const sent = { ...draft };
    try {
      await saveSnippet(previous, {
        name: sent.name,
        group: sent.group.trim() === "" ? null : sent.group,
        text: sent.text,
      });
      error = null;
      const renamed = sent.name !== previous;
      previous = sent.name;
      savedAs = sent;
      if (follow && renamed && !changed) {
        // The tab follows the snippet's new name.
        openView("snippet", draft.name);
        onclose();
      }
      return true;
    } catch (failure) {
      error = String(failure);
      return false;
    } finally {
      saving = false;
    }
  }

  // Listed so that closing the tab, quitting or opening another workspace
  // asks about unsaved changes first.
  $effect(() => {
    registerSnippet(tab, {
      name: draft.name.trim() === "" ? NEW_SNIPPET : draft.name.trim(),
      changed,
      save: () => save(false),
      error: () => error,
    });
  });
  onDestroy(() => unregisterSnippet(tab));

  function onkeydown(event: KeyboardEvent) {
    if (
      event.key.toLowerCase() === "s" &&
      (event.ctrlKey || event.metaKey) &&
      !event.shiftKey &&
      !event.altKey
    ) {
      event.preventDefault();
      void save();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class="view"
  data-view="snippet"
  data-snippet-editor={previous ?? ""}
  data-unsaved={changed || undefined}
  {onkeydown}
>
  {#if missing}
    <p class="empty">
      This snippet is no longer in the workspace.
      <Button size="compact" onclick={onclose}>Close tab</Button>
    </p>
  {:else}
    <form
      data-snippet-form
      data-snippet-editing={previous ?? ""}
      aria-label={previous === null ? "New snippet" : `Edit ${previous}`}
      onsubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <div class="bar">
        <span class="state" aria-live="polite">
          {#if changed}Unsaved changes{:else if previous !== null}Saved{/if}
        </span>
        <Button
          size="compact"
          data-snippet-cancel
          title="Close this tab, dropping unsaved changes"
          onclick={onclose}>{changed ? "Cancel" : "Close"}</Button
        >
        <Button
          size="compact"
          variant="primary"
          type="submit"
          data-snippet-save
          disabled={saving || (!changed && previous !== null)}
          title="Write the snippet to the workspace (Ctrl+S)">Save</Button
        >
      </div>
      {#if error}
        <p class="error" role="alert" data-snippet-error>Not saved: {error}</p>
      {/if}
      <div class="body">
        <div class="fields">
          <label for="snippet-name">Name</label>
          <input
            id="snippet-name"
            name="snippet-name"
            autocomplete="off"
            bind:this={nameInput}
            bind:value={draft.name}
          />
          <label for="snippet-group">Group</label>
          <input
            id="snippet-group"
            name="snippet-group"
            autocomplete="off"
            placeholder="None"
            list="snippet-groups"
            bind:value={draft.group}
          />
          <datalist id="snippet-groups">
            {#each groups as group (group)}
              <option value={group}></option>
            {/each}
          </datalist>
        </div>
        <div class="split">
          <div class="text">
            <label for="snippet-text">Text</label>
            <div class="editor">
              <div class="gutter" aria-hidden="true" bind:this={gutter}>
                {#each { length: lines }, index (index)}
                  <div>{index + 1}</div>
                {/each}
              </div>
              <textarea
                id="snippet-text"
                name="snippet-text"
                wrap="off"
                spellcheck="false"
                autocomplete="off"
                placeholder={"show interface {{interface}}"}
                bind:this={area}
                bind:value={draft.text}
                onscroll={syncGutter}></textarea>
            </div>
          </div>
          <aside class="found" data-snippet-found>
            <h3>
              Variables
              <span class="count" data-snippet-variable-count
                >{variables.length}</span
              >
            </h3>
            {#if variables.length === 0}
              <p class="none">None found yet. Write one as {"{{name}}"}.</p>
            {:else}
              <ul>
                {#each variables as name (name.toLowerCase())}
                  <li>
                    <code data-snippet-found-variable={name}>{name}</code>
                  </li>
                {/each}
              </ul>
            {/if}
            <details>
              <summary>How variables work</summary>
              <p>
                Each line is sent as a line of its own. Host fields fill
                variables: <code>{"{{name}}"}</code>,
                <code>{"{{address}}"}</code>, <code>{"{{site}}"}</code>,
                <code>{"{{custom.circuit}}"}</code> and so on. You are asked for
                any other <code>{"{{variable}}"}</code>, and for fields the host
                leaves empty, when the snippet is sent. Names are matched
                ignoring case and the blanks inside the braces. Snippets are
                kept in the workspace (<code>_snippets.yaml</code>).
              </p>
            </details>
          </aside>
        </div>
      </div>
    </form>
  {/if}
</div>

<style>
  .view {
    height: 100%;
    background: var(--bg);
    container-type: inline-size;
  }

  form {
    display: flex;
    flex-direction: column;
    height: 100%;
    font-size: var(--text-sm);
  }

  .bar {
    display: flex;
    align-items: center;
    justify-content: flex-end;
    gap: var(--space-2);
    padding: var(--space-1-5) var(--space-4);
    border-bottom: 1px solid var(--border);
    background: var(--surface);
  }

  .state {
    margin-right: auto;
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .error {
    margin: 0;
    padding: var(--space-2) var(--space-4);
    color: var(--danger);
    border-bottom: 1px solid var(--border);
    background: var(--surface);
    overflow-wrap: anywhere;
  }

  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    display: flex;
    flex-direction: column;
    gap: var(--space-3);
    padding: var(--space-4);
  }

  .fields {
    display: grid;
    grid-template-columns: 6rem minmax(0, 24rem);
    justify-content: start;
    align-items: center;
    gap: var(--space-1-5) var(--space-3);
  }

  label {
    color: var(--muted);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
  }

  .split {
    flex: 1;
    min-height: 20rem;
    display: flex;
    gap: var(--space-4);
    align-items: stretch;
  }

  /* No min-width: 0: the text keeps its 82 columns and the tab scrolls. */
  .text {
    flex: 1 1 auto;
    display: flex;
    flex-direction: column;
    gap: var(--space-1);
  }

  .editor {
    flex: 1;
    min-height: 16rem;
    display: flex;
    background: var(--input);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-md);
  }

  .editor:focus-within {
    border-color: var(--accent);
  }

  .gutter,
  textarea {
    margin: 0;
    padding: var(--space-2) 0;
    font-family: var(--font-mono);
    font-size: var(--text-sm);
    line-height: 1.5;
  }

  .gutter {
    flex: none;
    min-width: 4ch;
    padding-right: var(--space-2);
    padding-left: var(--space-2);
    overflow: hidden;
    text-align: right;
    color: var(--muted);
    background: var(--surface);
    border-right: 1px solid var(--border);
    user-select: none;
    font-variant-numeric: tabular-nums;
  }

  textarea {
    flex: 1;
    min-width: 82ch;
    padding-left: var(--space-3);
    padding-right: var(--space-2);
    border: 0;
    border-radius: 0;
    background: none;
    resize: none;
    white-space: pre;
    overflow: auto;
    outline: none;
  }

  .found {
    flex: 0 0 16rem;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  h3 {
    margin: 0;
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
  }

  .count {
    margin-left: var(--space-1);
    color: var(--muted);
    font-weight: normal;
    font-size: var(--text-xs);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: var(--space-1);
  }

  li code {
    overflow-wrap: anywhere;
  }

  .none,
  details p {
    margin: 0;
    color: var(--muted);
    font-size: var(--text-xs);
    line-height: 1.5;
  }

  summary {
    cursor: pointer;
    color: var(--text);
    font-size: var(--text-xs);
    margin-bottom: var(--space-1);
  }

  .empty {
    padding: var(--space-6) var(--space-4);
    color: var(--muted);
    line-height: 1.5;
  }

  /* Narrow tabs: the variables go below the text. */
  @container (max-width: 70rem) {
    .split {
      flex-direction: column;
    }

    .found {
      flex: none;
    }
  }
</style>
