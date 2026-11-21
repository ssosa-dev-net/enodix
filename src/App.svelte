<script lang="ts">
  import { tick, untrack } from "svelte";
  import FolderOpen from "@lucide/svelte/icons/folder-open";
  import FolderPlus from "@lucide/svelte/icons/folder-plus";
  import PanelLeft from "@lucide/svelte/icons/panel-left";
  import ServerPlus from "@lucide/svelte/icons/server-plus";
  import X from "@lucide/svelte/icons/x";
  import ActivityRail, {
    SIDE_VIEW_ICONS,
    SIDE_VIEWS,
  } from "./lib/ActivityRail.svelte";
  import CommandPalette, {
    type PaletteAction,
  } from "./lib/CommandPalette.svelte";
  import ConfirmClose from "./lib/ConfirmClose.svelte";
  import ConfirmJoin from "./lib/ConfirmJoin.svelte";
  import PasteConfirm, { type PasteMember } from "./lib/PasteConfirm.svelte";
  import ConfirmSwitch from "./lib/ConfirmSwitch.svelte";
  import ConfirmQuit from "./lib/ConfirmQuit.svelte";
  import { activeTransfers } from "./lib/fileTransfers.svelte";
  import { addressBesides, hostConnection } from "./lib/hostConnect";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import CredentialsPrompt from "./lib/CredentialsPrompt.svelte";
  import CredentialsView from "./lib/CredentialsView.svelte";
  import Dialog from "./lib/Dialog.svelte";
  import DetailsSheet from "./lib/DetailsSheet.svelte";
  import HighlightRules from "./lib/HighlightRules.svelte";
  import HostsView from "./lib/HostsView.svelte";
  import PaneView, {
    type PaneHandlers,
    type TabInfo,
  } from "./lib/PaneView.svelte";
  import PanelResizer from "./lib/PanelResizer.svelte";
  import { PaneRects, sessionWindowSize } from "./lib/paneRects.svelte";
  import {
    trackSessionWindows,
    windowMoves,
  } from "./lib/sessionWindows.svelte";
  import BackupsView from "./lib/BackupsView.svelte";
  import PreferencesView from "./lib/PreferencesView.svelte";
  import { openRunTab, setViewOpener, VIEWS } from "./lib/views";
  import {
    closeDetachedWindows,
    closeMainWindow,
    closeWorkspaceSessionsElsewhere,
    liveInSessionWindows,
    onBackUpHost,
    onDockConfigs,
    onDockRun,
    MAIN_WINDOW_ID,
    onMainWindowRequest,
    onSessionArrived,
    onSnippetTarget,
    openSessionWindow,
    sendSnippetTo,
    snippetTarget as readSnippetTarget,
    terminalUsed,
    type SnippetTarget,
  } from "./lib/windows";
  import PythonSetup from "./lib/PythonSetup.svelte";
  import { refreshPython } from "./lib/python.svelte";
  import Platforms from "./lib/Platforms.svelte";
  import QuickConnect from "./lib/QuickConnect.svelte";
  import SnippetsView from "./lib/SnippetsView.svelte";
  import SnippetVariables from "./lib/SnippetVariables.svelte";
  import TerminalSession from "./lib/TerminalSession.svelte";
  import SessionMenu, { type SessionAction } from "./lib/SessionMenu.svelte";
  import StatusLine from "./lib/StatusLine.svelte";
  import { jumpHostOf } from "./lib/ways";
  import UnlockVault from "./lib/UnlockVault.svelte";
  import UpdateNotice from "./lib/UpdateNotice.svelte";
  import { startUpdates } from "./lib/updates.svelte";
  import { vaultStatus } from "./lib/vault";
  import WorkspacesDialog from "./lib/WorkspacesDialog.svelte";
  import Welcome from "./lib/Welcome.svelte";
  import CsvImport from "./lib/CsvImport.svelte";
  import NornirImport from "./lib/NornirImport.svelte";
  import JobTab from "./lib/JobTab.svelte";
  import JobsSideView from "./lib/JobsSideView.svelte";
  import RunView from "./lib/RunView.svelte";
  import EnrichReview from "./lib/EnrichReview.svelte";
  import ConfigHistory from "./lib/ConfigHistory.svelte";
  import FilesTab from "./lib/FilesTab.svelte";
  import { stepTitle } from "./lib/jobs";
  import {
    liveJob,
    runningCount,
    startLiveJobs,
    type LiveJob,
  } from "./lib/jobs.svelte";
  import { forgetClosedDrafts, jobDrafts } from "./lib/jobTabs.svelte";
  import {
    hostRole,
    hostSafety,
    inventory,
    newFolder,
    newHost,
    onHostsMoved,
    openBackup,
    openConfigHistory,
    openCsvImport,
    openDetails,
    openFiles,
    openNornirImport,
    select,
    selectedFolder,
    selectedHost,
    setWorkspace,
    targetFolder,
    trackFilterView,
    workspaceChanged,
    type Item,
  } from "./lib/inventory.svelte";
  import { hostLogin } from "./lib/credentials";
  import { trackHighlightRules } from "./lib/highlight.svelte";
  import { profiles, refreshProfiles } from "./lib/profiles.svelte";
  import { joinConfirmations, NO_SAFETY } from "./lib/paste";
  import {
    expandForMembers,
    GroupPaste,
    onLayoutChanged,
  } from "./lib/groupPaste";
  import {
    changePreferences,
    fontStack,
    isDark,
    loadPreferences,
    PANEL_WIDTHS,
    preferences,
    zoomFont,
    type SideView,
  } from "./lib/preferences.svelte";
  import {
    appName,
    type ConnectTarget,
    type QuickConnectTarget,
    type SerialParameters,
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
    type ViewRef,
    viewOf,
  } from "./lib/layout";
  import {
    expandSnippet,
    findHost,
    onWorkspaceChanged,
    resolveSettings,
    restoreWorkspace,
    type Environment,
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
    /** The host's address, shown beside the title; null: none. */
    address: string | null;
    /** The jump host it goes through, once known; null: none. */
    jumpHost: string | null;
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
  /** Each tab's terminal size, for the status line. */
  let sizes = $state<Record<TabId, { cols: number; rows: number }>>({});
  /** The tab whose context menu is open, with its entries. */
  let tabMenu = $state<{
    x: number;
    y: number;
    entries: SessionAction[];
  } | null>(null);
  /** Raised when a tab is clicked, so that its terminal takes the keys. */
  let focusRequest = $state(0);
  /** The tab whose closing waits for the user's confirmation. */
  let closing = $state<TabId | null>(null);
  /** A Files tab whose closing would cancel its transfers, waiting for
   * the user's confirmation (#106). */
  let closingFiles = $state<{ tab: TabId; host: string } | null>(null);
  /** Each tab's terminal, for sending snippets to it. */
  const terminals: Record<TabId, TerminalSession | null> = {};
  /** Whether the workspace switcher is open. */
  let switcher = $state(false);
  /** A switch waiting for the user to keep or close the open sessions. */
  let switching = $state<{
    from: string;
    live: number;
    /** How many of them are in session windows (#88). */
    elsewhere: number;
    /** Transfers the Files tabs have running or waiting (#106). */
    transfers: number;
    decide: (choice: "keep" | "close" | "cancel") => void;
  } | null>(null);
  /** A snippet waiting for the user to give its variables a value. */
  let asking = $state<{ tab: TabId; name: string; names: string[] } | null>(
    null,
  );
  /** Tabs waiting for the user's yes before joining the group (#36). */
  let joining = $state<{
    /** The pane the question covers: the tab's, or the pane joining. */
    pane: PaneId | null;
    production: string[];
    count: number;
    answer: (join: boolean) => void;
  } | null>(null);
  /** A paste to the multi-send group waiting for the user's yes (#36). */
  let groupPasteQuestion = $state<{
    /** The tab pasted in, over whose terminal the question is shown. */
    tab: TabId;
    text: string;
    lineCount: number;
    environment: Environment | null;
    members: PasteMember[];
    answer: (send: boolean) => void;
  } | null>(null);

  // The group's paste question is shown over the tab pasted in: if that
  // tab goes or is hidden, the paste is cancelled (nothing waits on it).
  $effect(() => {
    const question = groupPasteQuestion;
    if (question && !placement.get(question.tab)?.shown) {
      question.answer(false);
    }
  });

  // The variables question is shown over its tab too: hidden or closed,
  // the snippet is not sent.
  $effect(() => {
    if (asking && !placement.get(asking.tab)?.shown) asking = null;
  });

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
  /** The active pane's session, as the status line shows it. */
  const statusSession = $derived.by(() => {
    const tab = activePane?.active ?? null;
    const session = tab === null ? undefined : sessions[tab];
    const size = tab === null ? undefined : sizes[tab];
    return session && size ? { name: session.name, ...size } : null;
  });
  /**
   * The terminal used last (#86), when it is in a session window: the
   * Snippets view and the palette send there instead.
   */
  let usedElsewhere = $state<SnippetTarget | null>(null);
  const elsewhere = $derived(
    usedElsewhere && usedElsewhere.window !== MAIN_WINDOW_ID
      ? usedElsewhere
      : null,
  );
  void readSnippetTarget().then(
    (target) => (usedElsewhere = target),
    () => {},
  );
  void onSnippetTarget((target) => (usedElsewhere = target));
  /** That tab's name, as the snippet panel and palette show it. */
  const snippetTarget = $derived(
    elsewhere
      ? elsewhere.member
        ? groupName()
        : elsewhere.name
      : snippetTab === null
        ? null
        : inGroup(snippetTab)
          ? groupName()
          : sessions[snippetTab].name,
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

  /** Shows `view` in a tab (its own if open). */
  function openViewTab(view: ViewRef) {
    focusRequest += 1;
    void edit({ kind: "openView", view: view.view, subject: view.subject });
  }
  setViewOpener(openViewTab);

  // A host renamed or moved (#99): the sessions to it and its config
  // history tab follow it to its new id.
  $effect(() =>
    onHostsMoved((moves, root) => {
      untrack(() => {
        const tree = inventory.workspace?.tree;
        for (const session of Object.values(sessions)) {
          if (session.workspace !== root || session.hostId === null) continue;
          const moved = moves.find((move) => move.from === session.hostId);
          if (!moved) continue;
          session.hostId = moved.to;
          const host = tree ? findHost(tree, moved.to) : null;
          if (session.profileLogin && host) session.profileLogin.host = host;
        }
        for (const item of viewTabs) {
          if (
            (item.view !== "configs" && item.view !== "files") ||
            item.subject === null
          )
            continue;
          const moved = moves.find((move) => move.from === item.subject);
          if (moved) {
            void edit({
              kind: "retargetView",
              view: item.view,
              from: moved.from,
              to: moved.to,
            });
          }
        }
      });
    }),
  );

  /** The view tabs, by tab id, as the core's layout names them. */
  const viewTabs = $derived(
    layout
      ? Object.keys(layout.kinds).flatMap((key) => {
          const tab = Number(key);
          const shows = viewOf(layout!, tab);
          return shows ? [{ tab, ...shows }] : [];
        })
      : [],
  );

  /** Drops the sessions of tabs no longer in the layout (their terminals go),
   * and the drafts of closed job tabs. */
  function forgetClosedTabs() {
    forgetClosedDrafts(
      new Set(
        viewTabs.flatMap((item) =>
          item.view === "job" && item.subject ? [item.subject] : [],
        ),
      ),
    );
    for (const key of Object.keys(sessions)) {
      const tab = Number(key);
      if (!placement.has(tab)) {
        delete sessions[tab];
        delete states[tab];
        delete sizes[tab];
      }
    }
  }

  // Terminals are placed over their pane's body rather than inside it, so
  // that moving a tab to another pane keeps its terminal (and session).
  const panesArea = new PaneRects();
  /** Where each pane's body is, relative to the panes' area. */
  const rects = $derived(panesArea.rects);

  // Panes can move without changing size (another pane closing).
  $effect(() => {
    if (layout) void tick().then(() => panesArea.measure());
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

  /** The group, as the snippet panel and questions name it. */
  function groupName(): string {
    return `the multi-send group (${layout?.groupSize ?? 0})`;
  }

  /** Adds `tab` to the multi-send group, or takes it out. View tabs never
   * join (the core refuses them too). */
  function toggleGroup(tab: TabId) {
    if (!sessions[tab]) return;
    if (inGroup(tab)) void edit({ kind: "leaveGroup", tab });
    else void join([tab], { kind: "joinGroup", tab });
  }

  /** Adds every tab of `pane` to the multi-send group. */
  function joinPane(pane: PaneId) {
    const joining = () =>
      (panes.find((each) => each.id === pane)?.tabs ?? []).filter(
        (tab) => sessions[tab] && !inGroup(tab),
      );
    void join(joining(), { kind: "joinPane", pane }, joining);
  }

  /**
   * Makes the layout edit `change`, which adds `tabs` to the multi-send
   * group, once the user agrees if the core says any of them must be
   * confirmed (production sessions): one question names them all. If
   * `current` (what the edit would add now) gained a tab while the user
   * was asked (one dragged into the pane), they are asked again.
   */
  async function join(
    tabs: TabId[],
    change: LayoutEdit,
    current?: () => TabId[],
  ) {
    let production: TabId[];
    try {
      production = await joinConfirmations(
        tabs
          .filter((tab) => sessions[tab])
          .map((tab) => ({ tab, safety: safetyOf(sessions[tab]) })),
      );
    } catch {
      return;
    }
    if (production.length > 0) {
      const agreed = await new Promise<boolean>((answer) => {
        joining = {
          pane:
            change.kind === "joinPane"
              ? change.pane
              : (placement.get(tabs[0])?.pane ?? null),
          production: production.map((tab) => sessions[tab]?.name ?? ""),
          count: tabs.length,
          answer,
        };
      });
      joining = null;
      focusRequest += 1;
      if (!agreed) return;
      const now = current?.() ?? tabs;
      if (now.some((tab) => !tabs.includes(tab))) {
        return join(now, change, current);
      }
    }
    await edit(change);
  }

  /**
   * Pastes into every member of the multi-send group, in any window, as
   * pasted in a member of this one (#36, #87): the core plans it once for
   * all; if any member's own rules would ask, one question over the tab
   * pasted in shows the text, the line count and the members (as for
   * production if one is). Then each member gets its lines through its
   * own terminal, with its own delay; one that ends stops, the rest go on.
   */
  const groupPaste = new GroupPaste({
    terminals,
    ready: (tab) => !!sessions[tab] && states[tab] === "connected",
    ask: async (tab, question) => {
      const send = await new Promise<boolean>((answer) => {
        groupPasteQuestion = { tab, ...question, answer };
      });
      groupPasteQuestion = null;
      focusRequest += 1;
      return send;
    },
    failed: (error) => {
      inventory.editError = `Cannot paste to the multi-send group: ${error}`;
    },
  });

  // The group changed in another window: its markers and size here too.
  void onLayoutChanged((next) => adopt(next));

  function tabState(tab: TabId): TabInfo["state"] {
    const state = states[tab] ?? "connecting";
    return state === "closed" ? "disconnected" : state;
  }

  const handlers: PaneHandlers = {
    tab(tab) {
      const shows = layout ? viewOf(layout, tab) : null;
      if (shows) {
        const name = VIEWS[shows.view].title(shows.subject);
        return {
          view: shows.view,
          name,
          title: name,
          role: null,
          // A host's Files tab is coloured as its sessions are (#103).
          environment:
            shows.view === "files" && shows.subject
              ? (hostSafety(shows.subject)?.environment ?? null)
              : null,
          state: "connected",
          member: false,
        };
      }
      const session = sessions[tab];
      if (!session) return null;
      return {
        view: null,
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
    refocus: () => (focusRequest += 1),
    split: (pane, direction) => void edit({ kind: "split", pane, direction }),
    closePane: (pane) => void edit({ kind: "closePane", pane }),
    joinPane,
    menu: (tab, x, y) => {
      const actions = sessions[tab] ? (terminals[tab]?.actions() ?? []) : [];
      tabMenu = {
        x,
        y,
        entries: [
          ...actions,
          {
            id: "close-tab",
            label: "Close tab",
            shortcut: "Ctrl+Shift+W",
            run: () => closeTab(tab),
          },
        ],
      };
    },
    body: (pane) => panesArea.body(pane),
  };

  /** Closes `tab`, first asking if its session is still live, or if it
   * is a Files tab with transfers running or waiting. */
  function closeTab(tab: TabId) {
    const shows = layout ? viewOf(layout, tab) : null;
    if (shows) {
      if (
        shows.view === "files" &&
        shows.subject &&
        activeTransfers(shows.subject) > 0
      )
        closingFiles = { tab, host: shows.subject };
      else closeViewTab(tab);
      return;
    }
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

  /** The job still running that the view tab `tab` shows (a job tab once
   * started, or a run's tab), if any. */
  function runningJobOf(tab: TabId): LiveJob | undefined {
    const shows = layout ? viewOf(layout, tab) : null;
    if (!shows?.subject) return undefined;
    const run =
      shows.view === "job"
        ? jobDrafts[shows.subject]?.run
        : shows.view === "run"
          ? shows.subject
          : null;
    const job = run ? liveJob(run) : undefined;
    return job && !job.finished ? job : undefined;
  }

  /** What closing a running job's tab says: the job goes on. */
  let jobNotice = $state<{ run: string; text: string } | null>(null);
  let jobNoticeTimer: ReturnType<typeof setTimeout> | undefined;

  /** Closes the view tab `tab`, never asking. A job it shows goes on
   * running, and (unless `quiet`) a notice says so. */
  function closeViewTab(tab: TabId, quiet = false) {
    const job = quiet ? undefined : runningJobOf(tab);
    if (job) {
      jobNotice = {
        run: job.run,
        text: `${stepTitle(job.plan.step)} on ${job.label} goes on running.`,
      };
      clearTimeout(jobNoticeTimer);
      jobNoticeTimer = setTimeout(() => (jobNotice = null), 10000);
    }
    void edit({ kind: "closeTab", tab });
  }

  // Job, run, config history, import and fill-in review tabs belong to the
  // workspace they were opened in (host ids, run ids and folders mean
  // nothing elsewhere): switching closes them. Their jobs go on running.
  let shownRoot: string | null | undefined = undefined;
  $effect(() => {
    const root = inventory.workspace?.root ?? null;
    untrack(() => {
      if (shownRoot !== undefined && root !== shownRoot) {
        for (const item of viewTabs) {
          if (
            item.view === "job" ||
            item.view === "run" ||
            item.view === "configs" ||
            item.view === "files" ||
            item.view === "csv-import" ||
            item.view === "nornir-import" ||
            item.view === "enrich"
          )
            void edit({ kind: "closeTab", tab: item.tab });
        }
        // Runs in windows of their own too (#77).
        void closeDetachedWindows().catch(() => {});
      }
      shownRoot = root;
    });
  });

  function confirmClose() {
    const tab = closing;
    closing = null;
    if (tab !== null) void edit({ kind: "closeTab", tab });
  }

  const host = $derived(selectedHost());
  const folder = $derived(selectedFolder());

  // The rail and the side view it selects (its width and whether it is
  // folded are the left panel's preferences): the width from the
  // preferences, or the one being dragged. There is no right column: host
  // and folder details are a sheet over the Hosts view (#64).
  let liveLeft = $state<number | null>(null);
  let windowWidth = $state(window.innerWidth);
  /** The terminal area keeps at least this much when the side view widens. */
  const MIN_CENTRE = 320;
  /** The activity rail's width. */
  const RAIL = 48;
  const sideView = $derived(preferences.current.sideView);
  const leftShown = $derived(!preferences.current.leftPanelHidden);
  const leftWidth = $derived(liveLeft ?? preferences.current.leftPanelWidth);
  /** How wide the side view may get without squeezing the terminal. */
  const widest = $derived(
    Math.min(PANEL_WIDTHS.max, windowWidth - RAIL - MIN_CENTRE),
  );
  const columns = $derived(
    [`${RAIL}px`, leftShown ? `${leftWidth}px` : null, "minmax(0, 1fr)"]
      .filter(Boolean)
      .join(" "),
  );

  function toggleSideView() {
    void changePreferences({
      leftPanelHidden: !preferences.current.leftPanelHidden,
    });
  }

  /** The details sheet is shown: open, with something selected to show,
   * over the Hosts view. */
  const detailsShown = $derived(
    inventory.details &&
      Boolean(host || folder) &&
      sideView === "hosts" &&
      leftShown,
  );

  // The sheet closes with what it showed (deleted, say) and with the Hosts
  // view (another view chosen, or the side view folded).
  $effect(() => {
    if (inventory.details && !detailsShown) inventory.details = false;
  });

  /** Opens the details sheet on `item` (the selection if none), bringing
   * back the Hosts view it lies over. */
  function showDetails(item: Item | null = inventory.selected) {
    if (!item) return;
    if (sideView !== "hosts" || !leftShown) {
      void changePreferences({ sideView: "hosts", leftPanelHidden: false });
    }
    openDetails(item);
  }

  function closeDetails() {
    inventory.details = false;
    focusRequest += 1;
  }

  /** What a rail icon (or its shortcut) does: shows `view`, or folds the
   * side view if it already shows it. */
  function selectView(view: SideView) {
    if (view === sideView && leftShown) {
      void changePreferences({ leftPanelHidden: true });
    } else {
      void changePreferences({ sideView: view, leftPanelHidden: false });
    }
  }

  /** What the palette reaches besides hosts and snippets (#76): every
   * side view, the view tabs that need no subject, creating, importing and
   * switching the workspace. New hosts, folders and imports go to the
   * selected folder (or the selected host's), as the add control's. */
  const paletteActions: PaletteAction[] = [
    ...SIDE_VIEWS.map((each) => ({
      id: `side-${each.view}`,
      label: each.name,
      words: `side view show ${each.view}`,
      detail: `Side view · Ctrl+Shift+${each.key}`,
      icon: SIDE_VIEW_ICONS[each.view],
      run: () =>
        void changePreferences({ sideView: each.view, leftPanelHidden: false }),
    })),
    {
      id: "fold-side-view",
      label: "Fold or unfold the side view",
      words: "hide show toggle panel",
      detail: "Ctrl+Shift+B",
      icon: PanelLeft,
      run: toggleSideView,
    },
    {
      id: "new-host",
      label: "New host",
      words: "add create device",
      detail: "In the selected folder",
      icon: ServerPlus,
      run: () => newHost(targetFolder()),
    },
    {
      id: "new-folder",
      label: "New folder",
      words: "add create",
      detail: "In the selected folder",
      icon: FolderPlus,
      run: () => newFolder(targetFolder()),
    },
    {
      id: "csv-import",
      label: "Import from CSV",
      words: "hosts spreadsheet add",
      detail: "Open in a tab",
      icon: VIEWS["csv-import"].icon,
      run: () => openCsvImport(targetFolder()),
    },
    {
      id: "nornir-import",
      label: "Import from Nornir",
      words: "hosts inventory yaml add",
      detail: "Open in a tab",
      icon: VIEWS["nornir-import"].icon,
      run: () => openNornirImport(targetFolder()),
    },
    {
      id: "switch-workspace",
      label: "Switch workspace",
      words: "open create add workspaces folder",
      detail: "Ctrl+Shift+O",
      icon: FolderOpen,
      run: () => (switcher = true),
    },
    ...(
      [
        [
          "preferences",
          "Preferences",
          "settings theme font scrollback python updates",
          "Ctrl+,",
        ],
        [
          "platforms",
          "Platform definitions",
          "platforms prompts paging privilege show config devices jobs",
          "Open in a tab",
        ],
        [
          "highlights",
          "Highlight rules",
          "highlighting keywords patterns colours colors",
          "Open in a tab",
        ],
        [
          "python-setup",
          "Python setup",
          "environment interpreter scripts nornir netmiko napalm",
          "Open in a tab",
        ],
      ] as const
    ).map(([view, label, words, detail]) => ({
      id: view,
      label,
      words,
      detail,
      icon: VIEWS[view].icon,
      run: () => openViewTab({ view, subject: null }),
    })),
  ];

  // The tree's filters apply whichever side view is shown.
  trackFilterView();

  void loadPreferences();
  // Jobs are the shell's (#77): what already ran shows after a reload.
  void startLiveJobs();
  // A run docked back from its own window opens as a tab here.
  void onDockRun((run) => {
    focusRequest += 1;
    openRunTab(run);
  });
  // A config history docked back likewise (#78), and its Back up now
  // opens the backup's tab here.
  void onDockConfigs((host) => {
    focusRequest += 1;
    openConfigHistory(host);
  });
  void onBackUpHost((host) => {
    const tree = inventory.workspace?.tree;
    openBackup(
      { kind: "hosts", ids: [host] },
      (tree && findHost(tree, host)?.name) ?? host,
    );
  });
  trackHighlightRules();
  // Where a session can move: Open in window offers each session window.
  trackSessionWindows();
  void startUpdates();
  // Python features stay off until an interpreter is chosen and works.
  void refreshPython();

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
    // Text shown in monospace (outputs, configs) in the terminal font,
    // here and in detached windows.
    document.documentElement.style.setProperty(
      "--font-mono",
      fontStack(preferences.current.fontFamily),
    );
  });

  appName().then((value) => {
    name = value;
    document.title = value;
  });

  /** Closing the main window while sessions are live (#88), transfers
   * (#106) or jobs (#124) run, waiting for the user's answer. */
  let quitting = $state<{
    live: number;
    elsewhere: number;
    transfers: number;
    jobs: number;
  } | null>(null);

  // Closing the main window (its Close button, Alt+F4, the taskbar) closes
  // every window and every session, and cancels every job (#124): asked
  // once if a session is opening or connected in any window, a transfer or
  // a job runs, else at once.
  void getCurrentWindow().onCloseRequested(async (event) => {
    event.preventDefault();
    if (quitting) return;
    const elsewhere = await liveInSessionWindows();
    const live = liveSessions + elsewhere;
    // Files tabs are in this window only (#106).
    const transfers = activeTransfers();
    // Jobs of every workspace: the shell keeps them all.
    const jobs = runningCount() + runningCount(true);
    if (live + transfers + jobs === 0) await closeMainWindow();
    else quitting = { live, elsewhere, transfers, jobs };
  });

  // Changes from outside the app (a git pull) show without a restart. The
  // window lives as long as the app, so the listener is never removed.
  void onWorkspaceChanged(workspaceChanged);

  /** With a vault, it is unlocked before the workspace opens. */
  let vaultLocked = $state(false);

  function reopenWorkspace() {
    restoreWorkspace().then(setWorkspace, (error) => {
      inventory.openError = `Could not reopen the last workspace: ${error}`;
    });
  }

  vaultStatus().then(
    (status) => {
      if (status === "locked") vaultLocked = true;
      else reopenWorkspace();
    },
    () => reopenWorkspace(),
  );

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
    // Those in session windows are asked about alike (#88).
    const elsewhere = current ? await liveInSessionWindows(current.root) : 0;
    // Files tabs close with the workspace, cancelling their transfers.
    const transfers = current ? activeTransfers() : 0;
    let choice: "keep" | "close" | "cancel" = "keep";
    if (current && live.length + elsewhere + transfers > 0) {
      choice = await new Promise((decide) => {
        switching = {
          from: current.tree.name,
          live: live.length + elsewhere,
          elsewhere,
          transfers,
          decide,
        };
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
    if (choice === "close" && current && next.root !== current.root) {
      for (const tab of tabs) await edit({ kind: "closeTab", tab });
      await closeWorkspaceSessionsElsewhere(current.root).catch((error) => {
        inventory.editError = `Cannot close the sessions in session windows: ${error}`;
      });
    }
    setWorkspace(next);
    inventory.openError = null;
    switcher = false;
    focusRequest += 1;
    return null;
  }

  /** Quick connect over SSH, without an inventory host. */
  function quickConnect(target: QuickConnectTarget) {
    connect(
      { kind: "quick", ...target },
      target.host,
      `${target.username}@${target.host}:${target.port}`,
    );
  }

  /** A serial console on `port`, without an inventory host. */
  function quickSerial(port: string, parameters: SerialParameters) {
    connect(
      { kind: "quickSerial", port, parameters },
      port,
      `${port} (${parameters.baudRate} ${parameters.dataBits}${parameters.parity[0].toUpperCase()}${parameters.stopBits})`,
    );
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
    const host =
      hostId && inventory.workspace
        ? findHost(inventory.workspace.tree, hostId)
        : null;
    void edit({ kind: "openTab" }, (tab) => {
      sessions[tab] = {
        source: { kind: "target", target, name, title },
        name,
        title,
        address: host ? addressBesides(host, title) : null,
        jumpHost: null,
        hostId,
        workspace,
        safety: (hostId ? hostSafety(hostId) : null) ?? NO_SAFETY,
        role: hostId ? hostRole(hostId) : null,
        profileLogin,
      };
      // Telnet and serial never go through one.
      if (hostId && target.kind === "host") {
        resolveSettings({ kind: "host", id: hostId }).then(
          (settings) => {
            const jump = jumpHostOf(hostId, settings);
            if (sessions[tab]) sessions[tab].jumpHost = jump;
          },
          () => {
            // The header then names no jump host; connecting says more.
          },
        );
      }
    });
  }

  /**
   * Moves `tab`'s session to a window of its own (#84), or into the open
   * session window `into` (#85), unless its terminal refuses now (a
   * question or a paste would be lost). A new window is sized so that its
   * terminal keeps its columns and rows.
   */
  async function openInWindow(tab: TabId, into: number | null = null) {
    const terminal = terminals[tab];
    const place = placement.get(tab);
    const rect = place ? rects[place.pane] : undefined;
    if (!terminal || !sessions[tab] || !rect) return;
    if (terminal.moveRefusal() !== null) return;
    const given = await terminal.handOver();
    if (!given) return;
    try {
      const shown = await openSessionWindow(
        tab,
        given.id,
        given.snapshot,
        sessionWindowSize(rect),
        into,
      );
      if (shown) adopt(shown);
      forgetClosedTabs();
    } catch (error) {
      await terminal.takeBack().catch(() => {});
      inventory.editError = `The session could not move to a window of its own: ${error}`;
    }
  }

  /** What the window holds about `tab`'s session that a move would lose. */
  function windowQuestionOf(tab: TabId): string | null {
    if (asking?.tab === tab)
      return "Not while the snippet's variables are asked";
    if (groupPasteQuestion?.tab === tab)
      return "Not while a question about the session is open";
    return null;
  }

  // A session docked back from its window, or taken from a window that
  // died (without its earlier output), gets a tab here (#84), shown as
  // the app's record of it says.
  void onSessionArrived((arrived) => {
    const record = arrived.record;
    const name = record?.name ?? `Session ${arrived.session}`;
    sessions[arrived.tab] = {
      source: {
        kind: "adopt",
        id: arrived.session,
        snapshot: arrived.snapshot,
      },
      name,
      title: record?.title ?? name,
      address: record?.address ?? null,
      jumpHost: record?.jumpHost ?? null,
      hostId: record?.host ?? null,
      workspace: record?.workspace ?? null,
      safety: record?.safety ?? NO_SAFETY,
      role: record?.role ?? null,
    };
    adopt(arrived.layout);
    focusRequest += 1;
  });

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
    select({ kind: "host", id: host.id });
    let login;
    try {
      login = await hostLogin(host.id, protocol);
    } catch (error) {
      // The profiles cannot be read (the keychain refused, say): say so,
      // and let the user log in by hand meanwhile.
      inventory.editError = `Cannot read the credential profiles: ${error}`;
      login = { kind: "prompt" } as const;
    }
    const connection = hostConnection(host, login);
    if (connection) {
      const { target, title, username } = connection;
      connect(
        target,
        host.name,
        title,
        username === null ? undefined : { host, username },
      );
    } else {
      const missingProfile = login.kind === "notSetUp" ? login.profile : null;
      void showConnectView({ kind: "prompt", host, protocol, missingProfile });
    }
  }

  /** Opens the session's host's Files tab: an SSH session to a host of
   * the open workspace (#103). */
  function filesOf(session: OpenSession) {
    const id = liveHost(session);
    if (id === null || !inventory.workspace) return undefined;
    return () => openFiles(id);
  }

  /** Opens the details sheet on the session's host, if it is in the open
   * workspace. */
  function detailsOf(session: OpenSession) {
    const id = liveHost(session);
    if (id === null || !inventory.workspace) return undefined;
    return () => showDetails({ kind: "host", id });
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
   * Sends the snippet `name` from the Snippets view or the palette to the
   * terminal used last (#86): one in a session window fills it in there,
   * through that terminal's own paste path; else as `sendSnippet`.
   */
  async function sendSnippetFromView(name: string) {
    const target = elsewhere;
    if (!target) {
      await sendSnippet(name);
      return;
    }
    try {
      await sendSnippetTo(target, name);
    } catch (error) {
      inventory.editError = `Cannot send ${name} to ${target.name}: ${error}`;
    }
  }

  // A session window asks for something only this window has (#86): the
  // details sheet on a session's host, or the password login after its
  // SSH agent did not work. Only for a host of the open workspace.
  void onMainWindowRequest(async (request) => {
    const tree = inventory.workspace?.tree;
    const host = tree ? findHost(tree, request.host) : null;
    if (!host) {
      inventory.editError = `${request.record?.name ?? "The session"}: its host is not in the open workspace.`;
      return;
    }
    if (request.kind === "details") {
      showDetails({ kind: "host", id: host.id });
      return;
    }
    if (request.kind === "files") {
      openFiles(host.id);
      return;
    }
    // From a session window's palette (#88): a host whose credentials are
    // asked here.
    if (request.kind === "connect") {
      await prompt(host);
      return;
    }
    let username: string | null = null;
    try {
      const login = await hostLogin(host.id, "ssh");
      if (login.kind === "profile") username = login.username;
    } catch {
      // The profiles cannot be read: log in by hand.
    }
    await showConnectView({
      kind: "prompt",
      host,
      protocol: "ssh",
      missingProfile: null,
      fallback:
        username === null ? undefined : { reason: request.reason, username },
    });
  });

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
    if (inGroup(tab)) {
      await sendSnippetToGroup(name, answers, tab);
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

  /**
   * Sends the snippet `name` to every connected member of the multi-send
   * group, as sent from `tab` (#36): the core fills it with each member's
   * own host values (a member without a live host gets only the answers);
   * variables some member lacks are asked once, for all. Then it goes
   * through the group's paste path.
   */
  async function sendSnippetToGroup(
    name: string,
    answers: Record<string, string>,
    tab: TabId,
  ) {
    let members;
    let expansion;
    try {
      members = await groupPaste.receiving(tab);
      expansion = await expandForMembers(name, members, answers);
    } catch (error) {
      inventory.editError = `Cannot send ${name}: ${error}`;
      return;
    }
    if (expansion.kind === "missing") {
      asking = { tab, name, names: expansion.names };
      return;
    }
    groupPaste.pasteEach(tab, members, expansion.texts);
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
    // Open in window (#84): the active session, if it may move now.
    if (event.shiftKey && letter("N")) {
      return tab === null || !sessions[tab]
        ? null
        : () => void openInWindow(tab);
    }
    // Not Ctrl+B: that is tmux's prefix, so it must reach devices.
    if (event.shiftKey && letter("B")) {
      return () => toggleSideView();
    }
    // Each side view: Ctrl+Shift and a letter no terminal program needs.
    const chosen = event.shiftKey
      ? SIDE_VIEWS.find((each) => letter(each.key))
      : undefined;
    if (chosen) return () => selectView(chosen.view);
    if (!event.shiftKey && (event.code === "Comma" || event.key === ",")) {
      return () => openViewTab({ view: "preferences", subject: null });
    }
    // Opens or closes the details sheet on the selection.
    if (event.shiftKey && letter("D")) {
      if (inventory.details) return closeDetails;
      return inventory.selected ? () => showDetails() : null;
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
    // The unlock screen takes every key.
    if (vaultLocked) return;
    const busy =
      inventory.dialog ||
      inventory.palette ||
      switcher ||
      switching ||
      quitting ||
      closingFiles !== null ||
      joining ||
      closing !== null;
    const action = zoomShortcut(event) ?? (busy ? null : shortcut(event));
    if (!action) return;
    event.preventDefault();
    event.stopPropagation();
    action();
  }
</script>

<svelte:window {onkeydowncapture} bind:innerWidth={windowWidth} />

<div class="window">
  <div class="layout" style:grid-template-columns={columns}>
    <ActivityRail
      view={sideView}
      folded={!leftShown}
      problems={inventory.workspace?.problems.length ?? 0}
      missingProfiles={profiles.view.missing.length}
      onselect={selectView}
      onpreferences={() => openViewTab({ view: "preferences", subject: null })}
    />
    {#if leftShown}
      <aside class="side-view" data-side-view={sideView}>
        {#if sideView === "hosts"}
          <div class="hosts-area" class:under-sheet={detailsShown}>
            <HostsView
              onopen={() => (switcher = true)}
              onconnect={prompt}
              ondetails={showDetails}
            />
          </div>
          {#if detailsShown}
            <DetailsSheet
              {host}
              {folder}
              onconnect={prompt}
              onclose={closeDetails}
            />
          {/if}
        {:else if sideView === "snippets"}
          <SnippetsView
            target={snippetTarget}
            onsend={(snippet) => void sendSnippetFromView(snippet)}
          />
        {:else if sideView === "jobs"}
          <JobsSideView />
        {:else if sideView === "credentials"}
          <CredentialsView />
        {:else}
          <BackupsView />
        {/if}
      </aside>
    {/if}

    <section class="centre" bind:this={panesArea.area}>
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
              // Focus is already where the user put it (perhaps a question
              // over this pane): no focus request, which would hand the
              // keys back to the pane active until then.
              if (!focused && place) void edit({ kind: "activate", tab });
              void terminalUsed(tab);
            }}
          >
            <TerminalSession
              bind:this={terminals[tab]}
              source={session.source}
              {tab}
              groupSize={inGroup(tab) ? layout.groupSize : null}
              ontogglegroup={() => toggleGroup(tab)}
              onendgroup={() => void edit({ kind: "endGroup" })}
              ongrouppaste={(text) => groupPaste.paste(tab, text)}
              title={session.title}
              address={session.address}
              jumpHost={session.jumpHost}
              onsize={(size) => (sizes[tab] = size)}
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
              ondetails={detailsOf(session)}
              onfiles={filesOf(session)}
              moves={windowMoves(
                (into) => void openInWindow(tab, into),
                "Ctrl+Shift+N",
              )}
              windowQuestion={windowQuestionOf(tab)}
            />
            {#if groupPasteQuestion?.tab === tab}
              {@const question = groupPasteQuestion}
              <PasteConfirm
                text={question.text}
                lineCount={question.lineCount}
                target={session.name}
                environment={question.environment}
                lineDelayMs={0}
                members={question.members}
                onsend={() => question.answer(true)}
                oncancel={() => question.answer(false)}
              />
            {/if}
            {#if asking?.tab === tab}
              {@const { name: snippet, names } = asking}
              <SnippetVariables
                {snippet}
                target={inGroup(tab) ? groupName() : session.name}
                {names}
                onsend={(answers) => {
                  // Read before clearing `asking`, which these come from.
                  const send = { snippet, tab };
                  asking = null;
                  void sendSnippet(send.snippet, answers, send.tab);
                }}
                oncancel={() => {
                  asking = null;
                  focusRequest += 1;
                }}
              />
            {/if}
          </div>
        {/each}
        {#each viewTabs as item (item.tab)}
          {@const place = placement.get(item.tab)}
          {@const rect = place ? rects[place.pane] : undefined}
          <div
            class="slot view"
            data-tab={item.tab}
            data-pane={place?.pane}
            data-view-slot={item.view}
            data-view-active={(place?.shown &&
              place.pane === layout.activePane) ||
              undefined}
            hidden={!place?.shown || rect === undefined}
            style:left="{rect?.left ?? 0}px"
            style:top="{rect?.top ?? 0}px"
            style:width="{rect?.width ?? 0}px"
            style:height="{rect?.height ?? 0}px"
            onfocusin={() => {
              if (place && place.pane !== layout?.activePane)
                void edit({ kind: "activatePane", pane: place.pane });
            }}
          >
            {#if item.view === "preferences"}
              <PreferencesView />
            {:else if item.view === "job" && item.subject && jobDrafts[item.subject] && inventory.workspace}
              <JobTab
                subject={item.subject}
                onclose={(quiet) => closeViewTab(item.tab, quiet)}
              />
            {:else if item.view === "run" && item.subject && inventory.workspace}
              <RunView
                run={item.subject}
                onclose={() => closeViewTab(item.tab, true)}
              />
            {:else if item.view === "configs" && item.subject && inventory.workspace}
              <ConfigHistory
                host={item.subject}
                onclose={() => closeViewTab(item.tab, true)}
              />
            {:else if item.view === "files" && item.subject && inventory.workspace}
              <FilesTab host={item.subject} />
            {:else if item.view === "csv-import" && inventory.workspace}
              <CsvImport
                folder={inventory.csvImport ?? ""}
                onclose={() => closeViewTab(item.tab, true)}
              />
            {:else if item.view === "nornir-import" && inventory.workspace}
              <NornirImport
                folder={inventory.nornirImport ?? ""}
                onclose={() => closeViewTab(item.tab, true)}
              />
            {:else if item.view === "platforms" && inventory.workspace}
              <Platforms onclose={() => closeViewTab(item.tab, true)} />
            {:else if item.view === "highlights"}
              <HighlightRules onclose={() => closeViewTab(item.tab, true)} />
            {:else if item.view === "enrich" && item.subject && inventory.workspace}
              <EnrichReview
                run={item.subject}
                onclose={() => closeViewTab(item.tab, true)}
              />
            {:else if item.view === "python-setup"}
              <PythonSetup onclose={() => closeViewTab(item.tab, true)} />
            {/if}
          </div>
        {/each}
        {#if joining}
          {@const question = joining}
          {@const rect =
            question.pane === null ? undefined : rects[question.pane]}
          <div
            class="pane-question"
            data-pane-dialog={question.pane}
            style:left={rect ? `${rect.left}px` : "0"}
            style:top={rect ? `${rect.top}px` : "0"}
            style:width={rect ? `${rect.width}px` : "100%"}
            style:height={rect ? `${rect.height}px` : "100%"}
          >
            <ConfirmJoin
              production={question.production}
              joining={question.count}
              onjoin={() => question.answer(true)}
              oncancel={() => question.answer(false)}
            />
          </div>
        {/if}
        {#if closingFiles}
          {@const question = closingFiles}
          {@const pane = placement.get(question.tab)?.pane}
          {@const rect = pane === undefined ? undefined : rects[pane]}
          <div
            class="pane-question"
            data-pane-dialog={pane}
            style:left={rect ? `${rect.left}px` : "0"}
            style:top={rect ? `${rect.top}px` : "0"}
            style:width={rect ? `${rect.width}px` : "100%"}
            style:height={rect ? `${rect.height}px` : "100%"}
          >
            <ConfirmClose
              name={(inventory.workspace &&
                findHost(inventory.workspace.tree, question.host)?.name) ??
                question.host}
              state="connected"
              environment={hostSafety(question.host)?.environment ?? null}
              files
              transfers={activeTransfers(question.host)}
              onclose={() => {
                // Read before clearing: `question` follows `closingFiles`.
                const tab = question.tab;
                closingFiles = null;
                closeViewTab(tab);
              }}
              oncancel={() => {
                closingFiles = null;
                focusRequest += 1;
              }}
            />
          </div>
        {/if}
        {#if closing !== null && sessions[closing]}
          {@const tab = closing}
          {@const state = tabState(tab)}
          {@const pane = placement.get(tab)?.pane}
          {@const rect = pane === undefined ? undefined : rects[pane]}
          <div
            class="pane-question"
            data-pane-dialog={pane}
            style:left={rect ? `${rect.left}px` : "0"}
            style:top={rect ? `${rect.top}px` : "0"}
            style:width={rect ? `${rect.width}px` : "100%"}
            style:height={rect ? `${rect.height}px` : "100%"}
          >
            <ConfirmClose
              name={sessions[tab].name}
              state={state === "disconnected" ? "connected" : state}
              environment={safetyOf(sessions[tab]).environment}
              onclose={confirmClose}
              oncancel={() => {
                closing = null;
                focusRequest += 1;
              }}
            />
          </div>
        {/if}
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
            {:else if !inventory.workspace}
              <Welcome
                {name}
                onswitch={switchWorkspace}
                onconnect={quickConnect}
                onserial={quickSerial}
              />
            {:else}
              <QuickConnect
                {name}
                onpalette={() => (inventory.palette = true)}
                onconnect={quickConnect}
                onserial={quickSerial}
              />
            {/if}
          </div>
        {/if}
      {/if}
    </section>

    {#if leftShown}
      <div class="edge" style:left="{RAIL + leftWidth}px">
        <PanelResizer
          side="left"
          width={leftWidth}
          min={PANEL_WIDTHS.min}
          max={widest}
          onlive={(width) => (liveLeft = width)}
          oncommit={(width) =>
            void changePreferences({ leftPanelWidth: width })}
          onreset={() =>
            void changePreferences({ leftPanelWidth: PANEL_WIDTHS.left })}
          onhide={toggleSideView}
        />
      </div>
    {/if}
  </div>
  <StatusLine vault={profiles.vault} session={statusSession} />
</div>

{#if tabMenu}
  <SessionMenu
    entries={tabMenu.entries}
    x={tabMenu.x}
    y={tabMenu.y}
    onclose={() => {
      tabMenu = null;
      focusRequest += 1;
    }}
  />
{/if}

<UpdateNotice live={liveSessions} />

{#if inventory.palette}
  <CommandPalette
    onconnect={prompt}
    onsnippet={(snippet) => void sendSnippetFromView(snippet)}
    target={snippetTarget}
    actions={paletteActions}
    onclose={() => (inventory.palette = false)}
  />
{/if}

{#if jobNotice}
  {@const notice = jobNotice}
  <div class="job-notice" role="status" data-job-still-running={notice.run}>
    <span>{notice.text} Find it in Jobs.</span>
    <button
      type="button"
      data-job-notice="show"
      onclick={() => {
        jobNotice = null;
        void changePreferences({ sideView: "jobs", leftPanelHidden: false });
      }}>Show in Jobs</button
    >
    <button
      type="button"
      class="dismiss"
      data-job-notice="dismiss"
      aria-label="Dismiss"
      onclick={() => (jobNotice = null)}
      ><X size={14} aria-hidden="true" /></button
    >
  </div>
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

{#if quitting}
  {@const { live, elsewhere, transfers, jobs } = quitting}
  <ConfirmQuit
    product={name}
    {live}
    {elsewhere}
    {transfers}
    {jobs}
    onquit={() => void closeMainWindow()}
    oncancel={() => {
      quitting = null;
      focusRequest += 1;
    }}
  />
{/if}

{#if switching}
  {@const { from, live, elsewhere, transfers, decide } = switching}
  <ConfirmSwitch
    {from}
    {live}
    {elsewhere}
    {transfers}
    onkeep={() => decide("keep")}
    onclosesessions={() => decide("close")}
    oncancel={() => decide("cancel")}
  />
{/if}

{#if vaultLocked}
  <UnlockVault
    {name}
    onunlocked={() => {
      vaultLocked = false;
      reopenWorkspace();
    }}
    onstartedover={() => {
      vaultLocked = false;
      reopenWorkspace();
    }}
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
  .window {
    height: 100%;
    display: grid;
    grid-template-rows: minmax(0, 1fr) auto;
  }

  .layout {
    position: relative;
    min-height: 0;
    display: grid;
  }

  /* Holds a panel's resize handle, on the panel's inner edge. */
  .edge {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 0;
  }

  .side-view {
    min-width: 0;
    min-height: 0;
    display: flex;
    flex-direction: column;
    background: var(--surface);
    border-right: 1px solid var(--border);
  }

  .side-view > :global(*) {
    flex: 1 1 auto;
    min-height: 0;
  }

  .hosts-area {
    display: flex;
    flex-direction: column;
  }

  .hosts-area > :global(*) {
    flex: 1 1 auto;
    min-height: 0;
  }

  /* With the details sheet open, the tree keeps the top of the side view
     (so that selecting in it changes what the sheet shows). */
  .side-view > .hosts-area.under-sheet {
    flex: 0 0 38%;
    min-height: 10rem;
  }

  /* The problems line would sit wedged between the short tree and the
     sheet; the rail's Hosts icon marks the problems meanwhile. */
  .hosts-area.under-sheet :global(footer.problems) {
    display: none;
  }

  .centre {
    position: relative;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
    display: flex;
  }

  /* Placed over its pane's body (see PaneRects). */
  .slot {
    position: absolute;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  .job-notice {
    position: fixed;
    left: 50%;
    bottom: calc(var(--space-4) + 1.75rem);
    transform: translateX(-50%);
    z-index: 30;
    display: flex;
    align-items: center;
    gap: var(--space-2);
    max-width: min(36rem, calc(100vw - 2 * var(--space-4)));
    padding: var(--space-1-5) var(--space-2) var(--space-1-5) var(--space-3);
    background: var(--dialog);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-md);
    box-shadow: var(--shadow);
    font-size: var(--text-sm);
  }

  .job-notice .dismiss {
    border-color: transparent;
    background: none;
    padding: var(--space-0-5);
    color: var(--muted);
  }

  .slot.view {
    background: var(--bg);
  }

  /* A question about one pane's tab, over that pane's body only (#74):
     the other panes stay usable. */
  .pane-question {
    position: absolute;
    z-index: 5;
    min-width: 0;
    min-height: 0;
  }

  .slot.connect {
    background: var(--bg);
  }
</style>
