<script lang="ts">
  import IconButton from "./IconButton.svelte";
  // The Snippets side view: the workspace's snippets by group, a filter the
  // core matches (every word in the name, group or text, as the palette's
  // `>` search), and one click sending a snippet to the active pane's
  // session through the app's one way of sending (`onsend`), so variables,
  // paste safety and multi-send groups are as everywhere else. Editing a
  // snippet opens it in a tab (SnippetEditor).
  import Pencil from "@lucide/svelte/icons/pencil";
  import Plus from "@lucide/svelte/icons/plus";
  import Search from "@lucide/svelte/icons/search";
  import Trash from "@lucide/svelte/icons/trash";
  import X from "@lucide/svelte/icons/x";
  import { inventory, removeSnippet } from "./inventory.svelte";
  import { bySnippetGroup, snippets } from "./snippets.svelte";
  import { openView } from "./views";
  import { findSnippets, type Snippet } from "./workspace";
  import { firstSent, stepsText } from "./steps";

  interface Props {
    /** The session snippets go to (the active pane's), by name; null: none. */
    target: string | null;
    onsend: (name: string) => void;
  }

  let { target, onsend }: Props = $props();

  // New and Edit open the snippet in a tab of its own (#10): room for its
  // lines, with the variables found listed beside it.
  const newSnippet = () => openView("snippet", null);
  const editSnippet = (snippet: Snippet) => openView("snippet", snippet.name);

  const workspace = $derived(inventory.workspace);
  const list = $derived(workspace?.snippets ?? []);

  /** The names the core found for the filter; null: no filter. */
  let found = $state<Set<string> | null>(null);
  let asked = 0;
  $effect(() => {
    const query = snippets.query.trim();
    void list; // asked again when the snippets change
    const mine = ++asked;
    if (!query) {
      found = null;
      return;
    }
    findSnippets(query).then(
      (hits) => {
        if (mine === asked) found = new Set(hits.map((hit) => hit.name));
      },
      () => {
        if (mine === asked) found = new Set();
      },
    );
  });

  // File order within each group, whatever order the core ranked them in.
  const shown = $derived(
    found === null ? list : list.filter((snippet) => found!.has(snippet.name)),
  );
  const groups = $derived(bySnippetGroup(shown));

  /** What a snippet sends, as its row's tooltip shows it. */
  function shownText(snippet: Snippet) {
    return snippet.steps ? stepsText(snippet.steps) : snippet.text;
  }

  function firstLine(text: string) {
    const lines = text.split(/\r\n|\n|\r/).filter((line) => line.trim());
    return lines.length > 1
      ? `${lines[0]}  (+${lines.length - 1} line${lines.length === 2 ? "" : "s"})`
      : (lines[0] ?? "");
  }
</script>

<div class="snippets" data-snippets>
  <header data-window-bar data-tauri-drag-region="deep">
    <h2>
      Snippets
      {#if list.length > 0}<span class="count">{list.length}</span>{/if}
    </h2>
    <IconButton
      icon={Plus}
      data-snippet-new
      label="New snippet"
      title="New snippet, saved in the workspace for the team"
      disabled={!workspace}
      onclick={() => newSnippet()}
    />
  </header>

  {#if !workspace}
    <p class="hint" data-needs-workspace>
      Snippets live in a workspace: open or create one to see them here.
    </p>
  {:else if list.length === 0}
    <div class="empty" data-snippets-empty>
      <p>
        Snippets are commands you send often, saved in the workspace for the
        team, with host fields such as <code>{"{{address}}"}</code> filled in when
        sent.
      </p>
      <button data-snippet-first onclick={() => newSnippet()}
        ><Plus size={14} aria-hidden="true" /> New snippet</button
      >
    </div>
  {:else}
    <div class="filtering">
      <label class="field">
        <Search size={14} aria-hidden="true" />
        <input
          name="snippet-filter"
          data-snippet-filter
          type="search"
          placeholder="Filter snippets"
          aria-label="Filter snippets"
          autocomplete="off"
          spellcheck="false"
          bind:value={snippets.query}
          onkeydown={(event) => {
            if (event.key === "Escape" && snippets.query) {
              event.stopPropagation();
              snippets.query = "";
            }
          }}
        />
        {#if snippets.query}
          <button
            class="clear"
            aria-label="Clear the filter"
            title="Clear the filter"
            onclick={() => (snippets.query = "")}
            ><X size={14} aria-hidden="true" /></button
          >
        {/if}
      </label>
      <p class="target" data-snippet-target={target ?? ""}>
        {#if target === null}
          Open a session to send snippets to it.
        {:else}
          Click to send to <strong>{target}</strong>
        {/if}
      </p>
    </div>
    <div class="body">
      {#if groups.length === 0}
        <p class="hint" data-snippets-none>
          No snippet has every word of “{snippets.query.trim()}” in its name,
          group or text.
        </p>
      {/if}
      {#each groups as { group, snippets: members } (group)}
        <div class="group" data-snippet-group={group ?? ""}>
          {#if group !== null}<h3>{group}</h3>{/if}
          <ul>
            {#each members as snippet (snippet.name)}
              <li>
                <button
                  class="send"
                  data-snippet={snippet.name}
                  disabled={target === null}
                  title={target === null
                    ? shownText(snippet)
                    : `${snippet.steps ? "Run on" : "Send to"} ${target}:\n${shownText(snippet)}`}
                  onclick={() => onsend(snippet.name)}
                >
                  <span class="name"
                    >{snippet.name}{#if snippet.steps}<span
                        class="steps"
                        data-snippet-steps
                        title="Runs step by step, waiting for the device"
                        >Steps</span
                      >{/if}</span
                  >
                  <span class="preview" data-snippet-preview
                    >{snippet.steps
                      ? firstSent(snippet.steps)
                      : firstLine(snippet.text)}</span
                  >
                </button>
                <IconButton
                  icon={Pencil}
                  size="compact"
                  class="row-action"
                  data-snippet-edit={snippet.name}
                  title="Edit"
                  label={`Edit ${snippet.name}`}
                  onclick={() => editSnippet(snippet)}
                />
                <IconButton
                  icon={Trash}
                  size="compact"
                  class="row-action"
                  data-snippet-delete={snippet.name}
                  title="Delete"
                  label={`Delete ${snippet.name}`}
                  onclick={() => removeSnippet(snippet.name)}
                />
              </li>
            {/each}
          </ul>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .snippets {
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    font-size: var(--text-sm);
  }

  header {
    display: flex;
    align-items: center;
    gap: var(--space-0-5);
    padding: var(--space-1-5) var(--space-1-5) var(--space-1-5) var(--space-3);
    border-bottom: 1px solid var(--border);
  }

  h2 {
    flex: 1;
    margin: 0;
    padding: var(--space-1) 0;
    font-size: var(--text-lg);
    font-weight: var(--weight-semibold);
  }

  .count {
    margin-left: var(--space-1);
    color: var(--muted);
    font-weight: normal;
    font-size: var(--text-xs);
    font-variant-numeric: tabular-nums;
  }

  .filtering {
    display: grid;
    gap: var(--space-1-5);
    padding: var(--space-2);
    border-bottom: 1px solid var(--border);
  }

  .field {
    display: flex;
    align-items: center;
    height: var(--control-height);
    gap: var(--space-1-5);
    padding: 0 var(--space-1-5) 0 var(--space-2);
    background: var(--input);
    border: 1px solid var(--control-border);
    border-radius: var(--radius-md);
    color: var(--muted);
  }

  .field:focus-within {
    border-color: var(--accent);
  }

  .field input {
    flex: 1;
    min-width: 0;
    border: 0;
    background: none;
    height: 100%;
    padding: 0;
    font-size: var(--text-sm);
    outline: none;
  }

  .field input::-webkit-search-cancel-button {
    display: none;
  }

  .field .clear {
    display: inline-grid;
    place-items: center;
    width: var(--control-height-small);
    height: var(--control-height-small);
    min-height: 0;
    padding: 0;
    border: 0;
    background: none;
    color: var(--muted);
  }

  .target {
    margin: 0;
    padding: 0 var(--space-0-5);
    color: var(--muted);
    font-size: var(--text-xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .target strong {
    color: var(--text);
    font-weight: var(--weight-semibold);
  }

  .body {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: var(--space-0-5) 0 var(--space-2);
  }

  .hint {
    margin: var(--space-2) var(--space-3);
    color: var(--muted);
    font-size: var(--text-xs);
    line-height: 1.5;
  }

  .empty {
    display: grid;
    justify-items: start;
    gap: var(--space-2);
    padding: var(--space-3);
    color: var(--muted);
    font-size: var(--text-xs);
    line-height: 1.5;
  }

  .empty p {
    margin: 0;
    line-height: 1.5;
  }

  .empty button {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
  }

  h3 {
    margin: var(--space-2) var(--space-3) var(--space-0-5);
    color: var(--text);
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  li {
    display: flex;
    align-items: center;
    gap: var(--space-0-5);
    padding: 0 var(--space-2) 0 var(--space-1-5);
  }

  .send {
    flex: 1;
    min-width: 0;
    display: grid;
    gap: 1px;
    text-align: left;
    border-color: transparent;
    background: none;
    justify-content: normal;
    min-height: 0;
    padding: var(--space-1) var(--space-1-5);
  }

  .send .name,
  .send .preview {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .send .steps {
    margin-left: var(--space-1-5);
    font-size: var(--text-xs);
    color: var(--accent);
  }

  .send .preview {
    color: var(--muted);
    font-family: var(--font-mono);
    font-size: var(--text-2xs);
  }

  .send:not(:disabled):hover {
    background: var(--surface-raised);
  }

  .send:disabled {
    opacity: 0.75;
  }

  /* The row's actions show on hover or focus. */
  li :global(.row-action) {
    visibility: hidden;
  }

  li:hover :global(.row-action),
  li:focus-within :global(.row-action) {
    visibility: visible;
  }
</style>
