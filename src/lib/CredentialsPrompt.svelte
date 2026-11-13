<script lang="ts">
  import type { Host } from "./workspace";

  interface Props {
    host: Host;
    onconnect: (credentials: { username: string; password: string }) => void;
    oncancel: () => void;
  }

  let { host, onconnect, oncancel }: Props = $props();

  let username = $state("");
  let password = $state("");

  function submit(event: SubmitEvent) {
    event.preventDefault();
    onconnect({ username, password });
  }
</script>

<main>
  <form onsubmit={submit}>
    <h2>{host.name}</h2>
    <p>SSH to {host.address}{host.port ? `:${host.port}` : ""}</p>

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
</main>

<style>
  main {
    height: 100%;
    display: grid;
    place-items: center;
    padding: 1.5rem;
  }

  form {
    width: min(22rem, 100%);
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
</style>
