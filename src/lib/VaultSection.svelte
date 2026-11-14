<script lang="ts">
  // The Preferences dialog's Security section: whether credentials are in
  // the keychain or the vault; turning the vault on, changing its master
  // password and turning it off.
  import { refreshProfiles } from "./profiles.svelte";
  import {
    changeVaultPassword,
    turnOffVault,
    turnOnVault,
    vaultFailure,
    vaultStatus,
    type VaultStatus,
  } from "./vault";

  /** The form shown: turning on, changing the password, turning off. */
  type Mode = "on" | "change" | "off";

  let status = $state<VaultStatus | null>(null);
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

<section class="security" data-vault-status={status}>
  <h3>Security</h3>
  {#if status === "off"}
    <p>
      Credential profiles and secrets are kept in the system's keychain. A vault
      keeps them in an encrypted file instead, behind a master password asked
      for once at every start.
    </p>
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
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <form
      onsubmit={(event) =>
        run(
          event,
          newPasswordReady,
          () => turnOnVault(password, repeated),
          null,
        )}
      {onkeydown}
      data-vault-form
    >
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
          disabled={busy || !newPasswordReady}
          >{busy ? "Moving…" : "Turn on"}</button
        >
      </div>
    </form>
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

  .buttons {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
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
