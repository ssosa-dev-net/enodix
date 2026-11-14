<script lang="ts">
  import CopyButton from "./CopyButton.svelte";
  import { describeOrigin, isOwn, type Item } from "./inventory.svelte";
  import type { Effective } from "./workspace";

  interface Props {
    /** The tags the host or folder sets itself. */
    tags: string[];
    /** All of its tags, with where each comes from (null while loading). */
    effective: Effective<string>[] | null;
    item: Item;
    onchange: (change: (current: string[]) => string[]) => void;
  }

  let { tags, effective, item, onchange }: Props = $props();

  let newTag = $state("");

  /** Tags from the folders above that the item does not set itself. */
  const inherited = $derived(
    (effective ?? []).filter(
      (tag) => !isOwn(tag.origin, item) && !tags.includes(tag.value),
    ),
  );

  function add(event: SubmitEvent) {
    event.preventDefault();
    const tag = newTag;
    if (tag.trim() === "") return;
    onchange((current) => [...current, tag]);
    newTag = "";
  }
</script>

<section>
  <h3>
    Tags
    <CopyButton
      value={[...inherited.map((t) => t.value), ...tags].join(", ")}
      what="tags"
    />
  </h3>
  {#if tags.length > 0 || inherited.length > 0}
    <ul class="tags">
      {#each inherited as tag (tag.value)}
        <li
          class="tag inherited"
          data-inherited-tag={tag.value}
          title={describeOrigin(tag.origin, item)}
        >
          {tag.value}
        </li>
      {/each}
      {#each tags as tag (tag)}
        <li class="tag">
          {tag}
          <button
            class="remove"
            aria-label="Remove tag {tag}"
            onclick={() =>
              onchange((current) => current.filter((t) => t !== tag))}>×</button
          >
        </li>
      {/each}
    </ul>
  {/if}
  <form class="row" onsubmit={add}>
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

<style>
  section {
    display: grid;
    gap: 0.45rem;
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

  .row {
    display: flex;
    align-items: center;
    gap: 0.25rem;
  }

  .row input {
    flex: 1;
    width: 100%;
  }

  .small {
    padding: 0.2rem 0.6rem;
    font-size: 0.8rem;
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

  .tag.inherited {
    padding-right: 0.5rem;
    border-style: dashed;
    background: none;
    color: var(--muted);
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
</style>
