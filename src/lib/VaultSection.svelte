<script lang="ts">
  // The Preferences dialog's Security section: whether credentials are in
  // the keychain or the vault, and turning the vault on.
  import { refreshProfiles } from "./profiles.svelte";
  import {
    turnOnVault,
    vaultFailure,
    vaultStatus,
    type VaultStatus,
  } from "./vault";

  let status = $state<VaultStatus | null>(null);
  let turningOn = $state(false);
  let password = $state("");
  let repeated = $state("");
  let error = $state<string | null>(null);
  let busy = $state(false);
  let first = $state<HTMLInputElement>();

  vaultStatus().then(
    (value) => (status = value),
    (failure) => (error = String(failure)),
  );

  $effect(() => {
    if (turningOn) first?.focus();
  });

  const differ = $derived(repeated !== "" && password !== repeated);

  function cancel() {
    turningOn = false;
    password = "";
    repeated = "";
    error = null;
  }

  async function confirm(event: SubmitEvent) {
    event.preventDefault();
    if (busy || password === "" || password !== repeated) return;
    busy = true;
    error = null;
    try {
      status = await turnOnVault(password, repeated);
      cancel();
      void refreshProfiles();
    } catch (failure) {
      error = vaultFailure(failure).message;
    } finally {
      busy = false;
    }
  }

  // Escape closes the whole dialog; keep it from doing so mid-form.
  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && turningOn) {
      event.stopPropagation();
      cancel();
    }
  }
</script>

<section class="security" data-vault-status={status}>
  <h3>Security</h3>
  {#if status === "off"}
    <p>
      Credential profiles and secrets are kept in the system's keychain. A vault
      keeps them in an encrypted file instead, behind a master password asked
      for once at every start.
    </p>
    {#if !turningOn}
      <div>
        <button
          type="button"
          data-vault="turn-on"
          onclick={() => (turningOn = true)}>Turn on the vault…</button
        >
      </div>
    {/if}
  {:else if status === "unlocked" || status === "locked"}
    <p>
      Credential profiles and secrets of every workspace are kept in the vault,
      behind the master password.
    </p>
  {/if}

  {#if turningOn}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <form onsubmit={confirm} {onkeydown} data-vault-form>
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
        <p class="note" data-vault-differ>
          The two passwords are not the same.
        </p>
      {/if}
      <p class="warning" data-vault-warning>
        If you forget the master password, nobody can recover it: the profiles
        and secrets in the vault are lost. Every workspace's profiles and
        secrets move out of the keychain into the vault.
      </p>
      <div class="actions">
        <button type="button" data-vault="cancel" onclick={cancel}
          >Cancel</button
        >
        <button
          type="submit"
          class="primary"
          data-vault="confirm"
          disabled={busy || password === "" || password !== repeated}
          >{busy ? "Moving…" : "Turn on"}</button
        >
      </div>
    </form>
  {/if}

  {#if error}
    <p class="error" role="alert" data-vault-error>{error}</p>
  {/if}
</section>

<style>
  .security {
    display: grid;
    gap: 0.5rem;
    padding-top: 0.6rem;
    border-top: 1px solid var(--border);
    font-size: 0.85rem;
  }

  h3 {
    margin: 0;
    font-size: 0.9rem;
    font-weight: 600;
  }

  p {
    margin: 0;
  }

  form {
    display: grid;
    gap: 0.5rem;
  }

  label {
    display: grid;
    gap: 0.25rem;
  }

  .warning {
    padding: 0.5rem 0.6rem;
    border: 1px solid var(--danger);
    border-radius: 4px;
  }

  .note {
    color: var(--muted);
  }

  .error {
    color: var(--danger);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
  }
</style>
