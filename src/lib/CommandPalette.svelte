<script lang="ts">
  import { tick } from "svelte";
  import RoleIcon from "./RoleIcon.svelte";
  import {
    fieldLabel,
    hostRole,
    inventory,
    revealHost,
  } from "./inventory.svelte";
  import {
    findHost,
    parentPath,
    searchHosts,
    type Host,
    type SearchHit,
    type SearchResults,
  } from "./workspace";

  interface Props {
    onconnect: (host: Host) => void;
    onclose: () => void;
  }

  let { onconnect, onclose }: Props = $props();

  /** The most hits shown at once. */
  const LIMIT = 50;

  let query = $state("");
  let results = $state<SearchResults | null>(null);
  /** The query `results` answer, which lags while the core answers. */
  let answered: string | null = null;
  let active = $state(0);
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
    searchHosts(typed, LIMIT)
      .then((found) => {
        if (stale) return;
        results = found;
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

  async function move(by: number) {
    const count = results?.hits.length ?? 0;
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

  async function onkeydown(event: KeyboardEvent) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      move(1);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      move(-1);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const reveal = event.shiftKey;
      const hit = await chosen();
      if (!hit) return;
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
      placeholder="Search hosts by name, address, tag or field"
      bind:value={query}
      {onkeydown}
      autocomplete="off"
      spellcheck="false"
      role="combobox"
      aria-expanded="true"
      aria-controls="palette-results"
      aria-activedescendant={results?.hits.length
        ? `palette-hit-${active}`
        : undefined}
    />
    <ul id="palette-results" role="listbox" bind:this={list}>
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
    </ul>
    {#if results && results.total === 0}
      <p class="empty">No host matches.</p>
    {/if}
    <footer>
      <span>
        {#if results && results.total > results.hits.length}
          {results.hits.length} of {results.total} hosts
        {:else if results}
          {results.total === 1 ? "1 host" : `${results.total} hosts`}
        {/if}
      </span>
      <span class="keys">
        <kbd>Enter</kbd> connect · <kbd>Shift+Enter</kbd> show in tree ·
        <kbd>Esc</kbd> close
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
    padding: 12vh 1rem 1rem;
    background: rgb(0 0 0 / 0.35);
  }

  .palette {
    width: min(38rem, 100%);
    max-height: 70vh;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.45);
    overflow: hidden;
  }

  input {
    margin: 0.6rem;
    padding: 0.45rem 0.6rem;
    font-size: 0.95rem;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0 0 0.3rem;
    overflow: auto;
    min-height: 0;
  }

  li {
    display: flex;
    align-items: baseline;
    gap: 0.5rem;
    padding: 0.3rem 0.8rem;
    font-size: 0.85rem;
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
    font-weight: 600;
  }

  .address,
  .detail,
  .folder {
    color: var(--muted);
    font-size: 0.78rem;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .folder {
    margin-left: auto;
    padding-left: 0.5rem;
  }

  .empty {
    margin: 0 0.8rem 0.6rem;
    color: var(--muted);
    font-size: 0.85rem;
  }

  footer {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.35rem 0.8rem;
    border-top: 1px solid var(--border);
    color: var(--muted);
    font-size: 0.75rem;
  }

  kbd {
    font-family: inherit;
    color: var(--text);
  }
</style>
