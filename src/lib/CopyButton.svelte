<script lang="ts">
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
  onclick={copy}>{copied ? "✓" : failed ? "!" : "⧉"}</button
>

<style>
  .copy {
    flex: none;
    width: 1.8rem;
    padding: 0.2rem 0;
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
