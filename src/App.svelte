<script lang="ts">
  import CommandPalette from "./lib/CommandPalette.svelte";
  import CredentialsPrompt from "./lib/CredentialsPrompt.svelte";
  import Dialog from "./lib/Dialog.svelte";
  import FolderDetails from "./lib/FolderDetails.svelte";
  import HostDetails from "./lib/HostDetails.svelte";
  import ProfilesDialog from "./lib/ProfilesDialog.svelte";
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
  import { hostLogin } from "./lib/credentials";
  import { profiles, refreshProfiles } from "./lib/profiles.svelte";
  import {
    appName,
    type ConnectTarget,
    type SessionSource,
  } from "./lib/session";
  import {
    openWorkspace,
    pickWorkspaceFolder,
    onWorkspaceChanged,
    restoreWorkspace,
    type Host,
  } from "./lib/workspace";

  /** What the centre of the window shows when no session is shown. */
  type View =
    | { kind: "quick" }
    | {
        kind: "prompt";
        host: Host;
        missingProfile: string | null;
        /** Why the host's profile could not log in, and its username. */
        fallback?: { reason: string; username: string };
      };

  /** A session with its terminal, shown or kept running out of sight. */
  interface OpenSession {
    /** New per session, so that each gets its own terminal. */
    key: number;
    source: SessionSource;
    title: string;
    /** The inventory host it connects to, if any (for its role). */
    hostId: string | null;
    /** The host and username, when logging in with a profile. */
    profileLogin?: { host: Host; username: string };
  }

  let name = $state("");
  let view = $state<View>({ kind: "quick" });
  /**
   * The open sessions. A new connection replaces the shown session; a
   * duplicate opens beside it (until sessions get tabs).
   */
  let sessions = $state<OpenSession[]>([]);
  /** The key of the session on screen; null shows `view`. */
  let shown = $state<number | null>(null);
  let nextKey = 0;

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

  /** Closes the session on screen, if one is: something else replaces it. */
  function leaveShown() {
    if (shown === null) return;
    const leaving = shown;
    sessions = sessions.filter((session) => session.key !== leaving);
    shown = null;
  }

  function connect(
    target: ConnectTarget,
    title: string,
    profileLogin?: { host: Host; username: string },
  ) {
    leaveShown();
    const key = ++nextKey;
    sessions.push({
      key,
      source: { kind: "target", target },
      title,
      hostId: target.kind === "host" ? target.id : null,
      profileLogin,
    });
    shown = key;
  }

  /** Opens a second session like `original` (core session `id`). */
  function duplicate(original: OpenSession, id: number) {
    const key = ++nextKey;
    const at = sessions.findIndex((session) => session.key === original.key);
    sessions.splice(at + 1, 0, {
      ...original,
      key,
      source: { kind: "duplicate", of: id },
    });
    shown = key;
  }

  /** Closes the session and shows the quick-connect form. */
  function dismiss(key: number) {
    sessions = sessions.filter((session) => session.key !== key);
    if (shown === key) shown = null;
    view = { kind: "quick" };
  }

  // The profiles a workspace names change with its files, and each
  // workspace has its own.
  $effect(() => {
    if (inventory.workspace) void refreshProfiles();
  });

  /**
   * Connects to an inventory host: at once with its credential profile,
   * else after asking for credentials (or for the profile to set up).
   */
  async function prompt(host: Host) {
    inventory.selected = { kind: "host", id: host.id };
    let login;
    try {
      login = await hostLogin(host.id);
    } catch (error) {
      // The profiles cannot be read (the keychain refused, say): say so,
      // and let the user log in by hand meanwhile.
      inventory.editError = `Cannot read the credential profiles: ${error}`;
      login = { kind: "prompt" } as const;
    }
    if (login.kind === "profile") {
      connect(
        { kind: "host", id: host.id, credentials: null },
        `${login.username}@${host.name}`,
        { host, username: login.username },
      );
    } else {
      const missingProfile = login.kind === "notSetUp" ? login.profile : null;
      leaveShown();
      view = { kind: "prompt", host, missingProfile };
    }
  }

  /**
   * For a session logging in with a profile: asks for a password instead,
   * after the profile (its SSH agent) could not log in.
   */
  function passwordLogin(session: OpenSession) {
    const login = session.profileLogin;
    if (!login) return undefined;
    return (reason: string) => {
      dismiss(session.key);
      view = {
        kind: "prompt",
        host: login.host,
        missingProfile: null,
        fallback: { reason, username: login.username },
      };
    };
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
    if (!ctrlK || !inventory.workspace || inventory.dialog || profiles.dialog)
      return;
    event.preventDefault();
    event.stopPropagation();
    inventory.palette = true;
  }
</script>

<svelte:window {onkeydowncapture} />

<div class="layout" class:with-details={host || folder}>
  <WorkspacePanel onopen={open} onconnect={prompt} />

  <section
    class="centre"
    class:with-switcher={sessions.length > 1 ||
      (sessions.length > 0 && shown === null)}
  >
    {#if sessions.length > 1 || (sessions.length > 0 && shown === null)}
      <div class="switcher" role="group" aria-label="Open sessions">
        {#each sessions as session (session.key)}
          <button
            data-session-switch={session.key}
            aria-pressed={session.key === shown}
            onclick={() => (shown = session.key)}>{session.title}</button
          >
        {/each}
      </div>
    {/if}
    <div class="views">
      {#each sessions as session (session.key)}
        <div class="slot" hidden={session.key !== shown}>
          <TerminalSession
            source={session.source}
            title={session.title}
            role={session.hostId ? hostRole(session.hostId) : null}
            shown={session.key === shown}
            ondismiss={() => dismiss(session.key)}
            onduplicate={(id) => duplicate(session, id)}
            onpasswordlogin={passwordLogin(session)}
          />
        </div>
      {/each}
      {#if shown === null}
        {#if view.kind === "prompt"}
          {@const host = view.host}
          {#key host.id}
            <CredentialsPrompt
              {host}
              missingProfile={view.missingProfile}
              fallback={view.fallback}
              oncancel={() => (view = { kind: "quick" })}
              onconnect={(credentials) =>
                connect(
                  { kind: "host", id: host.id, credentials },
                  `${credentials.username}@${host.name}`,
                )}
              onprofile={() => prompt(host)}
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
      {/if}
    </div>
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

{#if profiles.dialog}
  <ProfilesDialog onclose={() => (profiles.dialog = false)} />
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
    display: grid;
    grid-template-rows: minmax(0, 1fr);
  }

  .centre.with-switcher {
    grid-template-rows: auto minmax(0, 1fr);
  }

  .switcher {
    display: flex;
    flex-wrap: wrap;
    gap: 0.3rem;
    padding: 0.3rem 0.4rem;
    background: var(--bg);
    border-bottom: 1px solid var(--border);
  }

  .switcher button {
    padding: 0.15rem 0.6rem;
    font-size: 0.8rem;
  }

  .switcher button[aria-pressed="true"] {
    border-color: var(--accent, var(--border));
    font-weight: 600;
  }

  .views,
  .slot {
    min-width: 0;
    min-height: 0;
    height: 100%;
    overflow: hidden;
  }

  .side {
    min-height: 0;
    overflow: auto;
    background: var(--bg);
    border-left: 1px solid var(--border);
  }
</style>
