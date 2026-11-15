<script lang="ts">
  // The Credentials side view: the open workspace's credential profiles
  // (no secrets shown), the names of its secrets and where both are kept
  // (the keychain, or the vault and its state). Profiles the workspace
  // names that are not stored here stand out and are set up in place.
  // Every form opens in the view itself, in place of the lists; Escape or
  // Cancel goes back to them.
  import KeyRound from "@lucide/svelte/icons/key-round";
  import Lock from "@lucide/svelte/icons/lock";
  import LockOpen from "@lucide/svelte/icons/lock-open";
  import Pencil from "@lucide/svelte/icons/pencil";
  import Plus from "@lucide/svelte/icons/plus";
  import Settings from "@lucide/svelte/icons/settings";
  import Trash from "@lucide/svelte/icons/trash";
  import TriangleAlert from "@lucide/svelte/icons/triangle-alert";
  import type { ProfileSummary } from "./credentials";
  import { inventory } from "./inventory.svelte";
  import { openView } from "./views";
  import ProfileForm from "./ProfileForm.svelte";
  import { deleteProfile, deleteSecret, profiles } from "./profiles.svelte";
  import SecretForm from "./SecretForm.svelte";

  /** What the view shows: the lists, or a form for one profile or secret. */
  type Mode =
    | { kind: "list" }
    /** `profile` null creates one; `name` sets up a missing one. */
    | { kind: "edit"; profile: ProfileSummary | null; name?: string }
    | { kind: "delete"; name: string }
    /** `name` null adds a secret; else its value is replaced. */
    | { kind: "secret"; name: string | null }
    | { kind: "deleteSecret"; name: string };

  let mode = $state<Mode>({ kind: "list" });
  let error = $state<string | null>(null);

  const workspace = $derived(inventory.workspace);
  const view = $derived(profiles.view);
  const vault = $derived(profiles.vault);

  function describe(profile: ProfileSummary): string {
    const login =
      profile.auth.kind === "password"
        ? "password"
        : profile.auth.kind === "agent"
          ? "SSH agent"
          : `key ${profile.auth.key_file}`;
    const enable = profile.has_enable_secret ? ", enable secret" : "";
    return `${profile.username}, ${login}${enable}`;
  }

  function show(next: Mode) {
    error = null;
    mode = next;
  }

  const list = () => (mode = { kind: "list" });

  async function remove(name: string) {
    error = await deleteProfile(name);
    mode = { kind: "list" };
  }

  async function removeSecret(name: string) {
    error = await deleteSecret(name);
    mode = { kind: "list" };
  }

  // Escape inside a form goes back to the lists (a form's own Escape, such
  // as the vault's inside the profile form, stops it first).
  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && mode.kind !== "list") {
      event.stopPropagation();
      list();
    }
  }
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div class="credentials" data-credentials data-mode={mode.kind} {onkeydown}>
  <header>
    <h2>Credentials</h2>
    <button
      class="tool"
      data-profile-new
      aria-label="New credential profile"
      title="New credential profile, kept on this computer"
      disabled={!workspace}
      onclick={() => show({ kind: "edit", profile: null })}
      ><Plus size={16} aria-hidden="true" /></button
    >
  </header>

  {#if !workspace}
    <p class="hint">Open a workspace to see its credentials.</p>
  {:else if mode.kind === "edit"}
    <div class="form">
      <h3 class="title" data-credentials-form="profile">
        {mode.profile
          ? `Edit ${mode.profile.name}`
          : mode.name
            ? `Set up ${mode.name}`
            : "New credential profile"}
      </h3>
      {#key mode}
        <ProfileForm
          profile={mode.profile}
          fixedName={mode.name}
          confirm="Save"
          onsaved={list}
          oncancel={list}
        />
      {/key}
    </div>
  {:else if mode.kind === "secret"}
    <div class="form">
      <h3 class="title" data-credentials-form="secret">
        {mode.name ? `Replace the value of ${mode.name}` : "New secret"}
      </h3>
      {#key mode}
        <SecretForm replacing={mode.name} onsaved={list} oncancel={list} />
      {/key}
    </div>
  {:else if mode.kind === "delete" || mode.kind === "deleteSecret"}
    {@const name = mode.name}
    {@const secret = mode.kind === "deleteSecret"}
    <div class="form" data-credentials-confirm={secret ? "secret" : "profile"}>
      <h3 class="title">Delete {name}</h3>
      <p class="hint">
        {secret
          ? `Delete the secret ${name} and its value from this computer?`
          : `Delete the profile ${name} and its secrets from this computer? Hosts that use it will ask for it again.`}
      </p>
      <div class="actions">
        <button onclick={list}>Cancel</button>
        <!-- svelte-ignore a11y_autofocus -->
        <button
          class="danger"
          autofocus
          data-profile-action={secret ? undefined : "delete"}
          data-secret-action={secret ? "delete" : undefined}
          onclick={() => (secret ? removeSecret(name) : remove(name))}
          >Delete</button
        >
      </div>
    </div>
  {:else}
    <div class="body">
      <div class="vault" data-credentials-vault={vault ?? ""}>
        <span class="vault-icon" aria-hidden="true">
          {#if vault === "unlocked"}
            <LockOpen size={14} />
          {:else if vault === "locked"}
            <Lock size={14} />
          {:else}
            <KeyRound size={14} />
          {/if}
        </span>
        <span class="vault-text">
          {#if vault === "unlocked"}
            Kept in the vault, unlocked.
          {:else if vault === "locked"}
            Kept in the vault, which is locked.
          {:else if view.needs_vault}
            No keychain here and the vault is off.
          {:else}
            Kept in this computer's keychain; the vault is off.
          {/if}
        </span>
        <button
          class="small"
          data-open-vault
          aria-label="The vault, in Preferences"
          title="The vault, in Preferences"
          onclick={() => openView("preferences")}
          ><Settings size={13} aria-hidden="true" /></button
        >
      </div>
      {#if view.needs_vault}
        <p class="note" data-profiles-needs-vault>
          This system has no keychain the app can use: profiles and secrets are
          kept only in the vault, which saving one offers to turn on.
        </p>
      {/if}
      {#if profiles.error}
        <p class="error" role="alert">{profiles.error}</p>
      {/if}
      {#if error}
        <p class="error" role="alert">{error}</p>
      {/if}

      <h3>
        Profiles
        {#if view.profiles.length > 0}<span class="count"
            >{view.profiles.length}</span
          >{/if}
      </h3>
      {#if view.profiles.length === 0 && view.missing.length === 0}
        <div class="empty" data-profiles-empty>
          <p>
            A credential profile is a login (a username with a password, a key
            or the SSH agent) that hosts name in their settings, kept on this
            computer, never in the workspace's files.
          </p>
          <button
            data-profile-first
            onclick={() => show({ kind: "edit", profile: null })}
            ><Plus size={14} aria-hidden="true" /> New profile</button
          >
        </div>
      {:else}
        <ul data-profiles>
          {#each view.missing as name (name)}
            <li class="missing" data-profile={name} data-profile-missing>
              <TriangleAlert
                class="mark"
                size={14}
                aria-label="Not set up here"
              />
              <span class="text">
                <strong>{name}</strong>
                <small
                  >used in this workspace, not set up on this computer</small
                >
              </span>
              <button
                class="setup"
                data-profile-setup
                title={`Store the login of ${name} on this computer`}
                onclick={() => show({ kind: "edit", profile: null, name })}
                >Set up…</button
              >
            </li>
          {/each}
          {#each view.profiles as profile (profile.name)}
            <li data-profile={profile.name}>
              <span class="text">
                <strong>{profile.name}</strong>
                <small>{describe(profile)}</small>
              </span>
              <button
                class="small"
                data-profile-edit
                title="Edit or rename"
                aria-label={`Edit ${profile.name}`}
                onclick={() => show({ kind: "edit", profile })}
                ><Pencil size={13} aria-hidden="true" /></button
              >
              <button
                class="small"
                data-profile-delete
                title="Delete"
                aria-label={`Delete ${profile.name}`}
                onclick={() => show({ kind: "delete", name: profile.name })}
                ><Trash size={13} aria-hidden="true" /></button
              >
            </li>
          {/each}
        </ul>
      {/if}

      <h3>
        Secrets
        {#if view.secrets.length > 0}<span class="count"
            >{view.secrets.length}</span
          >{/if}
        <button
          class="small add"
          data-secret-new
          aria-label="New secret"
          title="New named secret"
          onclick={() => show({ kind: "secret", name: null })}
          ><Plus size={13} aria-hidden="true" /></button
        >
      </h3>
      <ul data-secrets>
        {#each view.secrets as name (name)}
          <li data-secret={name}>
            <span class="text"><strong>{name}</strong></span>
            <button
              class="small"
              data-secret-replace
              title="Replace the value"
              aria-label={`Replace the value of ${name}`}
              onclick={() => show({ kind: "secret", name })}
              ><Pencil size={13} aria-hidden="true" /></button
            >
            <button
              class="small"
              data-secret-delete
              title="Delete"
              aria-label={`Delete ${name}`}
              onclick={() => show({ kind: "deleteSecret", name })}
              ><Trash size={13} aria-hidden="true" /></button
            >
          </li>
        {:else}
          <li class="empty-line">
            Named values (tokens, keys) for scripts and integrations. Only their
            names are shown.
          </li>
        {/each}
      </ul>
    </div>
  {/if}
</div>

<style>
  .credentials {
    height: 100%;
    min-height: 0;
    display: flex;
    flex-direction: column;
    font-size: var(--text-sm);
  }

  header {
    display: flex;
    align-items: center;
    gap: var(--space-0-5);
    padding: var(--space-1-5) var(--space-1-5) var(--space-1-5) var(--space-3);
    border-bottom: 1px solid var(--border);
  }

  h2 {
    flex: 1;
    margin: 0;
    padding: var(--space-1) 0;
    font-size: var(--text-sm);
    font-weight: var(--weight-semibold);
  }

  .tool {
    flex: none;
    display: inline-grid;
    place-items: center;
    width: 28px;
    height: 28px;
    padding: 0;
    border-color: transparent;
    background: none;
    color: var(--muted);
  }

  .tool:hover:not(:disabled) {
    color: var(--text);
    background: var(--surface-raised);
  }

  .body,
  .form {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding: var(--space-2) 0 var(--space-3);
  }

  .form {
    display: grid;
    align-content: start;
    gap: var(--space-2);
    padding: var(--space-3);
  }

  .hint {
    margin: var(--space-2) var(--space-3);
    color: var(--muted);
    font-size: var(--text-xs);
  }

  .form .hint {
    margin: 0;
  }

  .vault {
    display: flex;
    align-items: center;
    gap: var(--space-1-5);
    margin: 0 var(--space-2) var(--space-1);
    padding: var(--space-1) var(--space-1) var(--space-1) var(--space-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-md);
    color: var(--muted);
    font-size: var(--text-xs);
  }

  .vault[data-credentials-vault="unlocked"] .vault-icon {
    color: var(--accent);
  }

  .vault-icon {
    display: inline-grid;
  }

  .vault-text {
    flex: 1;
    min-width: 0;
  }

  .note {
    margin: var(--space-1) var(--space-2);
    padding: var(--space-1-5) var(--space-2);
    border: 1px solid var(--warning);
    border-radius: var(--radius-sm);
    color: var(--muted);
    font-size: var(--text-xs);
  }

  .error {
    margin: var(--space-1) var(--space-3);
    color: var(--danger);
    font-size: var(--text-xs);
    overflow-wrap: anywhere;
  }

  h3 {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    margin: var(--space-2) var(--space-3) var(--space-0-5);
    color: var(--muted);
    font-size: var(--text-2xs);
    font-weight: var(--weight-semibold);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  h3.title {
    margin: 0;
    color: var(--text);
    font-size: var(--text-sm);
    text-transform: none;
    letter-spacing: normal;
    overflow-wrap: anywhere;
  }

  h3 .add {
    margin-left: auto;
  }

  .count {
    color: var(--muted);
    font-weight: normal;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  li {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    padding: var(--space-1) var(--space-2) var(--space-1) var(--space-3);
  }

  li:hover {
    background: var(--surface-raised);
  }

  li.missing {
    color: var(--warning);
  }

  li.missing strong {
    color: var(--text);
  }

  .text {
    flex: 1;
    min-width: 0;
    display: grid;
  }

  .text strong {
    font-weight: var(--weight-medium);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  small {
    color: var(--muted);
    font-size: var(--text-2xs);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .small {
    flex: none;
    display: inline-grid;
    place-items: center;
    padding: var(--space-1);
    border-color: transparent;
    background: none;
    color: var(--muted);
  }

  .small:hover {
    color: var(--text);
  }

  .setup {
    flex: none;
    padding: var(--space-0-5) var(--space-2);
    font-size: var(--text-xs);
  }

  .empty-line {
    color: var(--muted);
    font-size: var(--text-xs);
  }

  .empty-line:hover {
    background: none;
  }

  .empty {
    display: grid;
    justify-items: start;
    gap: var(--space-2);
    padding: var(--space-1) var(--space-3) var(--space-2);
    color: var(--muted);
    font-size: var(--text-xs);
  }

  .empty p {
    margin: 0;
  }

  .empty button {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }

  .danger,
  .danger:hover:not(:disabled) {
    color: var(--on-danger);
    background: var(--danger-fill);
    border-color: var(--danger-fill);
  }
</style>
