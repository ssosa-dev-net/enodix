<script lang="ts">
  import type { QuickConnectTarget } from "./session";

  interface Props {
    name: string;
    onconnect: (target: QuickConnectTarget) => void;
  }

  let { name, onconnect }: Props = $props();

  let host = $state("");
  let port = $state(22);
  let username = $state("");
  let password = $state("");

  function submit(event: SubmitEvent) {
    event.preventDefault();
    onconnect({ host: host.trim(), port, username, password });
  }
</script>

<main>
  <form onsubmit={submit}>
    <h1>{name}</h1>
    <p>Quick connect over SSH</p>

    <div class="row">
      <label class="host">
        Host
        <!-- svelte-ignore a11y_autofocus -->
        <input
          name="host"
          bind:value={host}
          required
          autofocus
          autocomplete="off"
          spellcheck="false"
          placeholder="192.0.2.10 or router1.example.net"
        />
      </label>
      <label class="port">
        Port
        <input
          name="port"
          type="number"
          bind:value={port}
          required
          min="1"
          max="65535"
        />
      </label>
    </div>
    <label>
      Username
      <input
        name="username"
        bind:value={username}
        required
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

    <button type="submit">Connect</button>
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
    width: min(26rem, 100%);
    display: grid;
    gap: 0.9rem;
  }

  h1 {
    margin: 0;
    font-size: 1.5rem;
    font-weight: 600;
  }

  p {
    margin: -0.6rem 0 0.4rem;
    color: var(--muted);
  }

  .row {
    display: grid;
    grid-template-columns: 1fr 6rem;
    gap: 0.75rem;
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

  button {
    margin-top: 0.4rem;
    justify-self: start;
  }
</style>
