<script lang="ts">
  import LockKeyhole from "@lucide/svelte/icons/lock-keyhole";
  import TriangleAlert from "@lucide/svelte/icons/triangle-exclamation-point";
  import {
    quitApp,
    startVaultOver,
    unlockVault,
    vaultFailure,
    type VaultFailure,
  } from "./vault";
  import WindowControls from "./WindowControls.svelte";

  interface Props {
    /** The app's name, for the title. */
    name: string;
    onunlocked: () => void;
    /** The vault was deleted: the app goes on with the keychain. */
    onstartedover: () => void;
  }

  let { name, onunlocked, onstartedover }: Props = $props();

  let password = $state("");
  let failure = $state<VaultFailure | null>(null);
  let busy = $state(false);
  let input = $state<HTMLInputElement>();
  /** Asking whether to delete the vault (a forgotten password). */
  let startingOver = $state(false);
  let keep = $state<HTMLButtonElement>();

  $effect(() => {
    if (startingOver) keep?.focus();
    else input?.focus();
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy || password === "") return;
    busy = true;
    try {
      await unlockVault(password);
      password = "";
      onunlocked();
    } catch (error) {
      failure = vaultFailure(error);
      password = "";
      input?.focus();
    } finally {
      busy = false;
    }
  }

  async function startOver() {
    if (busy) return;
    busy = true;
    try {
      await startVaultOver();
      onstartedover();
    } catch (error) {
      failure = vaultFailure(error);
      startingOver = false;
    } finally {
      busy = false;
    }
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && startingOver && !busy) {
      event.stopPropagation();
      startingOver = false;
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<div
  class="screen"
  role="dialog"
  aria-modal="true"
  aria-labelledby="unlock-title"
  tabindex="-1"
  data-unlock-screen
  {onkeydown}
>
  <!-- The screen covers the window's top bar: a strip of its own moves
       the window and holds its buttons (#80). -->
  <div class="frame" data-window-bar data-tauri-drag-region>
    <WindowControls />
  </div>
  {#if startingOver}
    <div class="panel" data-start-over>
      <div class="mark danger-mark" data-unlock-mark>
        <TriangleAlert size={22} aria-hidden="true" />
      </div>
      <h1 id="unlock-title">Start over?</h1>
      <p data-start-over-warning>
        Starting over deletes the vault. All stored credential profiles and
        secrets, of every workspace, will be gone for good; nobody can recover
        them. The workspaces themselves are not touched, and {name} goes on with the
        system's keychain (where there is none, with no stored credentials until the
        vault is turned on again).
      </p>
      <div class="actions">
        <button
          type="button"
          bind:this={keep}
          data-start-over="cancel"
          disabled={busy}
          onclick={() => (startingOver = false)}>Keep the vault</button
        >
        <button
          type="button"
          class="danger"
          data-start-over="confirm"
          disabled={busy}
          onclick={() => void startOver()}
          >{busy ? "Deleting…" : "Delete everything and start over"}</button
        >
      </div>
    </div>
  {:else}
    <form class="panel" onsubmit={submit}>
      <div class="mark" data-unlock-mark>
        <LockKeyhole size={22} aria-hidden="true" />
      </div>
      <h1 id="unlock-title">{name}</h1>
      <p class="lead">
        Your credential profiles and secrets are in the vault. Unlock it to open
        your workspace.
      </p>
      <label>
        Master password
        <input
          bind:this={input}
          bind:value={password}
          type="password"
          name="vault-unlock-password"
          autocomplete="off"
          disabled={busy}
        />
      </label>
      {#if failure}
        <p class="error" role="alert" data-unlock-error={failure.kind}>
          {#if failure.kind === "wrongPassword"}
            That is not the master password. Try again.
          {:else if failure.kind === "damaged"}
            The vault cannot be opened with any password: {failure.message}.
          {:else}
            {failure.message}
          {/if}
        </p>
      {/if}
      <div class="actions">
        <button
          type="button"
          class="forgot"
          data-unlock="start-over"
          disabled={busy}
          onclick={() => {
            failure = null;
            startingOver = true;
          }}>Forgot it? Start over…</button
        >
        <button type="button" data-unlock="quit" onclick={() => void quitApp()}
          >Quit</button
        >
        <button
          type="submit"
          class="primary"
          data-unlock="unlock"
          disabled={busy || password === ""}
          >{busy ? "Unlocking…" : "Unlock"}</button
        >
      </div>
    </form>
  {/if}
</div>

<style>
  .screen {
    position: fixed;
    inset: 0;
    z-index: 100;
    display: grid;
    place-items: center;
    padding: var(--space-4);
    background: var(--bg);
  }

  .frame {
    position: absolute;
    top: 0;
    left: 0;
    right: 0;
    height: 38px;
    display: flex;
    justify-content: flex-end;
  }

  .panel {
    width: min(26rem, 100%);
    display: grid;
    gap: var(--space-4);
    padding: var(--space-8) var(--space-6) var(--space-6);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
  }

  .mark {
    width: 44px;
    height: 44px;
    display: grid;
    place-items: center;
    border-radius: var(--radius-full);
    color: var(--accent);
    background: var(--surface-raised);
    border: 1px solid var(--border-strong);
  }

  [data-start-over-warning] {
    color: var(--text);
  }

  .danger-mark {
    color: var(--danger);
  }

  h1 {
    margin: 0;
    font-size: var(--text-xl);
    font-weight: var(--weight-semibold);
  }

  p {
    margin: 0;
    font-size: var(--text-sm);
    color: var(--muted);
  }

  label {
    display: grid;
    gap: var(--space-1);
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .panel .error {
    color: var(--danger);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--space-2);
  }

  .forgot {
    margin-right: auto;
    background: transparent;
    border-color: transparent;
    color: var(--muted);
    padding-inline: 0;
  }

  .forgot:hover:not(:disabled) {
    color: var(--text);
    background: transparent;
  }

  .danger {
    color: var(--danger);
    border-color: var(--danger);
  }
</style>
