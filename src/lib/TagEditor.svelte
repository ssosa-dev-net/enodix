<script lang="ts">
  import X from "@lucide/svelte/icons/x";
  import IconButton from "./IconButton.svelte";
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
          <IconButton
            icon={X}
            size="compact"
            label="Remove tag {tag}"
            onclick={() =>
              onchange((current) => current.filter((t) => t !== tag))}
          />
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
    <button type="submit" disabled={newTag.trim() === ""}>Add</button>
  </form>
</section>

<style>
  section {
    display: grid;
    gap: var(--space-1-5);
  }

  h3 {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 0 0 var(--space-1-5);
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    color: var(--text);
  }

  .row {
    display: flex;
    align-items: center;
    gap: var(--space-1);
  }

  .row input {
    flex: 1;
    width: 100%;
  }

  .tags {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1);
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .tag {
    display: flex;
    align-items: center;
    padding: 0 0 0 var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-full);
    background: var(--surface);
  }

  .tag.inherited {
    padding-right: var(--space-2);
    border-style: dashed;
    background: none;
    color: var(--muted);
  }
</style>
