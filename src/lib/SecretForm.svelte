<script lang="ts">
  import { untrack } from "svelte";
  import { saveSecret } from "./profiles.svelte";

  interface Props {
    /** The secret whose value is replaced; null adds one. */
    replacing: string | null;
    onsaved: () => void;
    oncancel: () => void;
  }

  let { replacing, onsaved, oncancel }: Props = $props();

  const fixed = untrack(() => replacing);
  let name = $state(fixed ?? "");
  // Kept only until saved: the form goes away then.
  let value = $state("");
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    busy = true;
    error = await saveSecret(fixed ?? name, value, fixed !== null);
    busy = false;
    if (error === null) {
      value = "";
      onsaved();
    }
  }
</script>

<form data-secret-form onsubmit={submit}>
  {#if fixed === null}
    <label>
      Secret name
      <!-- svelte-ignore a11y_autofocus -->
      <input
        name="secret-name"
        bind:value={name}
        required
        autofocus
        autocomplete="off"
        spellcheck="false"
        pattern="[A-Za-z0-9._\-]+"
        title="Letters, digits, -, _ and ."
      />
    </label>
  {/if}
  <label>
    {fixed === null ? "Value" : "New value"}
    <!-- svelte-ignore a11y_autofocus -->
    <input
      name="secret-value"
      type="password"
      bind:value
      required
      autofocus={fixed !== null}
      autocomplete="off"
    />
  </label>
  <p class="note">
    Kept for this workspace on this computer (in its keychain, or in the vault).
    The value is not shown again after saving.
  </p>

  {#if error}
    <p class="error" role="alert" data-secret-error>{error}</p>
  {/if}

  <div class="actions">
    <button type="button" data-secret-action="cancel" onclick={oncancel}
      >Cancel</button
    >
    <button type="submit" data-secret-action="save" disabled={busy}>Save</button
    >
  </div>
</form>

<style>
  form {
    display: grid;
    gap: 0.8rem;
  }

  label {
    display: grid;
    gap: 0.3rem;
    font-size: 0.8rem;
    color: var(--muted);
  }

  .note {
    margin: 0;
    font-size: 0.8rem;
    color: var(--muted);
  }

  .error {
    margin: 0;
    font-size: 0.85rem;
    color: var(--danger);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.6rem;
  }
</style>
