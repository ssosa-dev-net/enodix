<script lang="ts">
  import type { ProfileSummary } from "./credentials";
  import ProfileForm from "./ProfileForm.svelte";
  import { deleteProfile, profiles } from "./profiles.svelte";

  interface Props {
    onclose: () => void;
  }

  let { onclose }: Props = $props();

  /** What the dialog shows: the list, or a form for one profile. */
  type Mode =
    | { kind: "list" }
    | { kind: "edit"; profile: ProfileSummary | null; name?: string }
    | { kind: "delete"; name: string };

  let mode = $state<Mode>({ kind: "list" });
  let error = $state<string | null>(null);

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

  async function remove(name: string) {
    error = await deleteProfile(name);
    mode = { kind: "list" };
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key !== "Escape") return;
    if (mode.kind === "list") onclose();
    else mode = { kind: "list" };
  }
</script>

<svelte:window {onkeydown} />

<div
  class="backdrop"
  role="dialog"
  aria-modal="true"
  aria-labelledby="profiles-title"
  data-dialog="profiles"
>
  <section>
    {#if mode.kind === "edit"}
      <h2 id="profiles-title">
        {mode.profile
          ? `Edit ${mode.profile.name}`
          : mode.name
            ? `Set up ${mode.name}`
            : "New credential profile"}
      </h2>
      {#key mode}
        <ProfileForm
          profile={mode.profile}
          fixedName={mode.name}
          confirm="Save"
          onsaved={() => (mode = { kind: "list" })}
          oncancel={() => (mode = { kind: "list" })}
        />
      {/key}
    {:else if mode.kind === "delete"}
      {@const name = mode.name}
      <h2 id="profiles-title">Delete {name}</h2>
      <p>
        Delete the profile {name} and its secrets from this computer? Hosts that use
        it will ask for it again.
      </p>
      <div class="actions">
        <button onclick={() => (mode = { kind: "list" })}>Cancel</button>
        <!-- svelte-ignore a11y_autofocus -->
        <button class="danger" autofocus onclick={() => remove(name)}
          >Delete</button
        >
      </div>
    {:else}
      <h2 id="profiles-title">Credential profiles</h2>
      <p>
        Logins hosts use through their credential profile setting, kept for this
        workspace in this computer's keychain.
      </p>
      {#if profiles.error}
        <p class="error" role="alert">{profiles.error}</p>
      {/if}
      {#if error}
        <p class="error" role="alert">{error}</p>
      {/if}
      <ul>
        {#each profiles.view.profiles as profile (profile.name)}
          <li data-profile={profile.name}>
            <span class="text">
              <strong>{profile.name}</strong>
              <small>{describe(profile)}</small>
            </span>
            <button onclick={() => (mode = { kind: "edit", profile })}
              >Edit</button
            >
            <button
              onclick={() => (mode = { kind: "delete", name: profile.name })}
              >Delete</button
            >
          </li>
        {/each}
        {#each profiles.view.missing as name (name)}
          <li class="missing" data-profile={name}>
            <span class="text">
              <strong>{name}</strong>
              <small>used in this workspace, not set up on this computer</small>
            </span>
            <button
              onclick={() => (mode = { kind: "edit", profile: null, name })}
              >Set up…</button
            >
          </li>
        {:else}
          {#if profiles.view.profiles.length === 0}
            <li class="empty">No profiles yet.</li>
          {/if}
        {/each}
      </ul>
      <div class="actions">
        <button onclick={() => (mode = { kind: "edit", profile: null })}
          >New profile…</button
        >
        <!-- svelte-ignore a11y_autofocus -->
        <button autofocus onclick={onclose}>Close</button>
      </div>
    {/if}
  </section>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 15;
    display: grid;
    place-items: center;
    padding: 1rem;
    background: rgb(0 0 0 / 0.45);
  }

  section {
    width: min(30rem, 100%);
    max-height: 100%;
    overflow: auto;
    display: grid;
    gap: 0.8rem;
    padding: 1.2rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.4);
  }

  h2 {
    margin: 0;
    font-size: 1.05rem;
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  p {
    margin: 0;
    color: var(--muted);
    font-size: 0.85rem;
  }

  .error {
    color: var(--danger);
    overflow-wrap: anywhere;
  }

  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: grid;
    gap: 0.3rem;
  }

  li {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.4rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: 4px;
    font-size: 0.85rem;
  }

  li.missing {
    border-style: dashed;
  }

  li.empty {
    color: var(--muted);
    border: none;
  }

  .text {
    flex: 1;
    min-width: 0;
    display: grid;
  }

  small {
    color: var(--muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  li button {
    padding: 0.2rem 0.6rem;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.6rem;
  }

  .danger {
    background: #b62324;
    border-color: #b62324;
  }
</style>
