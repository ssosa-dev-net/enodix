<script lang="ts">
  // The session header's marker while the session forwards ports (#21),
  // and the list of every open forward it opens, to stop any of them.
  import { onMount } from "svelte";
  import Button from "./Button.svelte";
  import {
    forwardAddress,
    forwards,
    forwardsOf,
    followForwards,
    stopForward,
    type ForwardStatus,
  } from "./forwards.svelte";

  interface Props {
    /** The session shown; `null` until it is open. */
    sessionId: number | null;
  }

  let { sessionId }: Props = $props();

  onMount(() => {
    void followForwards();
  });

  let own = $derived(forwardsOf(sessionId));
  let listening = $derived(own.filter((f) => f.state === "listening"));
  let failing = $derived(own.some((f) => f.state === "failed"));
  let open = $state(false);
  let error = $state<string | null>(null);
  let panel = $state<HTMLElement | null>(null);
  let button = $state<HTMLButtonElement | null>(null);

  // This session's first, then the others.
  let sessions = $derived(
    [...forwards.sessions].sort(
      (a, b) => Number(b.id === sessionId) - Number(a.id === sessionId),
    ),
  );

  function describe(status: ForwardStatus): string {
    if (status.state === "listening") {
      const where = forwardAddress(status);
      const kind = status.spec.startsWith("D")
        ? `SOCKS5 proxy on ${where}`
        : `${where} reaches ${status.spec.split(":").slice(-2).join(" port ")}`;
      return status.lastFailure
        ? `${kind}. Last failure: ${status.lastFailure}`
        : kind;
    }
    if (status.state === "failed") return `Not open: ${status.reason}`;
    return "Stopped (opens again when the session reconnects)";
  }

  let markerTitle = $derived(
    own.map((f) => `${f.name} (${f.spec}): ${describe(f)}`).join("\n") +
      "\nAny program on this computer can use a forwarded port while it is open.",
  );

  async function stop(id: number, name: string) {
    error = null;
    try {
      await stopForward(id, name);
    } catch (reason) {
      error = `Cannot stop ${name}: ${reason}`;
    }
  }

  function onpointerdown(event: PointerEvent) {
    if (!open) return;
    const target = event.target as Node;
    if (panel?.contains(target) || button?.contains(target)) return;
    open = false;
  }

  function onkeydown(event: KeyboardEvent) {
    if (open && event.key === "Escape") {
      open = false;
      button?.focus();
    }
  }
</script>

<svelte:window {onpointerdown} {onkeydown} />

{#if listening.length > 0 || failing}
  <span class="holder">
    <button
      class="marker forwards"
      class:failing={listening.length === 0}
      data-forwards-marker={listening.length}
      title={markerTitle}
      aria-haspopup="dialog"
      aria-expanded={open}
      bind:this={button}
      onclick={() => (open = !open)}
    >
      {listening.length === 0
        ? "Forwards not open"
        : `Forwarding ${listening.length}`}
    </button>
    {#if open}
      <div
        class="panel"
        role="dialog"
        aria-label="Open port forwards"
        data-forwards-panel
        bind:this={panel}
      >
        <h3>Port forwards</h3>
        <p class="note">
          Any program on this computer can use a forwarded port while it is
          open. Forwards are set per host in the workspace (<code>forwards</code
          >).
        </p>
        {#each sessions as session (session.id)}
          <section>
            <h4>
              {session.name ?? `Session ${session.id}`}{session.id === sessionId
                ? " (this session)"
                : ""}
            </h4>
            <ul>
              {#each session.forwards as forward (forward.name)}
                <li
                  data-forward-row={forward.name}
                  data-forward-state={forward.state}
                >
                  <span class="name">{forward.name}</span>
                  <code class="spec">{forward.spec}</code>
                  <span class="state" class:bad={forward.state === "failed"}
                    >{describe(forward)}</span
                  >
                  {#if forward.state === "listening"}
                    <Button
                      size="compact"
                      data-forward-stop={forward.name}
                      onclick={() => stop(session.id, forward.name)}
                      >Stop</Button
                    >
                  {/if}
                </li>
              {/each}
            </ul>
          </section>
        {/each}
        {#if error}
          <p class="error" role="alert">{error}</p>
        {/if}
      </div>
    {/if}
  </span>
{/if}

<style>
  .holder {
    position: relative;
    flex: none;
  }

  .marker {
    padding: 0 var(--space-1-5);
    font: inherit;
    line-height: 18px;
    color: var(--accent);
    background: transparent;
    border: 1px solid currentColor;
    border-radius: var(--radius-full);
    cursor: pointer;
  }

  .marker.failing {
    color: var(--warning);
  }

  .panel {
    position: absolute;
    top: calc(100% + var(--space-1));
    right: 0;
    z-index: 20;
    width: min(32rem, 80vw);
    max-height: 60vh;
    overflow: auto;
    padding: var(--space-3);
    font-size: var(--text-sm);
    color: var(--text);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-small);
  }

  h3 {
    margin: 0 0 var(--space-1);
    font-size: var(--text-md);
    font-weight: var(--weight-semibold);
  }

  h4 {
    margin: var(--space-3) 0 var(--space-1);
    font-size: var(--text-sm);
    font-weight: var(--weight-medium);
    color: var(--muted);
  }

  .note {
    margin: 0;
    font-size: var(--text-xs);
    color: var(--muted);
  }

  ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  li {
    display: grid;
    grid-template-columns: auto auto 1fr auto;
    gap: var(--space-2);
    align-items: center;
    padding: var(--space-1) 0;
    border-top: 1px solid var(--border);
  }

  .name {
    font-weight: var(--weight-medium);
  }

  code {
    font-family: var(--font-mono);
    font-size: var(--text-xs);
  }

  .state {
    min-width: 0;
    color: var(--muted);
    overflow-wrap: anywhere;
  }

  .state.bad,
  .error {
    color: var(--danger);
  }

  .error {
    margin: var(--space-2) 0 0;
  }
</style>
