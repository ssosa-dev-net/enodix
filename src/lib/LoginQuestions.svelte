<script lang="ts">
  import { onMount, untrack } from "svelte";
  import type { LoginPrompt } from "./session";

  interface Props {
    /** The server's questions; a new prompt needs a new component. */
    prompt: LoginPrompt;
    /** False until the answers can be sent. */
    ready: boolean;
    onanswer: (answers: string[]) => void;
    oncancel: () => void;
  }

  let { prompt, ready, onanswer, oncancel }: Props = $props();

  const questions = untrack(() => prompt.questions);
  let answers = $state(questions.map(() => ""));
  let form: HTMLFormElement;

  // The terminal has focus; take it, so the user can type at once.
  onMount(() => form.querySelector("input")?.focus());

  function submit(event: SubmitEvent) {
    event.preventDefault();
    onanswer(answers);
  }
</script>

<div class="overlay">
  <form
    class="questions"
    data-login-prompt
    bind:this={form}
    onsubmit={submit}
    aria-label="Login questions"
  >
    <h2>{prompt.name || "The server asks"}</h2>
    {#if prompt.instructions}
      <p class="instructions">{prompt.instructions}</p>
    {/if}
    {#each questions as question, index (index)}
      <label>
        {question.text.trim()}
        <input
          name={`answer-${index}`}
          type={question.echo ? "text" : "password"}
          bind:value={answers[index]}
          autocomplete="off"
          spellcheck="false"
        />
      </label>
    {/each}
    <div class="actions">
      <button type="button" onclick={oncancel}>Cancel</button>
      <button type="submit" disabled={!ready}>Send</button>
    </div>
  </form>
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

  .questions {
    width: min(22rem, 100%);
    display: grid;
    gap: var(--space-3);
    padding: var(--space-4);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
  }

  h2 {
    margin: 0;
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
  }

  .instructions {
    margin: 0;
    font-size: var(--text-sm);
    white-space: pre-line;
    overflow-wrap: anywhere;
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
</style>
