<script lang="ts">
  import { tick } from "svelte";
  import type { Attachment } from "svelte/attachments";
  import CommandPalette from "./lib/CommandPalette.svelte";
  import ConfirmClose from "./lib/ConfirmClose.svelte";
  import CredentialsPrompt from "./lib/CredentialsPrompt.svelte";
  import Dialog from "./lib/Dialog.svelte";
  import FolderDetails from "./lib/FolderDetails.svelte";
  import HighlightRulesDialog from "./lib/HighlightRulesDialog.svelte";
  import HostDetails from "./lib/HostDetails.svelte";
  import PaneView, {
    type PaneHandlers,
    type TabInfo,
  } from "./lib/PaneView.svelte";
  import PreferencesDialog from "./lib/PreferencesDialog.svelte";
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
  import { highlighting, trackHighlightRules } from "./lib/highlight.svelte";
  import { profiles, refreshProfiles } from "./lib/profiles.svelte";
  import {
    isDark,
    loadPreferences,
    preferences,
    zoomFont,
  } from "./lib/preferences.svelte";
  import {
    appName,
    type ConnectTarget,
    type SessionSource,
    type SessionState,
  } from "./lib/session";
  import {
    editLayout,
    panesOf,
    resetLayout,
    type Layout,
    type LayoutEdit,
    type PaneId,
    type TabId,
  } from "./lib/layout";
  import {
    openWorkspace,
    pickWorkspaceFolder,
    onWorkspaceChanged,
    restoreWorkspace,
    type Host,
    type Protocol,
  } from "./lib/workspace";

  /** What the active pane shows when it shows no session. */
  type View =
    | { kind: "quick" }
    | {
        kind: "prompt";
        host: Host;
        /** The protocol the user chose; null: the host's own. */
        protocol: Protocol | null;
        missingProfile: string | null;
        /** Why the host's profile could not log in, and its username. */
        fallback?: { reason: string; username: string };
      };

  /** A tab's session with its terminal, shown or kept running out of sight. */
  interface OpenSession {
    source: SessionSource;
    /** The tab's label: the host's name, address or serial port. */
    name: string;
    /** Shown in the session's header, such as `user@host:port`. */
    title: string;
    /** The inventory host it connects to, if any (for its role). */
    hostId: string | null;
    /** The host and username, when logging in with a profile. */
    profileLogin?: { host: Host; username: string };
  }

  let name = $state("");
  let view = $state<View>({ kind: "quick" });
  /** The tabs and panes, as the core keeps them; null until it answers. */
  let layout = $state<Layout | null>(null);
  /** Each tab's session, by tab id. */
  let sessions = $state<Record<TabId, OpenSession>>({});
  /** Each tab's session state, as its terminal reports it. */
  let states = $state<Record<TabId, SessionState["state"]>>({});
  /** Raised when a tab is clicked, so that its terminal takes the keys. */
  let focusRequest = $state(0);
  /** The tab whose closing waits for the user's confirmation. */
  let closing = $state<TabId | null>(null);

  const panes = $derived(layout ? panesOf(layout.root) : []);
  const activePane = $derived(
    panes.find((pane) => pane.id === layout?.activePane) ?? null,
  );
  /** Where each tab is: its pane, and whether that pane shows it. */
  const placement = $derived(
    new Map(
      panes.flatMap((pane) =>
        pane.tabs.map((tab) => [
          tab,
          { pane: pane.id, shown: pane.active === tab },
        ]),
      ),
    ),
  );

  // Sessions do not outlive the interface, so neither do its tabs.
  resetLayout().then(adopt);

  /** Shows `next` unless a newer layout is already shown. */
  function adopt(next: Layout) {
    if (layout === null || next.revision > layout.revision) layout = next;
  }

  /**
   * Edits the layout; `prepare` is given the created tab or pane before the
   * new layout is shown (to give a new tab its session). Edits naming a tab
   * or pane that is already gone (a race with closing it) do nothing.
   */
  async function edit(
    change: LayoutEdit,
    prepare?: (created: number) => void,
  ): Promise<void> {
    let changed;
    try {
      changed = await editLayout(change);
    } catch {
      return;
    }
    if (changed.created !== null) prepare?.(changed.created);
    adopt(changed.layout);
    forgetClosedTabs();
  }

  /** Drops the sessions of tabs no longer in the layout (their terminals go). */
  function forgetClosedTabs() {
    for (const key of Object.keys(sessions)) {
      const tab = Number(key);
      if (!placement.has(tab)) {
        delete sessions[tab];
        delete states[tab];
      }
    }
  }

  /** Where each pane's body is, relative to the panes' area. */
  let rects = $state<
    Record<PaneId, { left: number; top: number; width: number; height: number }>
  >({});
  let area = $state<HTMLElement>();
  const bodies = new Map<PaneId, HTMLElement>();
  const resizes = new ResizeObserver(() => measure());

  function measure() {
    if (!area) return;
    const origin = area.getBoundingClientRect();
    const next: typeof rects = {};
    for (const [pane, body] of bodies) {
      const box = body.getBoundingClientRect();
      next[pane] = {
        left: box.left - origin.left,
        top: box.top - origin.top,
        width: box.width,
        height: box.height,
      };
    }
    rects = next;
  }

  // Terminals are placed over their pane's body rather than inside it, so
  // that moving a tab to another pane keeps its terminal (and session).
  function paneBody(pane: PaneId): Attachment<HTMLElement> {
    return (body) => {
      bodies.set(pane, body);
      resizes.observe(body);
      measure();
      return () => {
        bodies.delete(pane);
        resizes.unobserve(body);
      };
    };
  }

  // Panes can move without changing size (another pane closing).
  $effect(() => {
    if (layout) void tick().then(measure);
  });

  function tabState(tab: TabId): TabInfo["state"] {
    const state = states[tab] ?? "connecting";
    return state === "closed" ? "disconnected" : state;
  }

  const handlers: PaneHandlers = {
    tab(tab) {
      const session = sessions[tab];
      if (!session) return null;
      return {
        name: session.name,
        title: session.title,
        role: session.hostId ? hostRole(session.hostId) : null,
        state: tabState(tab),
      };
    },
    activate: (tab) => {
      focusRequest += 1;
      void edit({ kind: "activate", tab });
    },
    activatePane: (pane) => void edit({ kind: "activatePane", pane }),
    close: (tab) => closeTab(tab),
    move: (tab, pane, index) => {
      focusRequest += 1;
      void edit({ kind: "moveTab", tab, pane, index });
    },
    newTab: (pane) => {
      view = { kind: "quick" };
      void edit({ kind: "showConnectView", pane });
    },
    split: (pane, direction) => void edit({ kind: "split", pane, direction }),
    closePane: (pane) => void edit({ kind: "closePane", pane }),
    body: paneBody,
  };

  /** Closes `tab`, first asking if its session is still live. */
  function closeTab(tab: TabId) {
    if (!sessions[tab]) return;
    if (tabState(tab) !== "disconnected") {
      closing = tab;
      return;
    }
    void edit({ kind: "closeTab", tab });
  }

  // A session that ends while its closing waits needs no confirmation.
  $effect(() => {
    if (closing !== null && tabState(closing) === "disconnected")
      confirmClose();
  });

  function confirmClose() {
    const tab = closing;
    closing = null;
    if (tab !== null) void edit({ kind: "closeTab", tab });
  }

  const host = $derived(selectedHost());
  const folder = $derived(selectedFolder());

  void loadPreferences();
  trackHighlightRules();

  // Ctrl+scroll sizes the terminal font (TerminalSession), never the page.
  // (Svelte's own wheel listeners are passive, so this one is added here.)
  window.addEventListener(
    "wheel",
    (event) => {
      if (event.ctrlKey) event.preventDefault();
    },
    { passive: false },
  );

  // The whole app follows the theme (app.css), the system's by default.
  $effect(() => {
    document.documentElement.dataset.theme = isDark() ? "dark" : "light";
  });

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

  /** Opens a session in a new tab of the active pane. */
  function connect(
    target: ConnectTarget,
    name: string,
    title: string,
    profileLogin?: { host: Host; username: string },
  ) {
    void edit({ kind: "openTab" }, (tab) => {
      sessions[tab] = {
        source: { kind: "target", target },
        name,
        title,
        hostId: "id" in target ? target.id : null,
        profileLogin,
      };
    });
  }

  /** Opens a second session like `original`'s (core session `id`) in a
   * new tab beside it. */
  function duplicate(original: TabId, id: number) {
    const session = sessions[original];
    if (!session) return;
    void edit({ kind: "openTabBeside", tab: original }, (tab) => {
      sessions[tab] = { ...session, source: { kind: "duplicate", of: id } };
    });
  }

  /** Closes the tab and shows the quick-connect form in its place. */
  async function dismiss(tab: TabId) {
    view = { kind: "quick" };
    await edit({ kind: "dismissTab", tab });
  }

  /** Shows the connect view (with `next`) in the active pane. */
  async function showConnectView(next: View) {
    view = next;
    if (layout)
      await edit({ kind: "showConnectView", pane: layout.activePane });
  }

  // The profiles a workspace names change with its files, and each
  // workspace has its own.
  $effect(() => {
    if (inventory.workspace) void refreshProfiles();
  });

  /**
   * Connects to an inventory host over `protocol` (null: its own protocol
   * setting): by telnet at once (the device asks for the login in the
   * terminal); otherwise at once with its credential profile, else after
   * asking for credentials (or for the profile to set up).
   */
  async function prompt(host: Host, protocol: Protocol | null = null) {
    inventory.selected = { kind: "host", id: host.id };
    let login;
    try {
      login = await hostLogin(host.id, protocol);
    } catch (error) {
      // The profiles cannot be read (the keychain refused, say): say so,
      // and let the user log in by hand meanwhile.
      inventory.editError = `Cannot read the credential profiles: ${error}`;
      login = { kind: "prompt" } as const;
    }
    if (login.kind === "telnet") {
      connect(
        { kind: "telnet", id: host.id },
        host.name,
        `${host.name} (telnet)`,
      );
    } else if (login.kind === "serial") {
      connect(
        { kind: "serial", id: host.id },
        host.name,
        `${host.name} (serial)`,
      );
    } else if (login.kind === "profile") {
      connect(
        { kind: "host", id: host.id, credentials: null },
        host.name,
        `${login.username}@${host.name}`,
        { host, username: login.username },
      );
    } else {
      const missingProfile = login.kind === "notSetUp" ? login.profile : null;
      void showConnectView({ kind: "prompt", host, protocol, missingProfile });
    }
  }

  /**
   * For a session logging in with a profile: asks for a password instead,
   * after the profile (its SSH agent) could not log in.
   */
  function passwordLogin(tab: TabId, session: OpenSession) {
    const login = session.profileLogin;
    if (!login) return undefined;
    return async (reason: string) => {
      await dismiss(tab);
      view = {
        kind: "prompt",
        host: login.host,
        // Only SSH sessions log in with an agent.
        protocol: "ssh",
        missingProfile: null,
        fallback: { reason, username: login.username },
      };
    };
  }

  /** The window's shortcuts, by what they do. */
  function shortcut(event: KeyboardEvent): (() => void) | null {
    if (!event.ctrlKey || event.altKey || event.metaKey) return null;
    // By position too, so that letters work on any keyboard layout.
    const letter = (name: string) =>
      event.code === `Key${name}` ||
      event.key.toLowerCase() === name.toLowerCase();
    const tab = layout ? (activePane?.active ?? null) : null;
    if (!event.shiftKey && letter("K")) {
      return inventory.workspace ? () => (inventory.palette = true) : null;
    }
    if (
      event.key === "Tab" ||
      event.key === "PageDown" ||
      event.key === "PageUp"
    ) {
      const forward =
        event.key === "PageDown" || (event.key === "Tab" && !event.shiftKey);
      if (event.key !== "Tab" && event.shiftKey) {
        // Ctrl+Shift+PageUp/PageDown: moves the tab.
        return tab === null
          ? null
          : () =>
              void edit({ kind: "moveTabBy", tab, offset: forward ? 1 : -1 });
      }
      return () => void edit({ kind: forward ? "nextTab" : "previousTab" });
    }
    if (event.shiftKey && letter("W")) {
      return tab === null ? null : () => closeTab(tab);
    }
    return null;
  }

  /** The terminal font size: Ctrl+= (or Ctrl++), Ctrl+-, Ctrl+0. These work
   * with dialogs open too (the page itself never zooms). */
  function zoomShortcut(event: KeyboardEvent): (() => void) | null {
    if (!event.ctrlKey || event.altKey || event.metaKey) return null;
    if (
      event.code === "Equal" ||
      event.code === "NumpadAdd" ||
      event.key === "+" ||
      event.key === "="
    ) {
      return () => void zoomFont("in");
    }
    if (event.shiftKey) return null;
    if (
      event.code === "Minus" ||
      event.code === "NumpadSubtract" ||
      event.key === "-"
    ) {
      return () => void zoomFont("out");
    }
    if (event.code === "Digit0" || event.code === "Numpad0") {
      return () => void zoomFont("reset");
    }
    return null;
  }

  // Shortcuts work from anywhere, the terminal included, so they are caught
  // before the focused element sees them.
  function onkeydowncapture(event: KeyboardEvent) {
    const busy =
      inventory.dialog ||
      profiles.dialog ||
      preferences.dialog ||
      highlighting.dialog ||
      inventory.palette ||
      closing !== null;
    const action = zoomShortcut(event) ?? (busy ? null : shortcut(event));
    if (!action) return;
    event.preventDefault();
    event.stopPropagation();
    action();
  }
</script>

<svelte:window {onkeydowncapture} />

<div class="layout" class:with-details={host || folder}>
  <WorkspacePanel onopen={open} onconnect={prompt} />

  <section class="centre" bind:this={area}>
    {#if layout}
      <PaneView
        node={layout.root}
        activePane={layout.activePane}
        several={panes.length > 1}
        {handlers}
      />
      {#each Object.entries(sessions) as [key, session] (key)}
        {@const tab = Number(key)}
        {@const place = placement.get(tab)}
        {@const rect = place ? rects[place.pane] : undefined}
        {@const shown = !!place?.shown && rect !== undefined}
        {@const focused = shown && place?.pane === layout.activePane}
        <div
          class="slot"
          data-tab={tab}
          data-pane={place?.pane}
          data-active={focused || undefined}
          hidden={!shown}
          style:left="{rect?.left ?? 0}px"
          style:top="{rect?.top ?? 0}px"
          style:width="{rect?.width ?? 0}px"
          style:height="{rect?.height ?? 0}px"
          onfocusin={() => {
            if (!focused && place) handlers.activate(tab);
          }}
        >
          <TerminalSession
            source={session.source}
            title={session.title}
            role={session.hostId ? hostRole(session.hostId) : null}
            {shown}
            {focused}
            {focusRequest}
            onstate={(state) => (states[tab] = state.state)}
            ondismiss={() => dismiss(tab)}
            onduplicate={(id) => duplicate(tab, id)}
            onpasswordlogin={passwordLogin(tab, session)}
          />
        </div>
      {/each}
      {@const connectRect = rects[layout.activePane]}
      {#if activePane && activePane.active === null && connectRect}
        <div
          class="slot connect"
          data-pane={activePane.id}
          style:left="{connectRect.left}px"
          style:top="{connectRect.top}px"
          style:width="{connectRect.width}px"
          style:height="{connectRect.height}px"
        >
          {#if view.kind === "prompt"}
            {@const host = view.host}
            {@const protocol = view.protocol}
            {#key host.id}
              <CredentialsPrompt
                {host}
                missingProfile={view.missingProfile}
                fallback={view.fallback}
                oncancel={() => (view = { kind: "quick" })}
                onconnect={(credentials) =>
                  connect(
                    { kind: "host", id: host.id, credentials },
                    host.name,
                    `${credentials.username}@${host.name}`,
                  )}
                onprofile={() => prompt(host, protocol)}
              />
            {/key}
          {:else}
            <QuickConnect
              {name}
              onconnect={(target) =>
                connect(
                  { kind: "quick", ...target },
                  target.host,
                  `${target.username}@${target.host}:${target.port}`,
                )}
              onserial={(port, parameters) =>
                connect(
                  { kind: "quickSerial", port, parameters },
                  port,
                  `${port} (${parameters.baudRate} ${parameters.dataBits}${parameters.parity[0].toUpperCase()}${parameters.stopBits})`,
                )}
            />
          {/if}
        </div>
      {/if}
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

{#if closing !== null && sessions[closing]}
  {@const state = tabState(closing)}
  <ConfirmClose
    name={sessions[closing].name}
    state={state === "disconnected" ? "connected" : state}
    onclose={confirmClose}
    oncancel={() => {
      closing = null;
      focusRequest += 1;
    }}
  />
{/if}

{#if preferences.dialog}
  <PreferencesDialog
    onclose={() => {
      preferences.dialog = false;
      focusRequest += 1;
    }}
  />
{/if}

{#if highlighting.dialog}
  <HighlightRulesDialog
    onclose={() => {
      highlighting.dialog = false;
      focusRequest += 1;
    }}
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
    position: relative;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    display: flex;
  }

  /* Placed over its pane's body (see paneBody). */
  .slot {
    position: absolute;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  .slot.connect {
    background: var(--bg);
  }

  .side {
    min-height: 0;
    overflow: auto;
    background: var(--bg);
    border-left: 1px solid var(--border);
  }
</style>
