<script lang="ts">
  // Turning the vault on: the master password typed twice and the warning
  // that it cannot be recovered. Used by the Preferences' Security section
  // and by a profile or secret saved where only the vault can keep it.
  import { turnOnVault, vaultFailure, type VaultStatus } from "./vault";

  interface Props {
    /** Whether the system has a keychain whose entries move into it. */
    keychain: boolean;
    /** Called once the vault is on, with the status it now has. */
    onturnedon: (status: VaultStatus) => void;
    oncancel: () => void;
  }

  let { keychain, onturnedon, oncancel }: Props = $props();

  let password = $state("");
  let repeated = $state("");
  let error = $state<string | null>(null);
  let busy = $state(false);
  let first = $state<HTMLInputElement>();

  $effect(() => {
    first?.focus();
  });

  const differ = $derived(repeated !== "" && password !== repeated);
  const ready = $derived(password !== "" && password === repeated);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy || !ready) return;
    busy = true;
    error = null;
    try {
      const status = await turnOnVault(password, repeated);
      password = "";
      repeated = "";
      onturnedon(status);
    } catch (failure) {
      error = vaultFailure(failure).message;
    } finally {
      busy = false;
    }
  }

  // Escape would close the whole dialog; it cancels this form instead.
  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.stopPropagation();
      oncancel();
    }
  }
</script>

<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
<form onsubmit={submit} {onkeydown} data-vault-form>
  <label>
    Master password
    <input
      bind:this={first}
      bind:value={password}
      type="password"
      name="vault-password"
      autocomplete="new-password"
      disabled={busy}
    />
  </label>
  <label>
    The same password again
    <input
      bind:value={repeated}
      type="password"
      name="vault-repeat"
      autocomplete="new-password"
      disabled={busy}
    />
  </label>
  {#if differ}
    <p class="note" data-vault-differ>The two passwords are not the same.</p>
  {/if}
  <p class="warning" data-vault-warning>
    If you forget the master password, nobody can recover it: the profiles and
    secrets in the vault are lost.
    {#if keychain}
      Every workspace's profiles and secrets move out of the keychain into the
      vault.
    {/if}
  </p>
  {#if error}
    <p class="error" role="alert" data-vault-error>{error}</p>
  {/if}
  <div class="actions">
    <button type="button" data-vault="cancel" onclick={oncancel}>Cancel</button>
    <button
      type="submit"
      class="primary"
      data-vault="confirm"
      disabled={busy || !ready}
      >{busy ? (keychain ? "Moving…" : "Turning on…") : "Turn on"}</button
    >
  </div>
</form>

<style>
  form {
    display: grid;
    gap: var(--space-2);
    font-size: var(--text-sm);
  }

  label {
    display: grid;
    gap: var(--space-1);
  }

  p {
    margin: 0;
  }

  .warning {
    padding: var(--space-2) var(--space-2);
    border: 1px solid var(--danger);
    border-radius: var(--radius-sm);
  }

  .note {
    color: var(--muted);
    line-height: 1.5;
  }

  .error {
    color: var(--danger);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
