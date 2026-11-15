<script lang="ts">
  import { untrack } from "svelte";
  import {
    pickKeyFile,
    type ProfileDraft,
    type ProfileSummary,
  } from "./credentials";
  import { profiles, refreshProfiles, saveProfile } from "./profiles.svelte";
  import TurnOnVaultForm from "./TurnOnVaultForm.svelte";

  interface Props {
    /** The profile being changed; null creates one. */
    profile: ProfileSummary | null;
    /** A name to create the profile under, which cannot be changed. */
    fixedName?: string;
    confirm: string;
    /** Called with the profile's name once it is saved. */
    onsaved: (name: string) => void;
    oncancel: () => void;
  }

  let { profile, fixedName, confirm, onsaved, oncancel }: Props = $props();

  // The form's own copy, filled from the profile once.
  const initial = untrack(() => profile);
  let name = $state(untrack(() => fixedName) ?? initial?.name ?? "");
  let username = $state(initial?.username ?? "");
  let kind = $state<"password" | "private_key" | "agent">(
    initial?.auth.kind ?? "password",
  );
  let password = $state("");
  let keyFile = $state(
    initial?.auth.kind === "private_key" ? initial.auth.key_file : "",
  );
  let passphrase = $state("");
  let removePassphrase = $state(false);
  let enableSecret = $state("");
  let removeEnableSecret = $state(false);
  let error = $state<string | null>(null);
  let busy = $state(false);
  /** Saving where only the vault can keep a profile (no keychain, vault
   * off): why it must be turned on, then the form that does it. */
  let needsVault = $state<"explain" | "turn-on" | null>(null);

  // Secrets left blank keep what is stored, if anything is.
  const hasPassword = initial?.auth.kind === "password";
  const hasPassphrase =
    initial?.auth.kind === "private_key" && initial.auth.has_passphrase;
  const hasEnableSecret = initial?.has_enable_secret ?? false;

  function auth(): ProfileDraft["auth"] {
    switch (kind) {
      case "password":
        return {
          kind,
          password: password === "" && hasPassword ? null : password,
        };
      case "private_key":
        return {
          kind,
          key_file: keyFile.trim(),
          passphrase: removePassphrase
            ? ""
            : passphrase === ""
              ? null
              : passphrase,
        };
      case "agent":
        return { kind };
    }
  }

  function draft(): ProfileDraft {
    return {
      name,
      username,
      auth: auth(),
      enable_secret: removeEnableSecret
        ? ""
        : enableSecret === ""
          ? null
          : enableSecret,
    };
  }

  async function browse() {
    const file = await pickKeyFile();
    if (file !== null) keyFile = file;
  }

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (profiles.view.needs_vault) needsVault = "explain";
    else await save();
  }

  async function save() {
    busy = true;
    error = await saveProfile(initial?.name ?? null, draft());
    busy = false;
    if (error === null) onsaved(name.trim());
  }

  // Escape goes back to the form (TurnOnVaultForm's own Escape goes back
  // to the explanation) rather than closing the dialog and what was typed.
  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && needsVault === "explain") {
      event.stopPropagation();
      needsVault = null;
    }
  }

  /** The vault is on: the profile typed is saved into it. */
  async function vaultTurnedOn() {
    needsVault = null;
    await refreshProfiles();
    await save();
  }

  const keep = (stored: boolean) => (stored ? "Unchanged" : "");
</script>

{#if needsVault}
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div class="needs-vault" data-needs-vault={needsVault} {onkeydown}>
    <p>
      This system has no keychain the app can use, so profiles can only be kept
      in the vault, behind a master password. Turn the vault on to save this
      profile; what you typed is kept meanwhile.
    </p>
    {#if needsVault === "turn-on"}
      <TurnOnVaultForm
        keychain={false}
        onturnedon={vaultTurnedOn}
        oncancel={() => (needsVault = "explain")}
      />
    {:else}
      <div class="actions">
        <button
          type="button"
          data-needs-vault-action="back"
          onclick={() => (needsVault = null)}>Back</button
        >
        <!-- svelte-ignore a11y_autofocus -->
        <button
          type="button"
          class="primary"
          autofocus
          data-needs-vault-action="turn-on"
          onclick={() => (needsVault = "turn-on")}>Turn on the vault…</button
        >
      </div>
    {/if}
  </div>
{:else}
  <form class="profile" onsubmit={submit}>
    {#if fixedName === undefined}
      <label>
        Profile name
        <!-- svelte-ignore a11y_autofocus -->
        <input
          name="profile-name"
          bind:value={name}
          required
          autofocus
          autocomplete="off"
          spellcheck="false"
        />
      </label>
    {/if}
    <label>
      Username
      <!-- svelte-ignore a11y_autofocus -->
      <input
        name="profile-username"
        bind:value={username}
        required
        autofocus={fixedName !== undefined}
        autocomplete="off"
        spellcheck="false"
      />
    </label>
    <label>
      Log in with
      <select name="profile-auth" bind:value={kind}>
        <option value="password">Password</option>
        <option value="private_key">Private key</option>
        <option value="agent">SSH agent</option>
      </select>
    </label>
    {#if kind === "password"}
      <label>
        Password
        <input
          name="profile-password"
          type="password"
          bind:value={password}
          required={!hasPassword}
          placeholder={keep(hasPassword)}
          autocomplete="off"
        />
      </label>
    {:else if kind === "agent"}
      <p class="note" data-agent-note>
        Uses the keys of the SSH agent running on this computer: the one
        SSH_AUTH_SOCK names, else on Windows the OpenSSH agent or Pageant. The
        private keys never leave it.
      </p>
    {:else}
      <label>
        Private key file
        <span class="row">
          <input
            name="profile-key-file"
            bind:value={keyFile}
            required
            autocomplete="off"
            spellcheck="false"
          />
          <button type="button" onclick={browse}>Browse…</button>
        </span>
      </label>
      <label>
        Passphrase (if the key has one)
        <input
          name="profile-passphrase"
          type="password"
          bind:value={passphrase}
          placeholder={keep(hasPassphrase)}
          disabled={removePassphrase}
          autocomplete="off"
        />
      </label>
      {#if hasPassphrase}
        <label class="check">
          <input type="checkbox" bind:checked={removePassphrase} />
          The key has no passphrase any more
        </label>
      {/if}
    {/if}
    <label>
      Enable secret (optional)
      <input
        name="profile-enable"
        type="password"
        bind:value={enableSecret}
        placeholder={keep(hasEnableSecret)}
        disabled={removeEnableSecret}
        autocomplete="off"
      />
    </label>
    {#if hasEnableSecret}
      <label class="check">
        <input type="checkbox" bind:checked={removeEnableSecret} />
        Remove the enable secret
      </label>
    {/if}

    <p class="note">
      Secrets are kept on this computer (in its keychain, or in the vault when
      it is on), never in the workspace.
    </p>

    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}

    <div class="actions">
      <button type="button" onclick={oncancel}>Cancel</button>
      <button type="submit" disabled={busy}>{confirm}</button>
    </div>
  </form>
{/if}

<style>
  form {
    display: grid;
    gap: var(--space-3);
  }

  label {
    display: grid;
    gap: var(--space-1);
    font-size: var(--text-xs);
    color: var(--muted);
  }

  label.check {
    display: flex;
    align-items: center;
    gap: var(--space-1-5);
    margin-top: calc(-1 * var(--space-1-5));
  }

  .row {
    display: flex;
    gap: var(--space-1-5);
  }

  .row input {
    flex: 1;
  }

  .note {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .error {
    margin: 0;
    font-size: var(--text-sm);
    color: var(--danger);
    overflow-wrap: anywhere;
  }

  .needs-vault {
    display: grid;
    gap: var(--space-3);
    font-size: var(--text-sm);
  }

  .needs-vault p {
    margin: 0;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
