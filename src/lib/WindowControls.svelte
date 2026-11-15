<script lang="ts">
  // The window's own minimize, maximize or restore, and close buttons
  // (#80): the app draws its frame, so these sit at the right end of the
  // window's top bar. Close is the window's own close, so it does exactly
  // what closing the window did with the system frame.
  import Copy from "@lucide/svelte/icons/copy";
  import Minus from "@lucide/svelte/icons/minus";
  import Square from "@lucide/svelte/icons/square";
  import X from "@lucide/svelte/icons/x";
  import { getCurrentWindow } from "@tauri-apps/api/window";

  const current = getCurrentWindow();

  let maximized = $state(false);

  $effect(() => {
    const refresh = () =>
      void current.isMaximized().then((value) => (maximized = value));
    refresh();
    // Maximizing, restoring and snapping all resize the window.
    const unlisten = current.onResized(refresh);
    return () => void unlisten.then((stop) => stop());
  });
</script>

<div class="controls" data-window-controls>
  <button
    type="button"
    data-window-control="minimize"
    aria-label="Minimize"
    title="Minimize"
    onmousedown={(event) => event.preventDefault()}
    onclick={() => void current.minimize()}
    ><Minus size={16} aria-hidden="true" /></button
  >
  <button
    type="button"
    data-window-control="maximize"
    data-maximized={maximized || undefined}
    aria-label={maximized ? "Restore" : "Maximize"}
    title={maximized ? "Restore" : "Maximize"}
    onmousedown={(event) => event.preventDefault()}
    onclick={() => void current.toggleMaximize()}
  >
    {#if maximized}
      <Copy size={14} aria-hidden="true" />
    {:else}
      <Square size={13} aria-hidden="true" />
    {/if}
  </button>
  <button
    type="button"
    class="close"
    data-window-control="close"
    aria-label="Close"
    title="Close"
    onmousedown={(event) => event.preventDefault()}
    onclick={() => void current.close()}
    ><X size={17} aria-hidden="true" /></button
  >
</div>

<style>
  .controls {
    flex: none;
    align-self: stretch;
    display: flex;
  }

  button {
    display: grid;
    place-items: center;
    width: 42px;
    min-height: 0;
    height: 100%;
    padding: 0;
    border: 0;
    border-radius: 0;
    background: none;
    color: var(--muted);
  }

  button:hover:not(:disabled) {
    color: var(--text);
    background: var(--surface-raised);
  }

  button.close:hover:not(:disabled) {
    color: var(--on-danger);
    background: var(--danger-fill);
  }

  button:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
  }
</style>
