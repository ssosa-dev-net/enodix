<script lang="ts">
  import { onMount } from "svelte";
  import { ENVIRONMENT_NAMES } from "./paste";
  import type { Environment } from "./workspace";

  /** A session of the multi-send group the paste goes to. */
  export interface PasteMember {
    name: string;
    environment: Environment | null;
    /** Milliseconds between its lines; 0: none. */
    lineDelayMs: number;
    /** What it gets, if not the text shown (a snippet filled for it). */
    text: string | null;
  }

  interface Props {
    /** The text as it would be sent. */
    text: string;
    lineCount: number;
    /** The session's name, such as its host's. */
    target: string;
    environment: Environment | null;
    /** Milliseconds between lines; 0: none. */
    lineDelayMs: number;
    /**
     * For a paste to the multi-send group (#36): every member it goes to,
     * each with its own delay; `environment` is then the strictest one.
     */
    members?: PasteMember[];
    onsend: () => void;
    oncancel: () => void;
  }

  let {
    text,
    lineCount,
    target,
    environment,
    lineDelayMs,
    members,
    onsend,
    oncancel,
  }: Props = $props();

  let cancel: HTMLButtonElement;
  let send: HTMLButtonElement;

  // Taken from the terminal, which would otherwise keep the keys. On a
  // production host Enter by reflex cancels.
  onMount(() => (environment === "production" ? cancel : send).focus());

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
  role="alertdialog"
  aria-modal="true"
  aria-labelledby="paste-confirm-title"
  tabindex="-1"
  {onkeydown}
>
  <form
    class="card"
    class:production={environment === "production"}
    data-paste-confirm={environment ?? "none"}
    data-paste-members={members?.length}
    onsubmit={(event) => {
      event.preventDefault();
      onsend();
    }}
  >
    <h2 id="paste-confirm-title">
      Paste <span data-paste-lines={lineCount}
        >{lineCount} {lineCount === 1 ? "line" : "lines"}</span
      >
      into {members
        ? `${members.length} ${members.length === 1 ? "session" : "sessions"} of the multi-send group`
        : target}?
    </h2>
    {#if environment === "production" && members}
      <p class="warning" data-paste-production>
        The group includes production devices. Check that they are the right
        ones.
      </p>
    {:else if environment === "production"}
      <p class="warning" data-paste-production>
        This is a {ENVIRONMENT_NAMES[environment].toLowerCase()} device. Check that
        it is the right one.
      </p>
    {/if}
    <pre data-paste-text>{text}</pre>
    {#if members}
      <ul class="members">
        {#each members as member, index (index)}
          <li
            data-paste-member={member.name}
            data-environment={member.environment ?? undefined}
          >
            {member.name}{#if member.environment}<span class="environment"
                >{ENVIRONMENT_NAMES[member.environment]}</span
              >{/if}{#if member.lineDelayMs > 0}<span class="muted"
                >, lines {member.lineDelayMs} ms apart</span
              >{/if}{#if member.text !== null}<pre
                class="member-text"
                data-paste-member-text>{member.text}</pre>{/if}
          </li>
        {/each}
      </ul>
    {:else if lineDelayMs > 0}
      <p class="muted">Sent one line at a time, {lineDelayMs} ms apart.</p>
    {/if}
    <div class="actions">
      <button
        type="button"
        data-paste="cancel"
        bind:this={cancel}
        onclick={oncancel}>Cancel</button
      >
      <button type="submit" data-paste="send" bind:this={send}>Paste</button>
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
    width: min(36rem, 100%);
    max-height: 100%;
    display: grid;
    grid-template-rows: auto auto minmax(0, 1fr) auto auto;
    gap: var(--space-2-5);
    padding: var(--space-4);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
  }

  .card.production {
    border: 2px solid var(--env-production);
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

  .warning {
    color: var(--env-production);
    font-weight: var(--weight-semibold);
  }

  .muted {
    color: var(--muted);
  }

  .members {
    margin: 0;
    padding-left: var(--space-4);
    max-height: 8rem;
    overflow: auto;
    font-size: var(--text-sm);
  }

  .member-text {
    min-height: 0;
    max-height: 5rem;
    margin-top: var(--space-0-5);
  }

  .environment {
    margin-left: var(--space-1-5);
    font-size: var(--text-xs);
    color: var(--muted);
  }

  .members li[data-environment="production"] .environment {
    color: var(--env-production);
    font-weight: var(--weight-semibold);
  }

  pre {
    margin: 0;
    min-height: 2.5rem;
    max-height: 16rem;
    overflow: auto;
    padding: var(--space-2) var(--space-2);
    background: var(--terminal);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-family: var(--font-mono);
    font-size: var(--text-xs);
    white-space: pre;
    tab-size: 8;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-2);
  }
</style>
