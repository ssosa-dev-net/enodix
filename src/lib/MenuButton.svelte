<script lang="ts" module>
  import { untrack, type Component } from "svelte";

  /** One entry of a `MenuButton`'s menu. */
  export interface MenuItem {
    label: string;
    /** A lucide icon, shown before the label. */
    icon?: Component<{ size?: number; "aria-hidden"?: "true" }>;
    /** More about it, as the entry's tooltip (or why it is disabled). */
    title?: string;
    /** Shown but not offered: it keeps its place and is announced as
     * disabled. */
    disabled?: boolean;
    /** Drawn as a destructive action. */
    danger?: boolean;
    /** Extra attributes for the entry (the `data-*` markers the smoke
     * test drives). */
    attributes?: Record<string, string | undefined>;
    run: () => void;
  }

  /** A line between groups of entries. */
  export interface MenuSeparator {
    separator: true;
  }

  export type MenuEntry = MenuItem | MenuSeparator;
</script>

<script lang="ts">
  // A button that opens a menu of actions (#10): `aria-haspopup="menu"`,
  // `aria-expanded`, a `role="menu"` list of `role="menuitem"` buttons.
  // Enter, Space or the Down/Up arrow on the button open it with the first
  // (or last) entry focused; the arrows, Home and End move; Escape closes
  // it and returns to the button; Tab closes it; a press outside closes it.
  //
  // The list is a popover (the top layer), so that no clipping or scrolling
  // ancestor (the details sheet's body) cuts it, and it stays where it is in
  // the document (its entries are found below the button).
  import ChevronDown from "@lucide/svelte/icons/chevron-down";
  import Button from "./Button.svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";

  interface Props extends Omit<HTMLButtonAttributes, "children"> {
    /** The button's text. */
    label: string;
    entries: MenuEntry[];
    variant?: "primary" | "secondary" | "ghost" | "danger";
    size?: "default" | "compact";
  }

  let {
    label,
    entries,
    variant = "secondary",
    size = "compact",
    ...rest
  }: Props = $props();

  const id = $props.id();

  let open = $state(false);
  /** Which entry the list focuses when it opens. */
  let opening: "first" | "last" = "first";
  let trigger = $state<HTMLButtonElement | null>(null);

  /** The entries the arrow keys move through (disabled ones included). */
  function items(menu: HTMLElement): HTMLButtonElement[] {
    return [...menu.querySelectorAll<HTMLButtonElement>("[role=menuitem]")];
  }

  /** Shows the list below the button (above if it does not fit, inside the
   * window either way) and focuses the first (or last) entry. */
  function show(node: HTMLElement) {
    // Once per opening: a change to the button must not show it again.
    untrack(() => place(node));
  }

  function place(node: HTMLElement) {
    try {
      node.showPopover();
    } catch {
      // No popover support: the list is still placed (position: fixed).
      node.removeAttribute("popover");
    }
    if (trigger) {
      const from = trigger.getBoundingClientRect();
      const box = node.getBoundingClientRect();
      const margin = 4;
      const left = Math.max(
        margin,
        Math.min(from.left, window.innerWidth - box.width - margin),
      );
      let top = from.bottom + 2;
      if (top + box.height > window.innerHeight - margin) {
        top = Math.max(margin, from.top - box.height - 2);
      }
      node.style.left = `${left}px`;
      node.style.top = `${top}px`;
    }
    const all = items(node);
    const offered = all.filter(
      (item) => item.getAttribute("aria-disabled") !== "true",
    );
    const pool = offered.length > 0 ? offered : all;
    (opening === "last" ? pool.at(-1) : pool[0])?.focus();
  }

  function openMenu(at: "first" | "last") {
    opening = at;
    open = true;
  }

  /** Closes the list; `refocus` returns the keys to the button. */
  function close(refocus = false) {
    if (!open) return;
    open = false;
    if (refocus) trigger?.focus();
  }

  function ontriggerkeydown(event: KeyboardEvent) {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      openMenu(event.key === "ArrowDown" ? "first" : "last");
    }
  }

  function onmenukeydown(event: KeyboardEvent) {
    const menu = event.currentTarget as HTMLElement;
    const all = items(menu);
    const at = all.indexOf(document.activeElement as HTMLButtonElement);
    let next: number | null = null;
    switch (event.key) {
      case "ArrowDown":
        next = (at + 1) % all.length;
        break;
      case "ArrowUp":
        next = (at <= 0 ? all.length : at) - 1;
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = all.length - 1;
        break;
      case "Escape":
        // Closes the menu only: not the sheet or the dialog around it.
        event.preventDefault();
        event.stopPropagation();
        close(true);
        return;
      case "Tab":
        // The keys go on to the next control as usual.
        close();
        return;
      default:
        return;
    }
    event.preventDefault();
    all[next]?.focus();
  }

  // While it is open: a press outside, another window, a scrolled page or a
  // resized window closes it (it is placed by the button's position).
  $effect(() => {
    if (!open) return;
    const pressed = (event: PointerEvent) => {
      const target = event.target as Node;
      if (trigger?.contains(target)) return;
      if (document.getElementById(id)?.contains(target)) return;
      close();
    };
    const scrolled = (event: Event) => {
      if (!document.getElementById(id)?.contains(event.target as Node)) close();
    };
    const away = () => close();
    window.addEventListener("pointerdown", pressed, true);
    window.addEventListener("scroll", scrolled, true);
    window.addEventListener("resize", away);
    window.addEventListener("blur", away);
    return () => {
      window.removeEventListener("pointerdown", pressed, true);
      window.removeEventListener("scroll", scrolled, true);
      window.removeEventListener("resize", away);
      window.removeEventListener("blur", away);
    };
  });

  function choose(item: MenuItem) {
    if (item.disabled) return;
    // The keys are back on the button before the action runs, which may
    // move them on (a dialog, a tab).
    trigger?.focus();
    open = false;
    item.run();
  }
</script>

<Button
  bind:element={trigger}
  {variant}
  {size}
  {...rest}
  aria-haspopup="menu"
  aria-expanded={open}
  aria-controls={open ? id : undefined}
  onclick={() => (open ? close(true) : openMenu("first"))}
  onkeydown={ontriggerkeydown}
>
  <span class="label">{label}</span>
  <ChevronDown size={12} aria-hidden="true" />
</Button>
{#if open}
  <!-- svelte-ignore a11y_interactive_supports_focus -->
  <div
    {id}
    class="menu"
    role="menu"
    aria-label={label}
    popover="manual"
    onkeydown={onmenukeydown}
    {@attach show}
  >
    {#each entries as entry, index (index)}
      {#if "separator" in entry}
        <div class="separator" role="separator"></div>
      {:else}
        {@const Icon = entry.icon}
        <button
          type="button"
          class="item"
          class:danger={entry.danger}
          role="menuitem"
          tabindex="-1"
          aria-disabled={entry.disabled ? "true" : undefined}
          title={entry.title}
          {...entry.attributes}
          onclick={() => choose(entry)}
        >
          {#if Icon}<Icon size={14} aria-hidden="true" />{/if}
          <span>{entry.label}</span>
        </button>
      {/if}
    {/each}
  </div>
{/if}

<style>
  :global(.btn[aria-haspopup="menu"]) {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
  }

  /* A popover brings its own box (centred, bordered, padded): replaced. */
  .menu {
    position: fixed;
    inset: auto;
    width: max-content;
    height: max-content;
    max-height: calc(100vh - 8px);
    margin: 0;
    display: grid;
    align-content: start;
    min-width: 10rem;
    max-width: calc(100vw - 8px);
    padding: var(--space-1);
    overflow: visible;
    color: var(--text);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow);
    font-size: var(--text-sm);
  }

  .item {
    display: flex;
    align-items: center;
    justify-content: flex-start;
    gap: var(--space-2);
    min-height: 0;
    padding: var(--space-1-5) var(--space-2-5);
    border: 0;
    border-radius: var(--radius-sm);
    background: none;
    color: inherit;
    text-align: left;
    white-space: nowrap;
  }

  .item:hover,
  .item:focus-visible {
    background: var(--selected);
  }

  .item:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }

  .item.danger {
    color: var(--danger);
  }

  .item[aria-disabled="true"] {
    color: var(--muted);
    cursor: default;
  }

  .item[aria-disabled="true"]:hover {
    background: none;
  }

  .separator {
    height: 1px;
    margin: var(--space-1) 0;
    background: var(--border);
  }
</style>
