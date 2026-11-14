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
    padding: 1.5rem;
    background: rgb(0 0 0 / 0.45);
    overflow: auto;
  }

  .questions {
    width: min(22rem, 100%);
    display: grid;
    gap: 0.8rem;
    padding: 1.2rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  h2 {
    margin: 0;
    font-size: 1rem;
    font-weight: 600;
  }

  .instructions {
    margin: 0;
    font-size: 0.85rem;
    white-space: pre-line;
    overflow-wrap: anywhere;
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
</style>
