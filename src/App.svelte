<script lang="ts">
  import CredentialsPrompt from "./lib/CredentialsPrompt.svelte";
  import QuickConnect from "./lib/QuickConnect.svelte";
  import TerminalSession from "./lib/TerminalSession.svelte";
  import WorkspacePanel from "./lib/WorkspacePanel.svelte";
  import { appName, type ConnectTarget } from "./lib/session";
  import {
    openWorkspace,
    pickWorkspaceFolder,
    restoreWorkspace,
    type Host,
    type WorkspaceView,
  } from "./lib/workspace";

  /** What the centre of the window shows. */
  type View =
    | { kind: "quick" }
    | { kind: "prompt"; host: Host }
    | { kind: "session"; target: ConnectTarget; title: string; key: number };

  let name = $state("");
  let workspace = $state<WorkspaceView | null>(null);
  let workspaceError = $state<string | null>(null);
  let view = $state<View>({ kind: "quick" });
  // A new key per session, so that connecting again replaces the terminal.
  let sessions = 0;

  appName().then((value) => {
    name = value;
    document.title = value;
  });

  restoreWorkspace().then(
    (restored) => (workspace = restored),
    (error) =>
      (workspaceError = `Could not reopen the last workspace: ${error}`),
  );

  async function open() {
    const folder = await pickWorkspaceFolder();
    if (folder === null) return;
    try {
      workspace = await openWorkspace(folder);
      workspaceError = null;
    } catch (error) {
      workspaceError = String(error);
    }
  }

  function connect(target: ConnectTarget, title: string) {
    view = { kind: "session", target, title, key: ++sessions };
  }
</script>

<div class="layout">
  <WorkspacePanel
    {workspace}
    error={workspaceError}
    onopen={open}
    onconnect={(host) => (view = { kind: "prompt", host })}
  />

  <section>
    {#if view.kind === "session"}
      {#key view.key}
        <TerminalSession
          target={view.target}
          title={view.title}
          ondismiss={() => (view = { kind: "quick" })}
        />
      {/key}
    {:else if view.kind === "prompt"}
      {@const host = view.host}
      {#key host.id}
        <CredentialsPrompt
          {host}
          oncancel={() => (view = { kind: "quick" })}
          onconnect={({ username, password }) =>
            connect(
              { kind: "host", id: host.id, username, password },
              `${username}@${host.name}`,
            )}
        />
      {/key}
    {:else}
      <QuickConnect
        {name}
        onconnect={(target) =>
          connect(
            { kind: "quick", ...target },
            `${target.username}@${target.host}:${target.port}`,
          )}
      />
    {/if}
  </section>
</div>

<style>
  .layout {
    height: 100%;
    display: grid;
    grid-template-columns: minmax(14rem, 18rem) 1fr;
  }

  section {
    min-width: 0;
    min-height: 0;
  }
</style>
