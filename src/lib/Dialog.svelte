<script lang="ts">
  import { untrack } from "svelte";
  import type { DialogRequest } from "./inventory.svelte";

  interface Props {
    request: DialogRequest;
    onclose: () => void;
  }

  let { request, onclose }: Props = $props();

  // The form's own copy of the values, filled from the request once (a new
  // request mounts a new dialog).
  let values = $state<Record<string, string>>(
    untrack(() =>
      Object.fromEntries(
        request.fields.map((field) => [field.name, field.value]),
      ),
    ),
  );
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    busy = true;
    error = await request.onconfirm({ ...values });
    busy = false;
    if (error === null) onclose();
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Escape") onclose();
  }
</script>

<svelte:window {onkeydown} />

<div
  class="backdrop"
  role="dialog"
  aria-modal="true"
  aria-labelledby="dialog-title"
>
  <form onsubmit={submit}>
    <h2 id="dialog-title">{request.title}</h2>
    {#if request.message}
      <p>{request.message}</p>
    {/if}

    {#each request.fields as field, index (field.name)}
      <label>
        {field.label}
        {#if field.options}
          <select name={field.name} bind:value={values[field.name]}>
            {#each field.options as option (option.value)}
              <option value={option.value}>{option.label}</option>
            {/each}
          </select>
        {:else}
          <!-- svelte-ignore a11y_autofocus -->
          <input
            name={field.name}
            bind:value={values[field.name]}
            autofocus={index === 0}
            autocomplete="off"
            spellcheck="false"
          />
        {/if}
      </label>
    {/each}

    {#if error}
      <p class="error" role="alert">{error}</p>
    {/if}

    <div class="actions">
      <button type="button" onclick={onclose}>Cancel</button>
      <!-- svelte-ignore a11y_autofocus -->
      <button
        type="submit"
        class:danger={request.danger}
        disabled={busy}
        autofocus={request.fields.length === 0}>{request.confirm}</button
      >
    </div>
  </form>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 20;
    display: grid;
    place-items: center;
    padding: var(--space-4);
    background: var(--backdrop);
  }

  form {
    width: min(24rem, 100%);
    display: grid;
    gap: var(--space-3);
    padding: var(--space-4);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow);
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
  }

  p {
    margin: 0;
    color: var(--muted);
    font-size: var(--text-sm);
    overflow-wrap: anywhere;
  }

  .error {
    color: var(--danger);
  }

  label {
    display: grid;
    gap: var(--space-1);
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }

  .actions button[type="submit"] {
    padding: 0 var(--space-4);
  }

  .danger,
  .danger:hover:not(:disabled) {
    color: var(--on-danger);
    background: var(--danger-fill);
    border-color: var(--danger-fill);
  }
</style>
