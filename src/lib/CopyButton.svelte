<script lang="ts">
  import Check from "@lucide/svelte/icons/check";
  import Copy from "@lucide/svelte/icons/copy";
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert";
  interface Props {
    value: string;
    /** What is copied, for the button's label. */
    what: string;
  }

  let { value, what }: Props = $props();

  let copied = $state(false);
  let failed = $state(false);
  let timer: ReturnType<typeof setTimeout> | undefined;

  async function copy() {
    clearTimeout(timer);
    try {
      await navigator.clipboard.writeText(value);
      copied = true;
      failed = false;
    } catch {
      failed = true;
    }
    timer = setTimeout(() => {
      copied = false;
      failed = false;
    }, 1500);
  }
</script>

<button
  type="button"
  class="copy"
  class:copied
  title={failed ? "Could not copy" : `Copy ${what}`}
  aria-label="Copy {what}"
  disabled={value === ""}
  onclick={copy}
  >{#if copied}<Check
      size={14}
      aria-hidden="true"
    />{:else if failed}<TriangleAlert
      size={14}
      aria-hidden="true"
    />{:else}<Copy size={14} aria-hidden="true" />{/if}</button
>

<style>
  .copy {
    flex: none;
    display: inline-grid;
    place-items: center;
    width: 1.8rem;
    padding: var(--space-0-5) 0;
    color: var(--muted);
    background: none;
    border-color: transparent;
  }

  .copy:hover:not(:disabled) {
    color: var(--text);
    border-color: var(--border);
  }

  .copied {
    color: var(--ok);
  }
</style>
