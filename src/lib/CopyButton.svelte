<script lang="ts">
  import Check from "@lucide/svelte/icons/check";
  import Copy from "@lucide/svelte/icons/copy";
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert";
  import IconButton from "./IconButton.svelte";
  interface Props {
    value: string;
    /** What is copied, for the button's label. */
    what: string;
    /** The small size, for a dense row; beside an input it is the default. */
    compact?: boolean;
  }

  let { value, what, compact = false }: Props = $props();

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

<IconButton
  icon={copied ? Check : failed ? TriangleAlert : Copy}
  size={compact ? "compact" : "default"}
  class={copied ? "copied" : ""}
  title={failed ? "Could not copy" : `Copy ${what}`}
  label="Copy {what}"
  disabled={value === ""}
  onclick={copy}
/>

<style>
  :global(.btn.icon.copied) {
    color: var(--ok);
  }
</style>
