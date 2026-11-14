<script lang="ts">
  import { inventory, removeSnippet } from "./inventory.svelte";
  import { bySnippetGroup, editSnippet, newSnippet } from "./snippets.svelte";

  interface Props {
    /** The session snippets go to (the active pane's), by name; null: none. */
    target: string | null;
    onsend: (name: string) => void;
  }

  let { target, onsend }: Props = $props();

  let open = $state(true);
  const list = $derived(inventory.workspace?.snippets ?? []);
  const groups = $derived(bySnippetGroup(list));
</script>

<section class="snippets" class:open data-snippets>
  <header>
    <button
      class="toggle"
      aria-expanded={open}
      data-snippets-toggle
      onclick={() => (open = !open)}
    >
      <span class="caret" aria-hidden="true">{open ? "▾" : "▸"}</span>
      Snippets
      {#if list.length > 0}<span class="count">{list.length}</span>{/if}
    </button>
    <button
      class="new"
      data-snippet-new
      title="New snippet, saved in the workspace"
      onclick={() => newSnippet()}>+ Snippet</button
    >
  </header>
  {#if open}
    <div class="body">
      {#if list.length === 0}
        <p class="hint">
          Commands you send often, saved in the workspace for the team. Use
          <code>{"{{address}}"}</code> and other host fields as variables.
        </p>
      {:else}
        {#if target === null}
          <p class="hint">Open a session to send snippets to it.</p>
        {/if}
        {#each groups as { group, snippets } (group)}
          <div class="group" data-snippet-group={group ?? ""}>
            {#if group !== null}<h3>{group}</h3>{/if}
            <ul>
              {#each snippets as snippet (snippet.name)}
                <li>
                  <button
                    class="send"
                    data-snippet={snippet.name}
                    disabled={target === null}
                    title={target === null
                      ? snippet.text
                      : `Send to ${target}:\n${snippet.text}`}
                    onclick={() => onsend(snippet.name)}
                  >
                    {snippet.name}
                  </button>
                  <button
                    class="small"
                    data-snippet-edit={snippet.name}
                    title="Edit"
                    aria-label={`Edit ${snippet.name}`}
                    onclick={() => editSnippet(snippet)}>Edit</button
                  >
                  <button
                    class="small"
                    data-snippet-delete={snippet.name}
                    title="Delete"
                    aria-label={`Delete ${snippet.name}`}
                    onclick={() => removeSnippet(snippet.name)}>×</button
                  >
                </li>
              {/each}
            </ul>
          </div>
        {/each}
      {/if}
    </div>
  {/if}
</section>

<style>
  .snippets {
    flex: none;
    display: flex;
    flex-direction: column;
    max-height: 40%;
    border-top: 1px solid var(--border);
  }

  header {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.3rem 0.6rem;
    background: var(--surface);
  }

  .toggle {
    flex: 1;
    display: flex;
    align-items: center;
    gap: 0.35rem;
    border-color: transparent;
    background: none;
    padding: 0.1rem 0.2rem;
    font-weight: 600;
    text-align: left;
  }

  .caret {
    width: 0.8rem;
    color: var(--muted);
  }

  .count {
    color: var(--muted);
    font-weight: normal;
    font-size: 0.75rem;
  }

  .new {
    padding: 0.15rem 0.55rem;
    font-size: 0.8rem;
  }

  .body {
    min-height: 0;
    overflow: auto;
    padding: 0.2rem 0 0.4rem;
  }

  .hint {
    margin: 0.4rem 0.6rem;
    color: var(--muted);
    font-size: 0.8rem;
  }

  h3 {
    margin: 0.4rem 0.6rem 0.1rem;
    color: var(--muted);
    font-size: 0.72rem;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  li {
    display: flex;
    align-items: center;
    gap: 0.2rem;
    padding: 0 0.6rem 0 0.4rem;
  }

  .send {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    text-align: left;
    border-color: transparent;
    background: none;
    padding: 0.2rem 0.4rem;
  }

  .send:not(:disabled):hover {
    filter: none;
    background: var(--surface);
  }

  .send:disabled {
    opacity: 0.75;
  }

  .small {
    flex: none;
    padding: 0.05rem 0.35rem;
    font-size: 0.72rem;
    border-color: transparent;
    background: none;
    color: var(--muted);
    visibility: hidden;
  }

  li:hover .small,
  li:focus-within .small {
    visibility: visible;
  }
</style>
