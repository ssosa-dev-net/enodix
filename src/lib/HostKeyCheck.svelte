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
    {#if question.certificate}
      <p class="certificate" data-host-certificate>
        The device also presented a host certificate, which was not accepted:
        {question.certificate}.
      </p>
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
    padding: var(--space-6);
    background: var(--backdrop);
    overflow: auto;
  }

  .card {
    width: min(30rem, 100%);
    display: grid;
    gap: var(--space-3);
    padding: var(--space-4);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
  }

  .card.changed {
    border: 2px solid var(--danger);
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }

  .changed h2 {
    color: var(--danger);
  }

  p {
    margin: 0;
    font-size: var(--text-sm);
  }

  .certificate {
    color: var(--muted);
  }

  dl {
    margin: 0;
    display: grid;
    gap: var(--space-1);
    font-size: var(--text-xs);
  }

  dt {
    color: var(--muted);
  }

  dd {
    margin: 0 0 var(--space-1);
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    align-items: baseline;
  }

  code {
    font-family: var(--font-mono);
    overflow-wrap: anywhere;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    justify-content: flex-end;
    gap: var(--space-2);
  }

  button.danger {
    color: var(--danger);
    border-color: var(--danger);
  }
</style>
