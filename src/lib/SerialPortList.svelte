<script lang="ts">
  import { serialPorts, type SerialPortInfo } from "./session";

  interface Props {
    /** The port chosen now, highlighted in the list. */
    selected: string;
    /** Called with the port the user picks. */
    onpick: (port: string) => void;
  }

  let { selected, onpick }: Props = $props();

  let ports = $state<SerialPortInfo[] | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(false);

  /** Asks the core for the ports there are now. */
  export async function refresh() {
    loading = true;
    try {
      ports = await serialPorts();
      error = null;
    } catch (failure) {
      error = `Cannot list the serial ports: ${failure}`;
    } finally {
      loading = false;
    }
  }

  void refresh();
</script>

<div class="ports" data-serial-ports>
  <div class="head">
    <span>Detected ports</span>
    <button
      type="button"
      class="small"
      data-serial-refresh
      disabled={loading}
      onclick={refresh}>{loading ? "Looking…" : "Refresh"}</button
    >
  </div>
  {#if error}
    <p class="error" role="alert">{error}</p>
  {:else if ports === null}
    <p class="none">Looking for serial ports…</p>
  {:else if ports.length === 0}
    <p class="none" data-serial-none>
      No serial ports found. Plug in the adapter, then refresh.
    </p>
  {:else}
    <ul>
      {#each ports as port (port.name)}
        <li>
          <button
            type="button"
            data-serial-port={port.name}
            aria-pressed={port.name === selected}
            onclick={() => onpick(port.name)}
          >
            <span class="name">{port.name}</span>
            <span class="description"
              >{port.description || "Unknown adapter"}</span
            >
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</div>

<style>
  .ports {
    display: grid;
    gap: 0.3rem;
    font-size: 0.8rem;
    color: var(--muted);
  }

  .head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
  }

  .small {
    padding: 0.1rem 0.5rem;
    font-size: 0.75rem;
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
    display: grid;
    gap: 0.2rem;
  }

  li button {
    width: 100%;
    display: flex;
    gap: 0.6rem;
    align-items: baseline;
    text-align: left;
    padding: 0.3rem 0.5rem;
  }

  li button[aria-pressed="true"] {
    border-color: var(--accent, var(--border));
  }

  .name {
    font-weight: 600;
    color: var(--text);
    white-space: nowrap;
  }

  .description {
    overflow-wrap: anywhere;
  }

  p {
    margin: 0;
  }

  .error {
    color: var(--danger);
  }
</style>
