<script lang="ts">
  import { effectiveSettings } from "./inventory.svelte";
  import ProfileForm from "./ProfileForm.svelte";
  import type { Host } from "./workspace";

  interface Props {
    host: Host;
    /**
     * The credential profile the host uses that is not set up on this
     * computer; null when it has none.
     */
    missingProfile: string | null;
    onconnect: (credentials: { username: string; password: string }) => void;
    /** The missing profile has been set up: connect with it. */
    onprofile: () => void;
    oncancel: () => void;
  }

  let { host, missingProfile, onconnect, onprofile, oncancel }: Props =
    $props();

  // The port the session will use: the resolved one, else SSH's default.
  const effective = effectiveSettings(() => ({ kind: "host", id: host.id }));

  let username = $state("");
  let password = $state("");
  /** Log in by hand although the host has a profile. */
  let byHand = $state(false);

  function submit(event: SubmitEvent) {
    event.preventDefault();
    onconnect({ username, password });
  }
</script>

<main>
  <div class="panel">
    <h2>{host.name}</h2>
    <p>SSH to {host.address}:{effective.current?.port?.value ?? 22}</p>

    {#if missingProfile !== null && !byHand}
      <p class="notice" role="status">
        This host uses the credential profile <strong>{missingProfile}</strong>,
        which is not set up on this computer. Enter it to connect; it is saved
        for the next time.
      </p>
      <ProfileForm
        profile={null}
        fixedName={missingProfile}
        confirm="Save and connect"
        onsaved={onprofile}
        {oncancel}
      />
      <button class="link" type="button" onclick={() => (byHand = true)}
        >Log in once without saving a profile</button
      >
    {:else}
      <form onsubmit={submit}>
        <label>
          Username
          <!-- svelte-ignore a11y_autofocus -->
          <input
            name="username"
            bind:value={username}
            required
            autofocus
            autocomplete="off"
            spellcheck="false"
          />
        </label>
        <label>
          Password
          <input
            name="password"
            type="password"
            bind:value={password}
            autocomplete="off"
          />
        </label>

        <div class="actions">
          <button type="submit">Connect</button>
          <button type="button" onclick={oncancel}>Cancel</button>
        </div>
      </form>
    {/if}
  </div>
</main>

<style>
  main {
    height: 100%;
    display: grid;
    place-items: center;
    padding: 1.5rem;
    overflow: auto;
  }

  .panel {
    width: min(22rem, 100%);
    display: grid;
    gap: 0.9rem;
  }

  form {
    display: grid;
    gap: 0.9rem;
  }

  h2 {
    margin: 0;
    font-size: 1.25rem;
    font-weight: 600;
  }

  p {
    margin: -0.6rem 0 0.4rem;
    color: var(--muted);
  }

  p.notice {
    margin: 0;
    font-size: 0.85rem;
    color: var(--text);
    overflow-wrap: anywhere;
  }

  label {
    display: grid;
    gap: 0.3rem;
    font-size: 0.8rem;
    color: var(--muted);
  }

  input {
    font: inherit;
    font-size: 0.95rem;
    color: var(--text);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0.5rem 0.6rem;
    min-width: 0;
  }

  input:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -1px;
  }

  .actions {
    display: flex;
    gap: 0.6rem;
    margin-top: 0.4rem;
  }

  .link {
    justify-self: start;
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font-size: 0.8rem;
  }
</style>
