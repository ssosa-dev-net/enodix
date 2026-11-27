<script lang="ts">
  import { onMount } from "svelte";
  import {
    baseName,
    MODEM_PROTOCOLS,
    pickFilesToSend,
    pickReceiveTarget,
    type ModemProtocol,
    type TransferAction,
  } from "./modem";

  interface Props {
    direction: "send" | "receive";
    /** The session's name, such as its host's. */
    target: string;
    /** Starts the transfer; resolves to why it could not, or null. */
    onstart: (
      protocol: ModemProtocol,
      action: TransferAction,
    ) => Promise<string | null>;
    oncancel: () => void;
  }

  let { direction, target, onstart, oncancel }: Props = $props();

  let protocol = $state<ModemProtocol>("xmodem1k");
  let files = $state<string[]>([]);
  let receiveTo = $state<string | null>(null);
  let problem = $state<string | null>(null);
  let starting = $state(false);
  let first: HTMLSelectElement;

  const sending = $derived(direction === "send");
  const ready = $derived(
    sending
      ? files.length === 1 || (files.length > 1 && protocol === "ymodem")
      : receiveTo !== null,
  );

  onMount(() => first.focus());

  function changeProtocol() {
    // A file name chosen for XMODEM is no folder for YMODEM, and back.
    receiveTo = null;
    if (protocol !== "ymodem" && files.length > 1) files = files.slice(0, 1);
  }

  async function choose() {
    problem = null;
    if (sending) {
      const picked = await pickFilesToSend(protocol === "ymodem");
      if (picked.length > 0) files = picked;
    } else {
      receiveTo = (await pickReceiveTarget(protocol)) ?? receiveTo;
    }
  }

  async function start() {
    if (!ready || starting) return;
    starting = true;
    const action: TransferAction = sending
      ? { kind: "send", path: files }
      : { kind: "receive", path: receiveTo ?? "" };
    problem = await onstart(protocol, action);
    starting = false;
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      oncancel();
    }
  }
</script>

<div
  class="overlay"
  role="dialog"
  aria-labelledby="modem-transfer-title"
  tabindex="-1"
  {onkeydown}
>
  <form
    class="card"
    data-modem-form={direction}
    onsubmit={(event) => {
      event.preventDefault();
      void start();
    }}
  >
    <h2 id="modem-transfer-title">
      {sending ? "Send a file to" : "Receive a file from"}
      {target}
    </h2>
    <label class="field">
      <span>Protocol</span>
      <select
        bind:this={first}
        bind:value={protocol}
        onchange={changeProtocol}
        data-modem-protocol
      >
        {#each MODEM_PROTOCOLS as choice (choice.id)}
          <option value={choice.id}>{choice.name}</option>
        {/each}
      </select>
      <span class="muted"
        >{MODEM_PROTOCOLS.find((choice) => choice.id === protocol)?.hint}</span
      >
    </label>
    <div class="field">
      <span
        >{sending
          ? protocol === "ymodem"
            ? "Files"
            : "File"
          : protocol === "ymodem"
            ? "Into the folder"
            : "Save as"}</span
      >
      <div class="pick">
        <span class="picked" data-modem-picked>
          {#if sending}
            {files.length === 0
              ? "None chosen"
              : files.map(baseName).join(", ")}
          {:else}
            {receiveTo ?? "None chosen"}
          {/if}
        </span>
        <button type="button" data-modem-choose onclick={choose}>Choose…</button
        >
      </div>
    </div>
    <p class="muted">
      {#if sending}
        Start the device's receive first (for example <code>xmodem</code> or
        <code>copy xmodem: flash:</code> in ROMMON), then Start here.
      {:else}
        Start here, then the device's send. The file names a YMODEM device gives
        never replace a file already in the folder.
      {/if}
      Until it ends, the terminal shows nothing and holds what you type.
    </p>
    {#if problem}
      <p class="problem" role="alert" data-modem-problem>{problem}</p>
    {/if}
    <div class="actions">
      <button type="button" data-modem="cancel" onclick={oncancel}
        >Cancel</button
      >
      <button type="submit" data-modem="start" disabled={!ready || starting}
        >Start</button
      >
    </div>
  </form>
</div>

<style>
  .overlay {
    position: absolute;
    inset: 0;
    z-index: 3;
    display: grid;
    place-items: center;
    padding: var(--space-4);
    background: var(--backdrop);
    overflow: auto;
  }

  .card {
    width: min(32rem, 100%);
    display: grid;
    gap: var(--space-2-5);
    padding: var(--space-4);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }

  .field {
    display: grid;
    gap: var(--space-1);
    font-size: var(--text-sm);
  }

  .pick {
    display: flex;
    align-items: center;
    gap: var(--space-2);
  }

  .picked {
    flex: 1 1 auto;
    min-width: 0;
    overflow-wrap: anywhere;
    font-family: var(--font-mono);
    font-size: var(--text-xs);
  }

  .muted {
    margin: 0;
    color: var(--muted);
    font-size: var(--text-xs);
  }

  .problem {
    margin: 0;
    color: var(--danger);
    font-size: var(--text-sm);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
