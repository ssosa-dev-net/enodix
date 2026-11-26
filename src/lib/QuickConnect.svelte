<script lang="ts">
  import Search from "@lucide/svelte/icons/search";
  import SerialPortList from "./SerialPortList.svelte";
  import {
    DEFAULT_SERIAL_PARAMETERS,
    localShells,
    type LocalShell,
    type QuickConnectTarget,
    type SerialParameters,
  } from "./session";
  import type { FlowControl, Parity } from "./workspace";

  interface Props {
    name: string;
    /** Drawn inside another view (the welcome view), under its own title. */
    embedded?: boolean;
    /** Opens the palette (hosts, snippets, commands); offered when given. */
    onpalette?: () => void;
    onconnect: (target: QuickConnectTarget) => void;
    /** Opens a serial session on `port`, without an inventory host. */
    onserial: (port: string, parameters: SerialParameters) => void;
    /** Opens a shell on this computer: `shell` is the id of one the
     * computer lists, null the one the preference names (#22). */
    onlocal: (shell: string | null) => void;
  }

  let {
    name,
    embedded = false,
    onconnect,
    onserial,
    onlocal,
    onpalette,
  }: Props = $props();

  let mode = $state<"ssh" | "serial" | "local">("ssh");

  /** The shells this computer has; null until it answers. */
  let shells = $state<LocalShell[] | null>(null);
  /** The shell picked here; empty: the preference's. */
  let shell = $state("");
  localShells().then(
    (list) => (shells = list),
    () => (shells = []),
  );

  function submitLocal(event: SubmitEvent) {
    event.preventDefault();
    onlocal(shell || null);
  }

  let host = $state("");
  let port = $state(22);
  let username = $state("");
  let password = $state("");

  let serialPort = $state("");
  let baudRate = $state(DEFAULT_SERIAL_PARAMETERS.baudRate);
  let dataBits = $state(DEFAULT_SERIAL_PARAMETERS.dataBits);
  let parity = $state<Parity>(DEFAULT_SERIAL_PARAMETERS.parity);
  let stopBits = $state(DEFAULT_SERIAL_PARAMETERS.stopBits);
  let flowControl = $state<FlowControl>(DEFAULT_SERIAL_PARAMETERS.flowControl);

  const BAUD_RATES = [
    1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400,
  ];

  function submit(event: SubmitEvent) {
    event.preventDefault();
    onconnect({ host: host.trim(), port, username, password });
  }

  function submitSerial(event: SubmitEvent) {
    event.preventDefault();
    onserial(serialPort, {
      baudRate: Number(baudRate),
      dataBits: Number(dataBits),
      parity,
      stopBits: Number(stopBits),
      flowControl,
    });
  }
</script>

{#if embedded}
  <div class="panel embedded" data-quick-connect>
    <h2>Quick connect</h2>
    {@render body()}
  </div>
{:else}
  <main data-connect-view>
    <div class="panel" data-quick-connect>
      <h1>{name}</h1>
      {#if onpalette}
        <div class="palette">
          <p data-connect-hint>
            Double-click a host in the Hosts view to connect, or find it by name
            or address:
          </p>
          <button
            type="button"
            data-empty-pane="palette"
            aria-label="Search hosts and commands"
            title="Search hosts, snippets and commands (Ctrl+K)"
            onclick={onpalette}
          >
            <Search size={14} aria-hidden="true" />
            <span>Search hosts…</span>
            <kbd>Ctrl+K</kbd>
          </button>
          <p>Or quick connect to any device:</p>
        </div>
      {/if}
      {@render body()}
    </div>
  </main>
{/if}

{#snippet body()}
  <div class="modes" role="group" aria-label="Quick connect by">
    <button
      type="button"
      data-quick-mode="ssh"
      aria-pressed={mode === "ssh"}
      onclick={() => (mode = "ssh")}>SSH</button
    >
    <button
      type="button"
      data-quick-mode="serial"
      aria-pressed={mode === "serial"}
      onclick={() => (mode = "serial")}>Serial</button
    >
    <button
      type="button"
      data-quick-mode="local"
      aria-pressed={mode === "local"}
      onclick={() => (mode = "local")}>Local shell</button
    >
  </div>

  {#if mode === "ssh"}
    <form onsubmit={submit}>
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
  {:else if mode === "local"}
    <form onsubmit={submitLocal} data-quick-local>
      <p>Open a shell on this computer</p>
      <label>
        Shell
        <select name="local-shell" bind:value={shell}>
          <option value="">Default</option>
          {#each shells ?? [] as each (each.id)}
            <option value={each.id}>{each.name}</option>
          {/each}
        </select>
      </label>
      <button type="submit" data-open-local-shell>Open</button>
    </form>
  {:else}
    <form onsubmit={submitSerial} data-quick-serial>
      <p>Open a serial console</p>
      <SerialPortList
        selected={serialPort}
        onpick={(picked) => (serialPort = picked)}
      />
      <label>
        Port
        <input
          name="serial-port"
          bind:value={serialPort}
          required
          autocomplete="off"
          spellcheck="false"
          placeholder="COM3"
        />
      </label>
      <div class="line">
        <label>
          Baud rate
          <input
            name="baud-rate"
            type="number"
            min="1"
            list="quick-baud-rates"
            bind:value={baudRate}
            required
          />
          <datalist id="quick-baud-rates">
            {#each BAUD_RATES as rate (rate)}
              <option value={rate}></option>
            {/each}
          </datalist>
        </label>
        <label>
          Data bits
          <select name="data-bits" bind:value={dataBits}>
            {#each [5, 6, 7, 8] as bits (bits)}
              <option value={bits}>{bits}</option>
            {/each}
          </select>
        </label>
        <label>
          Parity
          <select name="parity" bind:value={parity}>
            <option value="none">None</option>
            <option value="odd">Odd</option>
            <option value="even">Even</option>
          </select>
        </label>
        <label>
          Stop bits
          <select name="stop-bits" bind:value={stopBits}>
            <option value={1}>1</option>
            <option value={2}>2</option>
          </select>
        </label>
        <label>
          Flow control
          <select name="flow-control" bind:value={flowControl}>
            <option value="none">None</option>
            <option value="hardware">Hardware (RTS/CTS)</option>
            <option value="software">Software (XON/XOFF)</option>
          </select>
        </label>
      </div>

      <button type="submit">Open</button>
    </form>
  {/if}
{/snippet}

<style>
  main {
    height: 100%;
    display: grid;
    place-items: center;
    padding: var(--space-6);
    overflow: auto;
  }

  .panel,
  form {
    width: min(26rem, 100%);
    display: grid;
    gap: var(--space-3);
  }

  h1 {
    margin: 0;
    font-size: var(--text-xl);
    font-weight: var(--weight-semibold);
  }

  h2 {
    margin: 0;
    font-size: var(--text-lg);
    font-weight: var(--weight-semibold);
  }

  .panel.embedded {
    width: 100%;
  }

  .palette {
    display: grid;
    gap: var(--space-2);
  }

  .palette button {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    color: var(--muted);
    text-align: start;
  }

  .palette button span {
    flex: 1;
  }

  .modes {
    display: flex;
    gap: var(--space-1);
  }

  .modes button[aria-pressed="true"] {
    border-color: var(--accent, var(--border));
    font-weight: var(--weight-semibold);
  }

  p {
    margin: 0 0 var(--space-0-5);
    color: var(--muted);
    line-height: 1.5;
  }

  .row {
    display: grid;
    grid-template-columns: 1fr 6rem;
    gap: var(--space-3);
  }

  .line {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(7.5rem, 1fr));
    gap: var(--space-3);
  }

  label {
    display: grid;
    gap: var(--space-1);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    color: var(--muted);
  }

  input,
  select {
    font: inherit;
    font-size: var(--text-md);
    color: var(--text);
    background-color: var(--surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    padding-left: var(--space-2);
    min-width: 0;
  }

  input:focus-visible,
  select:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -1px;
  }

  button[type="submit"] {
    margin-top: var(--space-1-5);
    justify-self: start;
  }
</style>
