<script lang="ts">
  import { tick } from "svelte";
  import type {
    AgentForwardingDecision,
    AgentForwardingQuestion,
  } from "./session";

  interface Props {
    /** The question; a new question needs a new component. */
    question: AgentForwardingQuestion;
    /** False until the answer can be sent. */
    ready: boolean;
    ondecide: (decision: AgentForwardingDecision) => void;
  }

  let { question, ready, ondecide }: Props = $props();

  const where = $derived(`${question.host} port ${question.port}`);
  let card: HTMLElement;

  // The terminal has focus; take it once the buttons work. The safe answer,
  // connecting without forwarding, has it, so Enter never forwards by
  // accident.
  let focused = false;
  $effect(() => {
    if (!ready || focused) return;
    focused = true;
    tick().then(() =>
      card
        .querySelector<HTMLButtonElement>("[data-decision=withoutForwarding]")
        ?.focus(),
    );
  });

  // Escape is the default answer.
  function keydown(event: KeyboardEvent) {
    if (event.key !== "Escape" || !ready) return;
    event.preventDefault();
    event.stopPropagation();
    ondecide("withoutForwarding");
  }
</script>

<div class="overlay">
  <div
    class="card"
    bind:this={card}
    role="dialog"
    aria-label="Forward your SSH agent?"
    data-agent-question
    tabindex="-1"
    onkeydown={keydown}
  >
    <h2>Forward your SSH agent to {question.name}?</h2>
    <p>
      The workspace's settings ask to forward your SSH agent to
      <code data-agent-question-where>{where}</code>. While this session is
      open, anyone with administrator rights on that device can use the keys in
      your agent to log in as you elsewhere.
    </p>
    <p class="hint">
      Forward it only to a device you trust. Your answer stays on this computer;
      the workspace cannot give it.
    </p>
    <div class="actions">
      <button
        data-decision="withoutForwarding"
        disabled={!ready}
        onclick={() => ondecide("withoutForwarding")}
      >
        Connect without forwarding
      </button>
      <button
        data-decision="forwardThisSession"
        disabled={!ready}
        onclick={() => ondecide("forwardThisSession")}
      >
        Forward for this session
      </button>
      <button
        data-decision="alwaysForward"
        class="warning"
        disabled={!ready}
        onclick={() => ondecide("alwaysForward")}
      >
        Always forward to this host
      </button>
    </div>
  </div>
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

  .card:focus {
    outline: none;
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
    overflow-wrap: anywhere;
  }

  p {
    margin: 0;
    font-size: var(--text-sm);
  }

  .hint {
    color: var(--muted);
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

  button.warning {
    color: var(--warning);
    border-color: var(--warning);
  }
</style>
