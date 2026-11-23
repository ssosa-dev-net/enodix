<script lang="ts">
  import type { Component } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";
  import Button from "./Button.svelte";

  interface Props extends Omit<HTMLButtonAttributes, "children"> {
    /** The accessible name, and the tooltip unless `title` is given. */
    label: string;
    /** A lucide icon component. */
    icon: Component<{ size?: number; "aria-hidden"?: "true" }>;
    variant?: "primary" | "secondary" | "ghost" | "danger";
    size?: "default" | "compact";
    pressed?: boolean;
    element?: HTMLButtonElement | null;
  }

  let {
    label,
    icon: Icon,
    variant = "ghost",
    size = "default",
    pressed,
    element = $bindable(null),
    title,
    class: className = "",
    ...rest
  }: Props = $props();
</script>

<Button
  bind:element
  {variant}
  {size}
  class="icon {className}"
  aria-label={label}
  aria-pressed={pressed}
  title={title ?? label}
  {...rest}
>
  <Icon size={size === "compact" ? 14 : 16} aria-hidden="true" />
</Button>

<style>
  :global(.btn.icon) {
    flex: none;
    width: var(--control-height);
    height: var(--control-height);
    min-width: 24px;
    min-height: 24px;
    padding: 0;
  }

  :global(.btn.icon[aria-pressed="true"]),
  :global(.btn.icon[aria-expanded="true"]) {
    color: var(--text);
    background: var(--surface-raised);
  }

  :global(.btn.icon.compact) {
    width: var(--control-height-small);
    height: var(--control-height-small);
    padding: 0;
  }
</style>
