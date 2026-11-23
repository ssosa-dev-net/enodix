<script lang="ts">
  import { onMount, untrack } from "svelte";
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
    /**
     * Why the host's profile could not log in (its SSH agent did not
     * work), and the profile's username to start from.
     */
    fallback?: { reason: string; username: string };
    onconnect: (credentials: { username: string; password: string }) => void;
    /** The missing profile has been set up: connect with it. */
    onprofile: () => void;
    oncancel: () => void;
  }

  let {
    host,
    missingProfile,
    fallback,
    onconnect,
    onprofile,
    oncancel,
  }: Props = $props();

  // The port the session will use: the resolved one, else SSH's default.
  const effective = effectiveSettings(() => ({ kind: "host", id: host.id }));

  let username = $state(untrack(() => fallback?.username) ?? "");
  let password = $state("");
  /** Log in by hand although the host has a profile. */
  let byHand = $state(false);
  let passwordInput = $state<HTMLInputElement>();

  // After a failed agent login the terminal had focus, and the username
  // is known: start at the password. (`autofocus` cannot take focus from
  // the terminal.)
  onMount(() => {
    if (fallback) passwordInput?.focus();
  });

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
      {#if fallback}
        <p class="notice" role="alert">
          Logging in with the SSH agent did not work: {fallback.reason}. Log in
          with a password instead.
        </p>
      {/if}
      <form onsubmit={submit}>
        <label>
          Username
          <!-- svelte-ignore a11y_autofocus -->
          <input
            name="username"
            bind:value={username}
            required
            autofocus={!fallback}
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
            bind:this={passwordInput}
            autocomplete="off"
          />
        </label>

        <div class="actions">
          <button type="submit">Connect</button>
          <button type="button" onclick={oncancel}>Cancel</button>
        </div>
      </form>
      {#if missingProfile === null && !fallback}
        <p class="hint" data-credentials-hint>
          Asked at each connection: to keep a login, name a credential profile
          in the Credential profile setting of this host's details.
        </p>
      {/if}
    {/if}
  </div>
</main>

<style>
  main {
    height: 100%;
    display: grid;
    place-items: center;
    padding: var(--space-6);
    overflow: auto;
  }

  .panel {
    width: min(22rem, 100%);
    display: grid;
    gap: var(--space-3);
  }

  form {
    display: grid;
    gap: var(--space-3);
  }

  h2 {
    margin: 0;
    font-size: var(--text-lg);
    font-weight: var(--weight-semibold);
  }

  p {
    margin: calc(-1 * var(--space-2)) 0 var(--space-1-5);
    color: var(--muted);
  }

  p.hint {
    margin: 0;
    font-size: var(--text-xs);
  }

  p.notice {
    margin: 0;
    font-size: var(--text-sm);
    color: var(--text);
    overflow-wrap: anywhere;
  }

  label {
    display: grid;
    gap: var(--space-1);
    font-size: var(--text-xs);
    color: var(--muted);
  }

  input {
    font: inherit;
    font-size: var(--text-md);
    color: var(--text);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding-inline: var(--space-2);
    min-width: 0;
  }

  input:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -1px;
  }

  .actions {
    display: flex;
    gap: var(--space-2);
    margin-top: var(--space-1-5);
  }

  .link {
    justify-self: start;
    min-height: 0;
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font-size: var(--text-xs);
  }
</style>
