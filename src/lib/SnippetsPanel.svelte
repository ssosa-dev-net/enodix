<script lang="ts">
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import ChevronRight from "@lucide/svelte/icons/chevron-right";
  import X from "@lucide/svelte/icons/x";
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
      <span class="caret" aria-hidden="true"
        >{#if open}<ChevronDown size={14} />{:else}<ChevronRight
            size={14}
          />{/if}</span
      >
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
                    onclick={() => removeSnippet(snippet.name)}
                    ><X size={13} aria-hidden="true" /></button
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
    gap: var(--space-1-5);
    padding: var(--space-1) var(--space-2);
    background: var(--surface);
  }

  .toggle {
    flex: 1;
    display: flex;
    align-items: center;
    gap: var(--space-1-5);
    border-color: transparent;
    background: none;
    padding: var(--space-0-5) var(--space-0-5);
    font-weight: var(--weight-semibold);
    text-align: left;
  }

  .caret {
    width: 0.8rem;
    color: var(--muted);
  }

  .count {
    color: var(--muted);
    font-weight: normal;
    font-size: var(--text-xs);
  }

  .new {
    padding: var(--space-0-5) var(--space-2);
    font-size: var(--text-xs);
  }

  .body {
    min-height: 0;
    overflow: auto;
    padding: var(--space-0-5) 0 var(--space-1-5);
  }

  .hint {
    margin: var(--space-1-5) var(--space-2);
    color: var(--muted);
    font-size: var(--text-xs);
  }

  h3 {
    margin: var(--space-1-5) var(--space-2) var(--space-0-5);
    color: var(--muted);
    font-size: var(--text-2xs);
    font-weight: var(--weight-semibold);
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
    gap: var(--space-0-5);
    padding: 0 var(--space-2) 0 var(--space-1-5);
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
    padding: var(--space-0-5) var(--space-1-5);
  }

  .send:not(:disabled):hover {
    background: var(--surface-raised);
  }

  .send:disabled {
    opacity: 0.75;
  }

  .small {
    flex: none;
    padding: var(--space-0-5) var(--space-1-5);
    font-size: var(--text-2xs);
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
