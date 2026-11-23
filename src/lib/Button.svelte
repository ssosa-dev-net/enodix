<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLButtonAttributes } from "svelte/elements";

  interface Props extends HTMLButtonAttributes {
    variant?: "primary" | "secondary" | "ghost" | "danger";
    size?: "default" | "compact";
    element?: HTMLButtonElement | null;
    children?: Snippet;
  }

  let {
    variant = "secondary",
    size = "default",
    element = $bindable(null),
    type = "button",
    class: className = "",
    children,
    ...rest
  }: Props = $props();
</script>

<button
  bind:this={element}
  {type}
  class="btn {variant} {size} {className}"
  {...rest}
>
  {@render children?.()}
</button>

<style>
  .btn {
    min-height: var(--control-height);
    padding: 0 var(--space-3);
    font-size: var(--text-sm);
    color: var(--text);
    background: transparent;
    border: 1px solid var(--control-border);
    border-radius: var(--radius-md);
  }

  .btn.compact {
    min-height: var(--control-height-small);
    padding: 0 var(--space-2);
    font-size: var(--text-xs);
  }

  .btn:hover:not(:disabled) {
    background: var(--surface-raised);
  }

  .btn.primary {
    color: var(--on-accent);
    background: var(--accent);
    border-color: var(--accent);
    font-weight: var(--weight-semibold);
    padding: 0 var(--space-4);
  }

  .btn.primary.compact {
    padding: 0 var(--space-3);
  }

  .btn.primary:hover:not(:disabled) {
    background: var(--accent);
    filter: brightness(1.1);
  }

  .btn.ghost {
    color: var(--muted);
    border-color: transparent;
  }

  .btn.ghost:hover:not(:disabled) {
    color: var(--text);
    background: var(--surface-raised);
  }

  .btn.danger {
    color: var(--danger);
    border-color: var(--danger);
  }

  .btn.danger:hover:not(:disabled) {
    color: var(--on-danger);
    background: var(--danger-fill);
    border-color: var(--danger-fill);
  }

  .btn:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 1px;
  }
</style>
