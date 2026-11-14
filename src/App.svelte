<script lang="ts">
  import { tick } from "svelte";
  import type { Attachment } from "svelte/attachments";
  import CommandPalette from "./lib/CommandPalette.svelte";
  import ConfirmClose from "./lib/ConfirmClose.svelte";
  import ConfirmSwitch from "./lib/ConfirmSwitch.svelte";
  import CredentialsPrompt from "./lib/CredentialsPrompt.svelte";
  import Dialog from "./lib/Dialog.svelte";
  import FolderDetails from "./lib/FolderDetails.svelte";
  import HighlightRulesDialog from "./lib/HighlightRulesDialog.svelte";
  import HostDetails from "./lib/HostDetails.svelte";
  import PaneView, {
    type PaneHandlers,
    type TabInfo,
  } from "./lib/PaneView.svelte";
  import PanelResizer from "./lib/PanelResizer.svelte";
  import PreferencesDialog from "./lib/PreferencesDialog.svelte";
  import ProfilesDialog from "./lib/ProfilesDialog.svelte";
  import QuickConnect from "./lib/QuickConnect.svelte";
  import SnippetDialog from "./lib/SnippetDialog.svelte";
  import SnippetVariables from "./lib/SnippetVariables.svelte";
  import TerminalSession from "./lib/TerminalSession.svelte";
  import UpdateNotice from "./lib/UpdateNotice.svelte";
  import { startUpdates } from "./lib/updates.svelte";
  import WorkspacePanel from "./lib/WorkspacePanel.svelte";
  import WorkspacesDialog from "./lib/WorkspacesDialog.svelte";
  import CsvImportDialog from "./lib/CsvImportDialog.svelte";
  import NornirImportDialog from "./lib/NornirImportDialog.svelte";
  import {
    hostRole,
    hostSafety,
    inventory,
    selectedFolder,
    selectedHost,
    setWorkspace,
    workspaceChanged,
  } from "./lib/inventory.svelte";
  import { hostLogin } from "./lib/credentials";
  import { highlighting, trackHighlightRules } from "./lib/highlight.svelte";
  import { profiles, refreshProfiles } from "./lib/profiles.svelte";
  import { snippets } from "./lib/snippets.svelte";
  import { NO_SAFETY } from "./lib/paste";
  import {
    changePreferences,
    isDark,
    loadPreferences,
    PANEL_WIDTHS,
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
    expandSnippet,
    onWorkspaceChanged,
    restoreWorkspace,
    type Host,
    type PasteSafety,
    type Protocol,
    type Role,
    type WorkspaceView,
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
    /**
     * The root of the workspace that host is in; null for quick connect.
     * Host ids only mean something in their own workspace, so a session
     * kept open after switching to another uses what it opened with.
     */
    workspace: string | null;
    /**
     * The host's environment and paste line delay when the session was
     * opened; kept for when the host is no longer in the open workspace.
     */
    safety: PasteSafety;
    /** The host's role when the session was opened, kept likewise. */
    role: Role | null;
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
  /** Each tab's terminal, for sending snippets to it. */
  const terminals: Record<TabId, TerminalSession | null> = {};
  /** Whether the workspace switcher is open. */
  let switcher = $state(false);
  /** A switch waiting for the user to keep or close the open sessions. */
  let switching = $state<{
    from: string;
    live: number;
    decide: (choice: "keep" | "close" | "cancel") => void;
  } | null>(null);
  /** A snippet waiting for the user to give its variables a value. */
  let asking = $state<{ tab: TabId; name: string; names: string[] } | null>(
    null,
  );

  const panes = $derived(layout ? panesOf(layout.root) : []);
  const activePane = $derived(
    panes.find((pane) => pane.id === layout?.activePane) ?? null,
  );
  /** The tab snippets are sent to: the active pane's, if connected. */
  const snippetTab = $derived.by(() => {
    const tab = activePane?.active ?? null;
    return tab !== null && sessions[tab] && states[tab] === "connected"
      ? tab
      : null;
  });
  /** That tab's name, as the snippet panel and palette show it. */
  const snippetTarget = $derived(
    snippetTab === null ? null : sessions[snippetTab].name,
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

  /** The session's host id, if that host is in the open workspace. */
  function liveHost(session: OpenSession): string | null {
    return session.hostId !== null &&
      session.workspace === inventory.workspace?.root
      ? session.hostId
      : null;
  }

  /** What applies to the session's pastes and colours: its host's, as the
   * workspace now sets them (live), else as they were when it opened. */
  function safetyOf(session: OpenSession): PasteSafety {
    const id = liveHost(session);
    return (id ? hostSafety(id) : null) ?? session.safety;
  }

  /** The session's host's role, likewise. */
  function roleOf(session: OpenSession): Role | null {
    const id = liveHost(session);
    return id ? hostRole(id) : session.role;
  }

  /** Whether `tab` is in the multi-send group. */
  function inGroup(tab: TabId): boolean {
    return layout?.group.includes(tab) ?? false;
  }

  /** Adds `tab` to the multi-send group, or takes it out. */
  function toggleGroup(tab: TabId) {
    void edit({ kind: inGroup(tab) ? "leaveGroup" : "joinGroup", tab });
  }

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
        role: roleOf(session),
        environment: safetyOf(session).environment,
        state: tabState(tab),
        member: inGroup(tab),
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

  // Side panels: widths from the preferences, or the one being dragged.
  let liveLeft = $state<number | null>(null);
  let liveRight = $state<number | null>(null);
  let windowWidth = $state(window.innerWidth);
  /** The terminal area keeps at least this much when panels widen. */
  const MIN_CENTRE = 320;
  const REVEAL_STRIP = 14;
  const leftShown = $derived(!preferences.current.leftPanelHidden);
  const hasDetails = $derived(Boolean(host || folder));
  const rightShown = $derived(
    hasDetails && !preferences.current.rightPanelHidden,
  );
  const leftWidth = $derived(liveLeft ?? preferences.current.leftPanelWidth);
  const rightWidth = $derived(liveRight ?? preferences.current.rightPanelWidth);
  const leftColumn = $derived(leftShown ? leftWidth : REVEAL_STRIP);
  const rightColumn = $derived(
    hasDetails ? (rightShown ? rightWidth : REVEAL_STRIP) : 0,
  );
  /** How wide a panel may get without squeezing the terminal. */
  const widest = (other: number) =>
    Math.min(PANEL_WIDTHS.max, windowWidth - other - MIN_CENTRE);
  const columns = $derived(
    hasDetails
      ? `${leftColumn}px minmax(0, 1fr) ${rightColumn}px`
      : `${leftColumn}px minmax(0, 1fr)`,
  );

  function togglePanel(side: "left" | "right") {
    if (side === "left") {
      void changePreferences({
        leftPanelHidden: !preferences.current.leftPanelHidden,
      });
    } else {
      void changePreferences({
        rightPanelHidden: !preferences.current.rightPanelHidden,
      });
    }
  }

  void loadPreferences();
  trackHighlightRules();
  void startUpdates();

  /** Sessions that installing an update would end. */
  const liveSessions = $derived(
    Object.values(states).filter(
      (state) => state === "connecting" || state === "connected",
    ).length,
  );

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

  /**
   * Switches to the workspace `open` opens. If sessions to hosts of the
   * open workspace are live, the user first chooses to keep them (they stay
   * in their tabs) or close them (their tabs close once the other workspace
   * opened). Resolves to why it did not open, or null.
   */
  async function switchWorkspace(
    open: () => Promise<WorkspaceView>,
  ): Promise<string | null> {
    const current = inventory.workspace;
    const tabs = current
      ? Object.keys(sessions)
          .map(Number)
          .filter((tab) => sessions[tab].workspace === current.root)
      : [];
    const live = tabs.filter((tab) => tabState(tab) !== "disconnected");
    let choice: "keep" | "close" | "cancel" = "keep";
    if (current && live.length > 0) {
      choice = await new Promise((decide) => {
        switching = { from: current.tree.name, live: live.length, decide };
      });
      switching = null;
      if (choice === "cancel") return null;
    }
    let next;
    try {
      next = await open();
    } catch (error) {
      return String(error);
    }
    // Reopening the same workspace leaves its sessions where they are.
    if (choice === "close" && next.root !== current?.root) {
      for (const tab of tabs) await edit({ kind: "closeTab", tab });
    }
    setWorkspace(next);
    inventory.openError = null;
    switcher = false;
    focusRequest += 1;
    return null;
  }

  /** Opens a session in a new tab of the active pane. */
  function connect(
    target: ConnectTarget,
    name: string,
    title: string,
    profileLogin?: { host: Host; username: string },
  ) {
    const hostId = "id" in target ? target.id : null;
    const workspace = hostId ? (inventory.workspace?.root ?? null) : null;
    void edit({ kind: "openTab" }, (tab) => {
      sessions[tab] = {
        source: { kind: "target", target },
        name,
        title,
        hostId,
        workspace,
        safety: (hostId ? hostSafety(hostId) : null) ?? NO_SAFETY,
        role: hostId ? hostRole(hostId) : null,
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
    // Its host can only be reached through its own workspace.
    if (!login || liveHost(session) === null) return undefined;
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

  /**
   * Sends the snippet `name` to the session of `tab` (the active pane's by
   * default): the core fills in its variables from the session's host and
   * `answers`; for any still without a value the user is asked first. The
   * lines go through the terminal's paste path, one by one.
   */
  async function sendSnippet(
    name: string,
    answers: Record<string, string> = {},
    tab: TabId | null = snippetTab,
  ) {
    const session = tab === null ? undefined : sessions[tab];
    if (tab === null || !session || states[tab] !== "connected") {
      inventory.editError = `Cannot send ${name}: no connected session.`;
      return;
    }
    let expansion;
    try {
      // A host of another workspace gives no values: all are asked for.
      expansion = await expandSnippet(name, liveHost(session), answers);
    } catch (error) {
      inventory.editError = `Cannot send ${name}: ${error}`;
      return;
    }
    if (expansion.kind === "missing") {
      asking = { tab, name, names: expansion.names };
      return;
    }
    const text = expansion.lines.map((line) => `${line}\n`).join("");
    if (!terminals[tab]?.sendText(text)) {
      inventory.editError = `Cannot send ${name}: the session is no longer connected.`;
      return;
    }
    focusRequest += 1;
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
    if (event.shiftKey && letter("O")) {
      return () => (switcher = true);
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
    if (event.shiftKey && letter("M")) {
      return tab === null ? null : () => toggleGroup(tab);
    }
    // Not Ctrl+B: that is tmux's prefix, so it must reach devices.
    if (event.shiftKey && letter("B")) {
      return () => togglePanel("left");
    }
    if (event.shiftKey && letter("D")) {
      return hasDetails ? () => togglePanel("right") : null;
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
      switcher ||
      switching ||
      snippets.editing ||
      asking ||
      closing !== null;
    const action = zoomShortcut(event) ?? (busy ? null : shortcut(event));
    if (!action) return;
    event.preventDefault();
    event.stopPropagation();
    action();
  }
</script>

<svelte:window {onkeydowncapture} bind:innerWidth={windowWidth} />

<div class="layout" style:grid-template-columns={columns}>
  {#if leftShown}
    <WorkspacePanel
      onopen={() => (switcher = true)}
      onconnect={prompt}
      {snippetTarget}
      onsnippet={(snippet) => void sendSnippet(snippet)}
    />
  {:else}
    <button
      class="reveal"
      data-reveal-panel="left"
      title="Show the workspace panel (Ctrl+Shift+B)"
      aria-label="Show the workspace panel"
      onclick={() => togglePanel("left")}>›</button
    >
  {/if}

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
            bind:this={terminals[tab]}
            source={session.source}
            {tab}
            groupSize={inGroup(tab) ? layout.group.length : null}
            ontogglegroup={() => toggleGroup(tab)}
            onendgroup={() => void edit({ kind: "endGroup" })}
            title={session.title}
            role={roleOf(session)}
            safety={safetyOf(session)}
            name={session.name}
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

  {#if hasDetails && !rightShown}
    <button
      class="reveal"
      data-reveal-panel="right"
      title="Show the details panel (Ctrl+Shift+D)"
      aria-label="Show the details panel"
      onclick={() => togglePanel("right")}>‹</button
    >
  {:else if host}
    <aside class="side">
      <HostDetails {host} onconnect={prompt} />
    </aside>
  {:else if folder}
    <aside class="side">
      <FolderDetails {folder} />
    </aside>
  {/if}

  {#if leftShown}
    <div class="edge" style:left="{leftWidth}px">
      <PanelResizer
        side="left"
        width={leftWidth}
        min={PANEL_WIDTHS.min}
        max={widest(rightColumn)}
        onlive={(width) => (liveLeft = width)}
        oncommit={(width) => void changePreferences({ leftPanelWidth: width })}
        onreset={() =>
          void changePreferences({ leftPanelWidth: PANEL_WIDTHS.left })}
        onhide={() => togglePanel("left")}
      />
    </div>
  {/if}
  {#if rightShown}
    <div class="edge" style:right="{rightWidth}px">
      <PanelResizer
        side="right"
        width={rightWidth}
        min={PANEL_WIDTHS.min}
        max={widest(leftColumn)}
        onlive={(width) => (liveRight = width)}
        oncommit={(width) => void changePreferences({ rightPanelWidth: width })}
        onreset={() =>
          void changePreferences({ rightPanelWidth: PANEL_WIDTHS.right })}
        onhide={() => togglePanel("right")}
      />
    </div>
  {/if}
</div>

<UpdateNotice live={liveSessions} />

{#if inventory.palette}
  <CommandPalette
    onconnect={prompt}
    onsnippet={(snippet) => void sendSnippet(snippet)}
    target={snippetTarget}
    onclose={() => (inventory.palette = false)}
  />
{/if}

{#if inventory.csvImport !== null && inventory.workspace}
  <CsvImportDialog
    folder={inventory.csvImport}
    onclose={() => {
      inventory.csvImport = null;
      focusRequest += 1;
    }}
  />
{/if}

{#if inventory.nornirImport !== null && inventory.workspace}
  <NornirImportDialog
    folder={inventory.nornirImport}
    onclose={() => {
      inventory.nornirImport = null;
      focusRequest += 1;
    }}
  />
{/if}

{#if switcher}
  <WorkspacesDialog
    onswitch={switchWorkspace}
    onclose={() => {
      switcher = false;
      focusRequest += 1;
    }}
  />
{/if}

{#if switching}
  {@const { from, live, decide } = switching}
  <ConfirmSwitch
    {from}
    {live}
    onkeep={() => decide("keep")}
    onclosesessions={() => decide("close")}
    oncancel={() => decide("cancel")}
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

{#if snippets.editing}
  {@const editing = snippets.editing}
  <SnippetDialog
    previous={editing.previous}
    initial={editing.snippet}
    onclose={() => {
      snippets.editing = null;
      focusRequest += 1;
    }}
  />
{/if}

{#if asking}
  {@const { tab, name, names } = asking}
  <SnippetVariables
    snippet={name}
    target={sessions[tab]?.name ?? ""}
    {names}
    onsend={(answers) => {
      // Read before clearing `asking`, which these are derived from.
      const send = { name, tab };
      asking = null;
      void sendSnippet(send.name, answers, send.tab);
    }}
    oncancel={() => {
      asking = null;
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
    position: relative;
    height: 100%;
    display: grid;
  }

  /* Holds a panel's resize handle, on the panel's inner edge. */
  .edge {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 0;
  }

  /* A folded panel: a thin strip that brings it back. */
  .reveal {
    min-width: 0;
    padding: 0;
    border: none;
    border-radius: 0;
    background: var(--bg);
    color: var(--muted);
    font-size: 0.75rem;
    cursor: pointer;
  }

  .reveal[data-reveal-panel="left"] {
    border-right: 1px solid var(--border);
  }

  .reveal[data-reveal-panel="right"] {
    border-left: 1px solid var(--border);
  }

  .reveal:hover {
    color: inherit;
    background: var(--border);
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
