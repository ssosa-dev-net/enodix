<script lang="ts">
  import { tick } from "svelte";
  import type { HostKeyDecision, HostKeyQuestion } from "./session";

  interface Props {
    /** The key to decide on; a new question needs a new component. */
    question: HostKeyQuestion;
    /** False until the decision can be sent. */
    ready: boolean;
    ondecide: (decision: HostKeyDecision) => void;
  }

  let { question, ready, ondecide }: Props = $props();

  const changed = $derived(question.stored.length > 0);
  const where = $derived(`${question.host}:${question.port}`);
  let card: HTMLElement;

  // The terminal has focus; take it once the buttons work. On a changed key
  // the safe choice, Cancel, has it, so Enter never accepts a changed key
  // by accident.
  let focused = false;
  $effect(() => {
    if (!ready || focused) return;
    focused = true;
    const selector = changed
      ? "[data-decision=cancel]"
      : "[data-decision=trustAndRemember]";
    // After the buttons are enabled in the DOM.
    tick().then(() => card.querySelector<HTMLButtonElement>(selector)?.focus());
  });
</script>

<div class="overlay">
  <section
    class="card"
    class:changed
    bind:this={card}
    role={changed ? "alert" : "dialog"}
    aria-label={changed ? "Host key changed" : "Unknown host key"}
    data-host-key={changed ? "changed" : "first-use"}
  >
    {#if changed}
      <h2>Warning: the host key of {where} has changed</h2>
      <p>
        The device presents a different key from the one trusted before. Someone
        may be intercepting the connection, or the device was replaced or given
        a new key. Connect only if you know why it changed.
      </p>
      <dl>
        <dt>Trusted until now</dt>
        {#each question.stored as stored (stored.fingerprint)}
          <dd>
            <span class="algorithm">{stored.algorithm}</span>
            <code data-fingerprint="stored">{stored.fingerprint}</code>
          </dd>
        {/each}
        <dt>Presented now</dt>
        <dd>
          <span class="algorithm">{question.key.algorithm}</span>
          <code data-fingerprint="presented">{question.key.fingerprint}</code>
        </dd>
      </dl>
    {:else}
      <h2>First connection to {where}</h2>
      <p>
        This device's host key is not known yet. Check its fingerprint with the
        device before trusting it.
      </p>
      <dl>
        <dt>Key type</dt>
        <dd><span class="algorithm">{question.key.algorithm}</span></dd>
        <dt>Fingerprint</dt>
        <dd>
          <code data-fingerprint="presented">{question.key.fingerprint}</code>
        </dd>
      </dl>
    {/if}
    <div class="actions">
      <button
        data-decision="cancel"
        disabled={!ready}
        onclick={() => ondecide("cancel")}
      >
        Cancel
      </button>
      <button
        data-decision="trustOnce"
        disabled={!ready}
        onclick={() => ondecide("trustOnce")}
      >
        {changed ? "Connect once" : "Trust once"}
      </button>
      <button
        data-decision="trustAndRemember"
        class:danger={changed}
        disabled={!ready}
        onclick={() => ondecide("trustAndRemember")}
      >
        {changed ? "Replace the stored key" : "Trust and remember"}
      </button>
    </div>
  </section>
</div>

<style>
  .overlay {
    position: absolute;
    inset: 0;
    display: grid;
    place-items: center;
    padding: 1.5rem;
    background: rgb(0 0 0 / 0.45);
    overflow: auto;
  }

  .card {
    width: min(30rem, 100%);
    display: grid;
    gap: 0.8rem;
    padding: 1.2rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  .card.changed {
    border: 2px solid var(--danger);
  }

  h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
    overflow-wrap: anywhere;
  }

  .changed h2 {
    color: var(--danger);
  }

  p {
    margin: 0;
    font-size: 0.85rem;
  }

  dl {
    margin: 0;
    display: grid;
    gap: 0.3rem;
    font-size: 0.8rem;
  }

  dt {
    color: var(--muted);
  }

  dd {
    margin: 0 0 0.3rem;
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
    align-items: baseline;
  }

  code {
    font-family: "Cascadia Mono", Consolas, "Courier New", monospace;
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: 0.6rem;
  }

  button.danger {
    color: var(--danger);
    border-color: var(--danger);
  }
</style>
