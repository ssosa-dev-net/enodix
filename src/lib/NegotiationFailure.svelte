<script lang="ts">
  import type { AlgorithmKind, NegotiationFailure } from "./session";

  interface Props {
    failure: NegotiationFailure;
    ondismiss: () => void;
  }

  let { failure, ondismiss }: Props = $props();

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
        ? `${failure.legacyCrypto ? "Even with legacy crypto on, none" : "Legacy crypto would not help: none"} of the ${nameOf(failure.unsupported)} algorithms it offers is supported.`
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
    padding: 1.5rem;
    background: rgb(0 0 0 / 0.45);
    overflow: auto;
  }

  .card {
    width: min(34rem, 100%);
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
    overflow-wrap: anywhere;
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

  dt.failed {
    color: var(--warning);
  }

  dd {
    margin: 0 0 0.3rem;
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem 0.6rem;
  }

  code {
    font-family: "Cascadia Mono", Consolas, "Courier New", monospace;
    overflow-wrap: anywhere;
  }

  .unknown {
    color: var(--muted);
    font-style: italic;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
  }
</style>
