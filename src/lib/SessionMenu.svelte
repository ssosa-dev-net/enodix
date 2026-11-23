<script lang="ts" module>
  /** One of a session's actions, as its header's menu and its tab's
   * context menu offer them (#67). */
  export interface SessionAction {
    /** Its `data-session-action`. */
    id: string;
    label: string;
    /** The key that does the same, shown beside the label. */
    shortcut?: string;
    /** More about it, as the entry's tooltip. */
    title?: string;
    disabled?: boolean;
    /** Extra attributes for the entry (markers the smoke test uses). */
    attributes?: Record<string, string>;
    run: () => void;
  }

  /** A way the window offers to move a session to another window (#84,
   * #85), shown as one of its actions. */
  export interface SessionMove extends Omit<SessionAction, "disabled"> {
    /** What it does, as the entry's tooltip while it is allowed (its id
     * is also a `data-<id>` marker). */
    title: string;
    /** Why the window cannot make it now, beyond the session's own
     * reasons; null: it can. */
    refusal?: () => string | null;
  }
</script>

<script lang="ts">
  import { tick } from "svelte";

  interface Props {
    entries: SessionAction[];
    /** Where it opens: its top left corner, kept inside the window. */
    x: number;
    y: number;
    /** The control that opens it, whose own click toggles it. */
    opener?: Element | null;
    onclose: () => void;
  }

  let { entries, x, y, opener = null, onclose }: Props = $props();

  let menu = $state<HTMLDivElement>();
  let left = $state(0);
  let top = $state(0);

  // Inside the window, whatever the size of the menu.
  $effect(() => {
    left = x;
    top = y;
    void tick().then(() => {
      if (!menu) return;
      const box = menu.getBoundingClientRect();
      left = Math.max(4, Math.min(x, window.innerWidth - box.width - 4));
      top = Math.max(4, Math.min(y, window.innerHeight - box.height - 4));
      menu.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
    });
  });
</script>

<svelte:window
  onpointerdown={(event) => {
    const target = event.target as Element;
    if (target.closest("[data-session-actions]")) return;
    if (opener?.contains(target)) return;
    onclose();
  }}
  onkeydown={(event) => {
    if (event.key === "Escape") {
      event.preventDefault();
      onclose();
    }
  }}
  onblur={onclose}
/>

<div
  class="menu"
  role="menu"
  data-session-actions
  bind:this={menu}
  style:left="{left}px"
  style:top="{top}px"
>
  {#each entries as entry (entry.id)}
    <button
      role="menuitem"
      data-session-action={entry.id}
      disabled={entry.disabled}
      title={entry.title}
      {...entry.attributes}
      onclick={() => {
        onclose();
        entry.run();
      }}
    >
      <span>{entry.label}</span>
      {#if entry.shortcut}<kbd>{entry.shortcut}</kbd>{/if}
    </button>
  {/each}
</div>

<style>
  .menu {
    position: fixed;
    z-index: 25;
    display: grid;
    min-width: 12rem;
    padding: var(--space-1);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow);
    font-size: var(--text-sm);
  }

  button {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-4);
    border: 0;
    border-radius: var(--radius-sm);
    background: none;
    text-align: left;
    min-height: 0;
    padding: var(--space-1-5) var(--space-2-5);
  }

  button:hover:not(:disabled),
  button:focus-visible {
    background: var(--selected);
  }

  kbd {
    color: var(--muted);
    font-family: var(--font-ui);
    font-size: var(--text-xs);
  }
</style>
