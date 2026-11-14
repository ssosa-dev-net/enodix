<script lang="ts">
  import {
    quitApp,
    unlockVault,
    vaultFailure,
    type VaultFailure,
  } from "./vault";

  interface Props {
    /** The app's name, for the title. */
    name: string;
    onunlocked: () => void;
  }

  let { name, onunlocked }: Props = $props();

  let password = $state("");
  let failure = $state<VaultFailure | null>(null);
  let busy = $state(false);
  let input = $state<HTMLInputElement>();

  $effect(() => {
    input?.focus();
  });

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    if (busy || password === "") return;
    busy = true;
    try {
      await unlockVault(password);
      password = "";
      onunlocked();
    } catch (error) {
      failure = vaultFailure(error);
      password = "";
      input?.focus();
    } finally {
      busy = false;
    }
  }
</script>

<div
  class="screen"
  role="dialog"
  aria-modal="true"
  aria-labelledby="unlock-title"
  data-unlock-screen
>
  <form onsubmit={submit}>
    <h1 id="unlock-title">{name}</h1>
    <p>Your credential profiles and secrets are in the vault.</p>
    <label>
      Master password
      <input
        bind:this={input}
        bind:value={password}
        type="password"
        name="vault-unlock-password"
        autocomplete="off"
        disabled={busy}
      />
    </label>
    {#if failure}
      <p class="error" role="alert" data-unlock-error={failure.kind}>
        {#if failure.kind === "wrongPassword"}
          That is not the master password. Try again.
        {:else if failure.kind === "damaged"}
          The vault cannot be opened with any password: {failure.message}.
        {:else}
          {failure.message}
        {/if}
      </p>
    {/if}
    <div class="actions">
      <button type="button" data-unlock="quit" onclick={() => void quitApp()}
        >Quit</button
      >
      <button
        type="submit"
        class="primary"
        data-unlock="unlock"
        disabled={busy || password === ""}
        >{busy ? "Unlocking…" : "Unlock"}</button
      >
    </div>
  </form>
</div>

<style>
  .screen {
    position: fixed;
    inset: 0;
    z-index: 100;
    display: grid;
    place-items: center;
    padding: 1rem;
    background: var(--bg);
  }

  form {
    width: min(24rem, 100%);
    display: grid;
    gap: 0.9rem;
    padding: 1.4rem;
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  h1 {
    margin: 0;
    font-size: 1.1rem;
    font-weight: 600;
  }

  p {
    margin: 0;
    font-size: 0.85rem;
  }

  label {
    display: grid;
    gap: 0.3rem;
    font-size: 0.85rem;
  }

  .error {
    color: var(--danger);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
  }
</style>
