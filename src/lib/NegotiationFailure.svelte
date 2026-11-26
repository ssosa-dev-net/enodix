<script lang="ts">
  import type { AlgorithmKind, NegotiationFailure } from "./session";

  interface Props {
    failure: NegotiationFailure;
    ondismiss: () => void;
    /** Opens the host through the system's ssh client; without it, the
     * card does not offer that. */
    onsystemssh?: () => void;
  }

  let { failure, ondismiss, onsystemssh }: Props = $props();

  const kinds: { kind: AlgorithmKind; label: string; name: string }[] = [
    { kind: "keyExchange", label: "Key exchange", name: "key-exchange" },
    { kind: "hostKey", label: "Host key", name: "host-key" },
    { kind: "cipher", label: "Cipher", name: "cipher" },
    { kind: "mac", label: "MAC", name: "MAC" },
  ];
  const nameOf = (kind: AlgorithmKind) =>
    kinds.find((k) => k.kind === kind)?.name ?? kind;

  const who = $derived(
    failure.jumpHost ? `the jump host ${failure.jumpHost}` : "the device",
  );
  const advice = $derived(
    failure.legacyCryptoWouldHelp
      ? `Turning on Legacy crypto would help: set it in the settings of ${
          failure.jumpHost ? `the jump host ${failure.jumpHost}` : "this host"
        } (or of its folder) and connect again.`
      : failure.unsupported
        ? `${failure.legacyCrypto ? "Even with legacy crypto on, none" : "Legacy crypto would not help: none"} of the ${nameOf(failure.unsupported)} algorithms it offers is supported. To reach it anyway, use telnet or serial, or the system's ssh client in a local shell (insecure: it is not checked or logged by the app).`
        : "",
  );
</script>

<div class="overlay">
  <section
    class="card"
    role="alert"
    data-negotiation-failure={failure.legacyCryptoWouldHelp
      ? "legacy-would-help"
      : failure.unsupported
        ? "unsupported"
        : "no-advice"}
  >
    <h2>
      No {nameOf(failure.kind)} algorithm in common with {who}
    </h2>
    {#if advice}
      <p data-advice>{advice}</p>
    {/if}
    <dl>
      {#each kinds as { kind, label } (kind)}
        <dt class:failed={kind === failure.kind}>{label} offered</dt>
        <dd data-offered={kind}>
          {#each failure.offered[kind] as name, i (i)}
            <code>{name}</code>
          {:else}
            <span class="unknown">unknown</span>
          {/each}
        </dd>
      {/each}
    </dl>
    <div class="actions">
      {#if onsystemssh && failure.unsupported}
        <button
          data-system-ssh
          title="Opens a local shell running the system's ssh with the outdated algorithms allowed. Not checked or logged by the app."
          onclick={onsystemssh}>Open with the system ssh (insecure)</button
        >
      {/if}
      <button onclick={ondismiss}>New connection</button>
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
    width: min(34rem, 100%);
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
    overflow-wrap: anywhere;
  }

  p {
    margin: 0;
    font-size: var(--text-sm);
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

  dt.failed {
    color: var(--warning);
  }

  dd {
    margin: 0 0 var(--space-1);
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-1-5) var(--space-2);
  }

  code {
    font-family: var(--font-mono);
    overflow-wrap: anywhere;
  }

  .unknown {
    color: var(--muted);
    font-style: italic;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    flex-wrap: wrap;
    gap: var(--space-2);
  }
</style>
