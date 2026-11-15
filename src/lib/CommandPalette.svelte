<script lang="ts">
  import { tick } from "svelte";
  import RoleIcon from "./RoleIcon.svelte";
  import type { ViewName } from "./layout";
  import { openView, VIEWS } from "./views";
  import {
    fieldLabel,
    hostRole,
    inventory,
    revealHost,
  } from "./inventory.svelte";
  import {
    findHost,
    findSnippets,
    parentPath,
    searchHosts,
    type Host,
    type SearchHit,
    type SearchResults,
    type Snippet,
  } from "./workspace";

  interface Props {
    onconnect: (host: Host) => void;
    /** Sends the snippet called so to the active session. */
    onsnippet: (name: string) => void;
    /** The session snippets go to, by name; null: none. */
    target: string | null;
    onclose: () => void;
  }

  let { onconnect, onsnippet, target, onclose }: Props = $props();

  /** A query starting with this searches the snippets instead of hosts. */
  const SNIPPET_PREFIX = ">";

  /** The most hits shown at once. */
  const LIMIT = 50;

  /** Views the palette opens (#72), listed after the hosts when every
   * typed word is part of their label or keywords. */
  const COMMANDS: readonly { view: ViewName; label: string; words: string }[] =
    [
      {
        view: "platforms",
        label: "Platform definitions",
        words: "platforms prompts paging privilege show config devices jobs",
      },
      {
        view: "highlights",
        label: "Highlight rules",
        words: "highlighting keywords patterns colours colors",
      },
    ];

  let query = $state("");
  let results = $state<SearchResults | null>(null);
  /** The query `results` answer, which lags while the core answers. */
  let answered: string | null = null;
  let active = $state(0);
  /** The snippets found, when the query searches snippets; else null. */
  let snippetHits = $state<Snippet[] | null>(null);
  const snippetMode = $derived(query.startsWith(SNIPPET_PREFIX));
  /** The views matching the query (none for an empty or snippet query). */
  const commands = $derived.by(() => {
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    if (snippetMode || words.length === 0) return [];
    return COMMANDS.filter((command) => {
      const text = `${command.label} ${command.words}`.toLowerCase();
      return words.every((word) => text.includes(word));
    });
  });
  /** The hosts listed (before the commands). */
  const hostCount = $derived(snippetHits ? 0 : (results?.hits.length ?? 0));
  let list = $state<HTMLElement>();
  let input = $state<HTMLInputElement>();

  // Take the keyboard from whatever had it (the terminal, say).
  $effect(() => {
    input?.focus();
  });

  // Every keystroke asks the core's index; a late answer to an older query
  // is dropped.
  $effect(() => {
    const typed = query;
    void inventory.workspace;
    let stale = false;
    const search = typed.startsWith(SNIPPET_PREFIX)
      ? findSnippets(typed.slice(SNIPPET_PREFIX.length)).then((found) => {
          if (stale) return;
          snippetHits = found.slice(0, LIMIT);
          results = null;
        })
      : searchHosts(typed, LIMIT).then((found) => {
          if (stale) return;
          results = found;
          snippetHits = null;
        });
    search
      .then(() => {
        if (stale) return;
        answered = typed;
        active = 0;
      })
      .catch((error) => {
        if (!stale) inventory.editError = String(error);
      });
    return () => {
      stale = true;
    };
  });

  /** The first field that matched, if it is not the name or address. */
  function detail(hit: SearchHit) {
    const other = hit.matches.find(
      (match) => match.field !== "name" && match.field !== "address",
    );
    return other ? `${fieldLabel(other.field)}: ${other.value}` : null;
  }

  function connect(hit: SearchHit) {
    const tree = inventory.workspace?.tree;
    const host = tree ? findHost(tree, hit.id) : null;
    onclose();
    if (host) onconnect(host);
  }

  async function show(hit: SearchHit) {
    onclose();
    revealHost(hit.id);
    await tick();
    const row = document.querySelector<HTMLElement>(
      `nav [data-host="${CSS.escape(hit.id)}"]`,
    );
    row?.scrollIntoView({ block: "nearest" });
    row?.focus();
  }

  function send(snippet: Snippet) {
    onclose();
    onsnippet(snippet.name);
  }

  function run(view: ViewName) {
    onclose();
    openView(view);
  }

  async function move(by: number) {
    const count =
      snippetHits?.length ?? (results?.hits.length ?? 0) + commands.length;
    if (count === 0) return;
    active = (active + by + count) % count;
    await tick();
    list
      ?.querySelector(`[data-index="${active}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }

  /** The chosen hit, for the query as typed now: if Enter beats the
   * answer to the last keystroke, the best hit of a fresh search. */
  async function chosen(): Promise<SearchHit | undefined> {
    if (answered === query) return results?.hits[active];
    return (await searchHosts(query, 1)).hits[0];
  }

  /** The same for a snippet search. */
  async function chosenSnippet(): Promise<Snippet | undefined> {
    if (answered === query) return snippetHits?.[active];
    return (await findSnippets(query.slice(SNIPPET_PREFIX.length)))[0];
  }

  async function onkeydown(event: KeyboardEvent) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      move(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      move(-1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      if (snippetMode) {
        const snippet = await chosenSnippet();
        if (snippet && target !== null) send(snippet);
        return;
      }
      // A command chosen (they follow the hosts listed).
      const command =
        answered === query ? commands[active - hostCount] : undefined;
      if (command) {
        run(command.view);
        return;
      }
      const reveal = event.shiftKey;
      const hit = await chosen();
      if (!hit) {
        // No host matches: the first command, if one does.
        if (commands[0]) run(commands[0].view);
        return;
      }
      if (reveal) show(hit);
      else connect(hit);
    } else if (event.key === "Escape") {
      event.preventDefault();
      onclose();
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div
  class="backdrop"
  onclick={(event) => {
    if (event.target === event.currentTarget) onclose();
  }}
>
  <div
    class="palette"
    role="dialog"
    aria-modal="true"
    aria-label="Search hosts"
  >
    <input
      bind:this={input}
      name="palette-query"
      placeholder="Search hosts by name, address, tag or field; > for snippets"
      bind:value={query}
      {onkeydown}
      autocomplete="off"
      spellcheck="false"
      role="combobox"
      aria-expanded="true"
      aria-controls="palette-results"
      aria-activedescendant={(snippetHits?.length ??
      (results?.hits.length ?? 0) + commands.length)
        ? `palette-hit-${active}`
        : undefined}
    />
    <ul id="palette-results" role="listbox" bind:this={list}>
      {#each snippetHits ?? [] as snippet, index (snippet.name)}
        <li
          id="palette-hit-{index}"
          role="option"
          aria-selected={index === active}
          aria-disabled={target === null}
          class:active={index === active}
          data-index={index}
          data-snippet-hit={snippet.name}
          onclick={() => {
            if (target !== null) send(snippet);
          }}
          onpointermove={() => (active = index)}
        >
          <span class="icon" aria-hidden="true">&gt;</span>
          <span class="name">{snippet.name}</span>
          <span class="detail">{snippet.text.split(/\r?\n/)[0]}</span>
          {#if snippet.group}<span class="folder">{snippet.group}</span>{/if}
        </li>
      {/each}
      {#each results?.hits ?? [] as hit, index (hit.id)}
        {@const role = hostRole(hit.id)}
        {@const folder = parentPath(hit.id)}
        {@const more = detail(hit)}
        <li
          id="palette-hit-{index}"
          role="option"
          aria-selected={index === active}
          class:active={index === active}
          data-index={index}
          data-hit={hit.id}
          onclick={() => connect(hit)}
          onpointermove={() => (active = index)}
        >
          <span class="icon">
            {#if role}<RoleIcon {role} />{/if}
          </span>
          <span class="name">{hit.name}</span>
          <span class="address">{hit.address}</span>
          {#if more}<span class="detail">{more}</span>{/if}
          {#if folder}<span class="folder">{folder}</span>{/if}
        </li>
      {/each}
      {#each commands as command, offset (command.view)}
        {@const index = hostCount + offset}
        {@const Icon = VIEWS[command.view].icon}
        <li
          id="palette-hit-{index}"
          role="option"
          aria-selected={index === active}
          class:active={index === active}
          data-index={index}
          data-palette-command={command.view}
          onclick={() => run(command.view)}
          onpointermove={() => (active = index)}
        >
          <span class="icon"><Icon size={14} aria-hidden="true" /></span>
          <span class="name">{command.label}</span>
          <span class="detail">Open in a tab</span>
        </li>
      {/each}
    </ul>
    {#if results && results.total === 0 && commands.length === 0}
      <p class="empty">No host matches.</p>
    {/if}
    {#if snippetHits && snippetHits.length === 0}
      <p class="empty">No snippet matches.</p>
    {:else if snippetHits && target === null}
      <p class="empty">Open a session to send snippets to it.</p>
    {/if}
    <footer>
      <span>
        {#if snippetHits}
          {snippetHits.length === 1
            ? "1 snippet"
            : `${snippetHits.length} snippets`}
        {:else if results && results.total > results.hits.length}
          {results.hits.length} of {results.total} hosts
        {:else if results}
          {results.total === 1 ? "1 host" : `${results.total} hosts`}
        {/if}
      </span>
      <span class="keys">
        {#if snippetMode}
          <kbd>Enter</kbd> send{target === null ? "" : ` to ${target}`} ·
          <kbd>Esc</kbd> close
        {:else}
          <kbd>Enter</kbd> connect · <kbd>Shift+Enter</kbd> show in tree ·
          <kbd>&gt;</kbd> snippets · <kbd>Esc</kbd> close
        {/if}
      </span>
    </footer>
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
    padding: 12vh var(--space-4) var(--space-4);
    background: var(--backdrop);
  }

  .palette {
    width: min(38rem, 100%);
    max-height: 70vh;
    display: flex;
    flex-direction: column;
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
    overflow: hidden;
  }

  input {
    margin: var(--space-2);
    padding: var(--space-1-5) var(--space-2);
    font-size: var(--text-md);
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0 0 var(--space-1);
    overflow: auto;
    min-height: 0;
  }

  li {
    display: flex;
    align-items: baseline;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-3);
    font-size: var(--text-sm);
    cursor: pointer;
    white-space: nowrap;
  }

  li.active {
    background: var(--surface);
    box-shadow: inset 2px 0 var(--accent);
  }

  .icon {
    width: 1rem;
    flex: none;
  }

  .name {
    font-weight: var(--weight-semibold);
  }

  .address,
  .detail,
  .folder {
    color: var(--muted);
    font-size: var(--text-xs);
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .folder {
    margin-left: auto;
    padding-left: var(--space-2);
  }

  .empty {
    margin: 0 var(--space-3) var(--space-2);
    color: var(--muted);
    font-size: var(--text-sm);
  }

  footer {
    display: flex;
    justify-content: space-between;
    gap: var(--space-4);
    padding: var(--space-1-5) var(--space-3);
    border-top: 1px solid var(--border);
    color: var(--muted);
    font-size: var(--text-xs);
  }

  kbd {
    font-family: inherit;
    color: var(--text);
  }
</style>
