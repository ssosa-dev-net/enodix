<script lang="ts">
  import CommandPalette from "./lib/CommandPalette.svelte";
  import CredentialsPrompt from "./lib/CredentialsPrompt.svelte";
  import Dialog from "./lib/Dialog.svelte";
  import FolderDetails from "./lib/FolderDetails.svelte";
  import HostDetails from "./lib/HostDetails.svelte";
  import QuickConnect from "./lib/QuickConnect.svelte";
  import TerminalSession from "./lib/TerminalSession.svelte";
  import WorkspacePanel from "./lib/WorkspacePanel.svelte";
  import {
    hostRole,
    inventory,
    selectedFolder,
    selectedHost,
    setWorkspace,
    workspaceChanged,
  } from "./lib/inventory.svelte";
  import { appName, type ConnectTarget } from "./lib/session";
  import {
    openWorkspace,
    pickWorkspaceFolder,
    onWorkspaceChanged,
    restoreWorkspace,
    type Host,
  } from "./lib/workspace";

  /** What the centre of the window shows. */
  type View =
    | { kind: "quick" }
    | { kind: "prompt"; host: Host }
    | { kind: "session"; target: ConnectTarget; title: string; key: number };

  let name = $state("");
  let view = $state<View>({ kind: "quick" });
  // A new key per session, so that connecting again replaces the terminal.
  let sessions = 0;

  const host = $derived(selectedHost());
  const folder = $derived(selectedFolder());

  appName().then((value) => {
    name = value;
    document.title = value;
  });

  // Changes from outside the app (a git pull) show without a restart. The
  // window lives as long as the app, so the listener is never removed.
  void onWorkspaceChanged(workspaceChanged);

  restoreWorkspace().then(setWorkspace, (error) => {
    inventory.openError = `Could not reopen the last workspace: ${error}`;
  });

  async function open() {
    const folder = await pickWorkspaceFolder();
    if (folder === null) return;
    try {
      setWorkspace(await openWorkspace(folder));
      inventory.openError = null;
    } catch (error) {
      inventory.openError = String(error);
    }
  }

  function connect(target: ConnectTarget, title: string) {
    view = { kind: "session", target, title, key: ++sessions };
  }

  function prompt(host: Host) {
    inventory.selected = { kind: "host", id: host.id };
    view = { kind: "prompt", host };
  }

  // Ctrl+K opens the palette from anywhere, the terminal included, so it is
  // caught before the focused element sees it.
  function onkeydowncapture(event: KeyboardEvent) {
    const ctrlK =
      event.ctrlKey &&
      !event.altKey &&
      !event.shiftKey &&
      !event.metaKey &&
      // By position too, so that it works on any keyboard layout.
      (event.code === "KeyK" || event.key.toLowerCase() === "k");
    if (!ctrlK || !inventory.workspace || inventory.dialog) return;
    event.preventDefault();
    event.stopPropagation();
    inventory.palette = true;
  }
</script>

<svelte:window {onkeydowncapture} />

<div class="layout" class:with-details={host || folder}>
  <WorkspacePanel onopen={open} onconnect={prompt} />

  <section class="centre">
    {#if view.kind === "session"}
      {#key view.key}
        <TerminalSession
          target={view.target}
          title={view.title}
          role={view.target.kind === "host" ? hostRole(view.target.id) : null}
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

  {#if host}
    <aside class="side">
      <HostDetails {host} onconnect={prompt} />
    </aside>
  {:else if folder}
    <aside class="side">
      <FolderDetails {folder} />
    </aside>
  {/if}
</div>

{#if inventory.palette}
  <CommandPalette
    onconnect={prompt}
    onclose={() => (inventory.palette = false)}
  />
{/if}

{#if inventory.dialog}
  {#key inventory.dialog}
    <Dialog
      request={inventory.dialog}
      onclose={() => (inventory.dialog = null)}
    />
  {/key}
{/if}

<style>
  .layout {
    height: 100%;
    display: grid;
    grid-template-columns: minmax(14rem, 18rem) 1fr;
  }

  .layout.with-details {
    grid-template-columns: minmax(14rem, 18rem) 1fr minmax(15rem, 19rem);
  }

  .centre {
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  .side {
    min-height: 0;
    overflow: auto;
    background: var(--bg);
    border-left: 1px solid var(--border);
  }
</style>
