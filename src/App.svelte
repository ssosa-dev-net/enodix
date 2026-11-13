<script lang="ts">
  import QuickConnect from "./lib/QuickConnect.svelte";
  import TerminalSession from "./lib/TerminalSession.svelte";
  import { appName, type QuickConnectTarget } from "./lib/session";

  let name = $state("");
  let target = $state<QuickConnectTarget | null>(null);

  appName().then((value) => {
    name = value;
    document.title = value;
  });
</script>

{#if target}
  <TerminalSession {target} ondismiss={() => (target = null)} />
{:else}
  <QuickConnect {name} onconnect={(value) => (target = value)} />
{/if}
