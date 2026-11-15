<script lang="ts">
  // The Preferences dialog's Security section: whether credentials are in
  // the keychain or the vault; turning the vault on, changing its master
  // password and turning it off. Without a keychain (Linux) the vault is
  // the only store: it has to be on to keep anything, and cannot be turned
  // off.
  import { refreshProfiles } from "./profiles.svelte";
  import TurnOnVaultForm from "./TurnOnVaultForm.svelte";
  import {
    changeVaultPassword,
    turnOffVault,
    vaultFailure,
    vaultKeychain,
    vaultStatus,
    type VaultStatus,
  } from "./vault";

  /** The form shown: turning on, changing the password, turning off. */
  type Mode = "on" | "change" | "off";

  let status = $state<VaultStatus | null>(null);
  /** Whether the system has a keychain (assumed until known). */
  let keychain = $state(true);
  let mode = $state<Mode | null>(null);
  let current = $state("");
  let password = $state("");
  let repeated = $state("");
  let error = $state<string | null>(null);
  /** What was just done, said once. */
  let done = $state<string | null>(null);
  let busy = $state(false);
  let first = $state<HTMLInputElement>();

  vaultStatus().then(
    (value) => (status = value),
    (failure) => (error = String(failure)),
  );
  vaultKeychain().then(
    (value) => (keychain = value),
    (failure) => (error = String(failure)),
  );

  function turnedOn(next: VaultStatus) {
    status = next;
    cancel();
    void refreshProfiles();
  }

  $effect(() => {
    if (mode) first?.focus();
  });

  const differ = $derived(repeated !== "" && password !== repeated);
  const newPasswordReady = $derived(password !== "" && password === repeated);

  function open(next: Mode) {
    cancel();
    done = null;
    mode = next;
  }

  function cancel() {
    mode = null;
    current = "";
    password = "";
    repeated = "";
    error = null;
  }

  /** Runs `action` for the open form; `message` is said once it worked. */
  async function run(
    event: SubmitEvent,
    ready: boolean,
    action: () => Promise<VaultStatus | void>,
    message: string | null,
  ) {
    event.preventDefault();
    if (busy || !ready) return;
    busy = true;
    error = null;
    try {
      const next = await action();
      if (next) status = next;
      cancel();
      done = message;
      void refreshProfiles();
    } catch (failure) {
      const reason = vaultFailure(failure);
      error =
        reason.kind === "wrongPassword"
          ? "That is not the master password."
          : reason.message;
    } finally {
      busy = false;
    }
  }

  // Escape closes the whole dialog; keep it from doing so mid-form.
  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && mode) {
      event.stopPropagation();
      cancel();
    }
  }
</script>

<section
  class="security"
  data-vault-status={status}
  data-vault-keychain={keychain}
>
  <h3>Security</h3>
  {#if status === "off" && !keychain}
    <p data-vault-no-keychain>
      This system has no keychain the app can use, so credential profiles and
      secrets can only be kept in the vault: an encrypted file behind a master
      password asked for once at every start. Turn it on to store any.
    </p>
  {:else if status === "off"}
    <p>
      Credential profiles and secrets are kept in the system's keychain. A vault
      keeps them in an encrypted file instead, behind a master password asked
      for once at every start.
    </p>
  {/if}
  {#if status === "off"}
    {#if !mode}
      <div>
        <button type="button" data-vault="turn-on" onclick={() => open("on")}
          >Turn on the vault…</button
        >
      </div>
    {/if}
  {:else if status === "unlocked" || status === "locked"}
    <p>
      Credential profiles and secrets of every workspace are kept in the vault,
      behind the master password.
    </p>
    {#if !mode && status === "unlocked"}
      <div class="buttons">
        <button type="button" data-vault="change" onclick={() => open("change")}
          >Change the master password…</button
        >
        <button type="button" data-vault="turn-off" onclick={() => open("off")}
          >Turn off the vault…</button
        >
      </div>
    {/if}
  {/if}

  {#if done}
    <p class="note" role="status" data-vault-done>{done}</p>
  {/if}

  {#if mode === "on"}
    <TurnOnVaultForm {keychain} onturnedon={turnedOn} oncancel={cancel} />
  {:else if mode === "change"}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <form
      onsubmit={(event) =>
        run(
          event,
          current !== "" && newPasswordReady,
          () => changeVaultPassword(current, password, repeated),
          "The master password was changed. The old one no longer opens the vault.",
        )}
      {onkeydown}
      data-vault-change-form
    >
      <label>
        Current master password
        <input
          bind:this={first}
          bind:value={current}
          type="password"
          name="vault-current"
          autocomplete="current-password"
          disabled={busy}
        />
      </label>
      <label>
        New master password
        <input
          bind:value={password}
          type="password"
          name="vault-new"
          autocomplete="new-password"
          disabled={busy}
        />
      </label>
      <label>
        The new password again
        <input
          bind:value={repeated}
          type="password"
          name="vault-new-repeat"
          autocomplete="new-password"
          disabled={busy}
        />
      </label>
      {#if differ}
        <p class="note" data-vault-differ>
          The two new passwords are not the same.
        </p>
      {/if}
      <p class="warning" data-vault-warning>
        If you forget the new master password, nobody can recover it.
      </p>
      <div class="actions">
        <button type="button" data-vault="cancel" onclick={cancel}
          >Cancel</button
        >
        <button
          type="submit"
          class="primary"
          data-vault="change-confirm"
          disabled={busy || current === "" || !newPasswordReady}
          >{busy ? "Changing…" : "Change"}</button
        >
      </div>
    </form>
  {:else if mode === "off" && !keychain}
    <div class="explained" data-vault-off-no-keychain>
      <p>
        The vault cannot be turned off here: this system has no keychain the app
        can use, so there is no other store to move the profiles and secrets to.
        They stay in the vault.
      </p>
      <div class="actions">
        <button type="button" data-vault="cancel" onclick={cancel}>Close</button
        >
      </div>
    </div>
  {:else if mode === "off"}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <form
      onsubmit={(event) =>
        run(
          event,
          current !== "",
          () => turnOffVault(current),
          "The vault is off: profiles and secrets are back in the system's keychain.",
        )}
      {onkeydown}
      data-vault-off-form
    >
      <p>
        Every workspace's profiles and secrets move back into the system's
        keychain, and the vault file is deleted. No master password is asked for
        at start any more.
      </p>
      <label>
        Master password
        <input
          bind:this={first}
          bind:value={current}
          type="password"
          name="vault-off-password"
          autocomplete="current-password"
          disabled={busy}
        />
      </label>
      <div class="actions">
        <button type="button" data-vault="cancel" onclick={cancel}
          >Cancel</button
        >
        <button
          type="submit"
          class="primary"
          data-vault="turn-off-confirm"
          disabled={busy || current === ""}
          >{busy ? "Moving…" : "Turn off"}</button
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
    gap: var(--space-2);
    padding-top: var(--space-2);
    border-top: 1px solid var(--border);
    font-size: var(--text-sm);
  }

  h3 {
    margin: 0;
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
  }

  p {
    margin: 0;
  }

  form {
    display: grid;
    gap: var(--space-2);
  }

  label {
    display: grid;
    gap: var(--space-1);
  }

  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }

  .explained {
    display: grid;
    gap: var(--space-2);
  }

  .warning {
    padding: var(--space-2) var(--space-2);
    border: 1px solid var(--danger);
    border-radius: var(--radius-sm);
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
    gap: var(--space-2);
  }
</style>
