<script lang="ts">
  import SerialPortList from "./SerialPortList.svelte";
  import {
    DEFAULT_SERIAL_PARAMETERS,
    type QuickConnectTarget,
    type SerialParameters,
  } from "./session";
  import type { FlowControl, Parity } from "./workspace";

  interface Props {
    name: string;
    onconnect: (target: QuickConnectTarget) => void;
    /** Opens a serial session on `port`, without an inventory host. */
    onserial: (port: string, parameters: SerialParameters) => void;
  }

  let { name, onconnect, onserial }: Props = $props();

  let mode = $state<"ssh" | "serial">("ssh");

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

<main>
  <div class="panel">
    <h1>{name}</h1>
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

  .panel,
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

  .modes {
    display: flex;
    gap: 0.3rem;
  }

  .modes button {
    padding: 0.2rem 0.8rem;
    font-size: 0.85rem;
  }

  .modes button[aria-pressed="true"] {
    border-color: var(--accent, var(--border));
    font-weight: 600;
  }

  p {
    margin: 0 0 0.2rem;
    color: var(--muted);
  }

  .row {
    display: grid;
    grid-template-columns: 1fr 6rem;
    gap: 0.75rem;
  }

  .line {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(7.5rem, 1fr));
    gap: 0.75rem;
  }

  label {
    display: grid;
    gap: 0.3rem;
    font-size: 0.8rem;
    color: var(--muted);
  }

  input,
  select {
    font: inherit;
    font-size: 0.95rem;
    color: var(--text);
    background: var(--surface);
    border: 1px solid var(--border);
    border-radius: 4px;
    padding: 0.5rem 0.6rem;
    min-width: 0;
  }

  input:focus-visible,
  select:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -1px;
  }

  button[type="submit"] {
    margin-top: 0.4rem;
    justify-self: start;
  }
</style>
