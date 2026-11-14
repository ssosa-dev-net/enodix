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
    padding: 1rem;
    background: rgb(0 0 0 / 0.45);
  }

  form {
    width: min(24rem, 100%);
    display: grid;
    gap: 0.8rem;
    padding: 1.2rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    box-shadow: 0 10px 30px rgb(0 0 0 / 0.4);
  }

  h2 {
    margin: 0;
    font-size: 1.05rem;
    font-weight: 600;
  }

  p {
    margin: 0;
    color: var(--muted);
    font-size: 0.85rem;
    overflow-wrap: anywhere;
  }

  .error {
    color: var(--danger);
  }

  label {
    display: grid;
    gap: 0.3rem;
    font-size: 0.8rem;
    color: var(--muted);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.6rem;
  }

  .actions button[type="submit"] {
    padding: 0.35rem 1.1rem;
  }

  .danger {
    background: #b62324;
    border-color: #b62324;
  }
</style>
