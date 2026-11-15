<script lang="ts">
  import Lock from "@lucide/svelte/icons/lock";
  import LockOpen from "@lucide/svelte/icons/lock-open";
  import KeyRound from "@lucide/svelte/icons/key-round";
  import type { VaultStatus } from "./vault";

  interface Props {
    /** The vault's state; null until known. */
    vault: VaultStatus | null;
    /** The active session's name and terminal size; null: none shown. */
    session: { name: string; cols: number; rows: number } | null;
  }

  let { vault, session }: Props = $props();

  const VAULT_TEXT: Record<VaultStatus, string> = {
    off: "Vault off",
    locked: "Vault locked",
    unlocked: "Vault unlocked",
  };

  const VAULT_TITLE: Record<VaultStatus, string> = {
    off: "The vault is off (Preferences turns it on)",
    locked: "The vault is locked",
    unlocked: "Credentials are kept in the vault, unlocked for this run",
  };
</script>

<footer class="status-line" data-status-line>
  {#if vault}
    <span class="item" data-status-vault={vault} title={VAULT_TITLE[vault]}>
      {#if vault === "unlocked"}
        <LockOpen size={12} aria-hidden="true" />
      {:else if vault === "locked"}
        <Lock size={12} aria-hidden="true" />
      {:else}
        <KeyRound size={12} aria-hidden="true" />
      {/if}
      {VAULT_TEXT[vault]}
    </span>
  {/if}
  <span class="spacer"></span>
  {#if session}
    <span class="item name" data-status-session>{session.name}</span>
    <span
      class="item"
      data-status-size
      data-cols={session.cols}
      data-rows={session.rows}
      title="The active terminal's columns and rows"
      >{session.cols} × {session.rows}</span
    >
  {/if}
</footer>

<style>
  .status-line {
    box-sizing: border-box;
    height: 22px;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: 0 var(--space-3);
    overflow: hidden;
    background: var(--surface);
    border-top: 1px solid var(--border);
    color: var(--muted);
    font-size: var(--text-2xs);
    white-space: nowrap;
  }

  .item {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1);
  }

  .name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .spacer {
    flex: 1;
  }
</style>
