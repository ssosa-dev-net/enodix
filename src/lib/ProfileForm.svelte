<script lang="ts">
  import { untrack } from "svelte";
  import {
    pickKeyFile,
    type ProfileDraft,
    type ProfileSummary,
  } from "./credentials";
  import { saveProfile } from "./profiles.svelte";

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
  let kind = $state<"password" | "private_key">(
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

  // Secrets left blank keep what is stored, if anything is.
  const hasPassword = initial?.auth.kind === "password";
  const hasPassphrase =
    initial?.auth.kind === "private_key" && initial.auth.has_passphrase;
  const hasEnableSecret = initial?.has_enable_secret ?? false;

  function draft(): ProfileDraft {
    return {
      name,
      username,
      auth:
        kind === "password"
          ? {
              kind,
              password: password === "" && hasPassword ? null : password,
            }
          : {
              kind,
              key_file: keyFile.trim(),
              passphrase: removePassphrase
                ? ""
                : passphrase === ""
                  ? null
                  : passphrase,
            },
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
    busy = true;
    error = await saveProfile(initial?.name ?? null, draft());
    busy = false;
    if (error === null) onsaved(name.trim());
  }

  const keep = (stored: boolean) => (stored ? "Unchanged" : "");
</script>

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
    Secrets are kept in this computer's keychain, never in the workspace.
  </p>

  {#if error}
    <p class="error" role="alert">{error}</p>
  {/if}

  <div class="actions">
    <button type="button" onclick={oncancel}>Cancel</button>
    <button type="submit" disabled={busy}>{confirm}</button>
  </div>
</form>

<style>
  form {
    display: grid;
    gap: 0.8rem;
  }

  label {
    display: grid;
    gap: 0.3rem;
    font-size: 0.8rem;
    color: var(--muted);
  }

  label.check {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    margin-top: -0.4rem;
  }

  .row {
    display: flex;
    gap: 0.4rem;
  }

  .row input {
    flex: 1;
  }

  .note {
    margin: 0;
    font-size: 0.8rem;
    color: var(--muted);
  }

  .error {
    margin: 0;
    font-size: 0.85rem;
    color: var(--danger);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.6rem;
  }
</style>
