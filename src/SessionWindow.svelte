<script lang="ts">
  // A session window (#84, #85): sessions moved out of the main window, on
  // any monitor. The app's own frame (#80) with tabs and split panes, as
  // in the main window, each session with its header and terminal, and
  // the status line; no rail, side views, connect view or view tabs. Its
  // sessions take part in the app's multi-send group (#87): they join and
  // leave here, show its marker and size, and a paste in a member is
  // planned and asked here for every member, wherever it is. What it
  // shows of each session comes from the app's record of it, not from
  // the window that gave it up. Dock back
  // (its menu, Ctrl+Shift+N) returns a session to a tab of the main
  // window; Dock all back returns them all. Closing the window with live
  // sessions asks: dock them back, close them, or cancel (#88). The
  // window closes itself once its last tab has gone. Its palette (Ctrl+K)
  // connects a host into this window and sends a snippet to its active
  // terminal (#88).
  import { tick, untrack } from "svelte";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import { listen } from "@tauri-apps/api/event";
  import CommandPalette from "./lib/CommandPalette.svelte";
  import ConfirmClose from "./lib/ConfirmClose.svelte";
  import ConfirmCloseWindow from "./lib/ConfirmCloseWindow.svelte";
  import ConfirmJoin from "./lib/ConfirmJoin.svelte";
  import PasteConfirm, { type PasteMember } from "./lib/PasteConfirm.svelte";
  import PaneView, {
    type PaneHandlers,
    type TabInfo,
  } from "./lib/PaneView.svelte";
  import SessionMenu, {
    type SessionAction,
    type SessionMove,
  } from "./lib/SessionMenu.svelte";
  import SnippetVariables from "./lib/SnippetVariables.svelte";
  import StatusLine from "./lib/StatusLine.svelte";
  import TerminalSession from "./lib/TerminalSession.svelte";
  import {
    editLayout,
    panesOf,
    type Layout,
    type LayoutEdit,
    type TabId,
  } from "./lib/layout";
  import { askForKeys, givingKeys, keyMark, keysAsked } from "./lib/keys";
  import { PaneRects, sessionWindowSize } from "./lib/paneRects.svelte";
  import { joinConfirmations, NO_SAFETY } from "./lib/paste";
  import {
    expandForMembers,
    GroupPaste,
    onLayoutChanged,
    sessionSafety,
  } from "./lib/groupPaste";
  import {
    currentWorkspace,
    onWorkspaceChanged,
    type Environment,
    type Host,
  } from "./lib/workspace";
  import { addressBesides, hostConnection } from "./lib/hostConnect";
  import { hostLogin } from "./lib/credentials";
  import {
    hostRole,
    hostSafety,
    inventory,
    setWorkspace,
  } from "./lib/inventory.svelte";
  import {
    fontStack,
    isDark,
    loadPreferences,
    preferences,
    zoomFont,
    type Preferences,
  } from "./lib/preferences.svelte";
  import {
    appName,
    sessionRecord,
    type SessionRecord,
    type SessionSource,
    type SessionState,
  } from "./lib/session";
  import {
    trackSessionWindows,
    windowMoves,
  } from "./lib/sessionWindows.svelte";
  import { expandSnippetForSession } from "./lib/workspace";
  import {
    askMainWindow,
    closeSessionWindow,
    connectFromMain,
    dockAll,
    dockSession,
    onAnswerCheck,
    onSnippetSend,
    onTabArrived,
    sayAnswering,
    terminalUsed,
    openSessionWindow,
    takeSessions,
    takeTab,
    type Docking,
    type TakenSession,
  } from "./lib/windows";

  /** A tab's session: how its terminal takes it, and the app's record. */
  interface WindowSession {
    source: SessionSource;
    record: SessionRecord;
  }

  /** The tabs and panes, as the core keeps them; null until taken. */
  let layout = $state<Layout | null>(null);
  /** Each tab's session, by tab id. */
  let sessions = $state<Record<TabId, WindowSession>>({});
  /** Each tab's session state, as its terminal reports it. */
  let states = $state<Record<TabId, SessionState["state"]>>({});
  /** Each tab's terminal size, for the status line. */
  let sizes = $state<Record<TabId, { cols: number; rows: number }>>({});
  /** Each tab's terminal. */
  const terminals: Record<TabId, TerminalSession | null> = {};
  let error = $state<string | null>(null);
  /** The tab whose closing waits for the user's yes. */
  let closing = $state<TabId | null>(null);
  /** The tab's context menu, when open. */
  let tabMenu = $state<{
    x: number;
    y: number;
    entries: SessionAction[];
  } | null>(null);
  let focusRequest = $state(0);
  /** Asks that the active terminal take the keys (#141), as App's. */
  function requestKeys(mark?: number) {
    askForKeys(mark);
    focusRequest += 1;
  }
  /** Set while sessions leave: no other move meanwhile. */
  let moving = false;
  /** A snippet waiting for the user to give its variables a value (#86). */
  /** `group`: sent to the multi-send group, decided when first sent. */
  let asking = $state<{
    tab: TabId;
    name: string;
    names: string[];
    group: boolean;
  } | null>(null);
  /** A production tab waiting for the user's yes before joining (#87). */
  let joining = $state<{
    tab: TabId;
    production: string[];
    answer: (join: boolean) => void;
  } | null>(null);
  /** Closing the window, waiting for the user to dock back, close or
   * cancel (#88): the names of its live sessions. */
  let closingWindow = $state<{ live: string[]; production: boolean } | null>(
    null,
  );
  /** Whether the palette is open (#88). */
  let palette = $state(false);
  /** Tabs opened here whose record was fetched from the app (#88). */
  const recordsFetched = new Set<TabId>();
  /** A paste to the multi-send group waiting for the user's yes (#87). */
  let groupPasteQuestion = $state<{
    /** The tab pasted in, over whose terminal the question is shown. */
    tab: TabId;
    text: string;
    lineCount: number;
    environment: Environment | null;
    members: PasteMember[];
    answer: (send: boolean) => void;
  } | null>(null);

  void loadPreferences();
  // Changed in another window (zoom, theme): shown here too.
  void listen<Preferences>("preferences-changed", (event) => {
    preferences.current = event.payload;
  });
  trackSessionWindows();

  $effect(() => {
    document.documentElement.dataset.theme = isDark() ? "dark" : "light";
    document.documentElement.style.setProperty(
      "--font-mono",
      fontStack(preferences.current.fontFamily),
    );
  });

  // Ctrl+scroll sizes the terminal font (TerminalSession), never the page.
  window.addEventListener(
    "wheel",
    (event) => {
      if (event.ctrlKey) event.preventDefault();
    },
    { passive: false },
  );

  /** Shows `next` unless a newer layout is already shown. */
  function adopt(next: Layout) {
    if (layout === null || next.revision > layout.revision) layout = next;
    forgetClosedTabs();
  }

  /** Gives the tab a terminal for a session taken over from elsewhere. */
  function receive(taken: TakenSession) {
    if (sessions[taken.tab]) return;
    const record = taken.record ?? {
      name: `Session ${taken.session}`,
      title: `Session ${taken.session}`,
      address: null,
      jumpHost: null,
      host: null,
      workspace: null,
      safety: NO_SAFETY,
      role: null,
      insecure: null,
    };
    sessions[taken.tab] = {
      source: { kind: "adopt", id: taken.session, snapshot: taken.snapshot },
      record,
    };
  }

  // A tab moved here while this page is open: taken once (one already
  // taken as the page loaded is skipped).
  void onTabArrived(async (tab) => {
    if (sessions[tab]) return;
    const mark = keyMark();
    try {
      const taken = await takeTab(tab);
      receive(taken.session);
      adopt(taken.layout);
      requestKeys(mark);
    } catch (reason) {
      error = `A session could not move here: ${reason}`;
    }
  });

  // The group changed in another window: its markers and size here too.
  void onLayoutChanged((next) => adopt(next));

  takeSessions().then(
    (taken) => {
      for (const each of taken.sessions) receive(each);
      adopt(taken.layout);
    },
    (reason) => (error = String(reason)),
  );

  /** Layout edits sent and not answered yet (#141, as in App). */
  let pendingEdits = 0;

  /** Edits the layout; `prepare` is given the created tab first. Edits
   * naming a tab or pane already gone do nothing. `ask`: the user's edit,
   * whose active terminal takes the keys unless they move meanwhile
   * (#141); false for one that follows where the keys are. */
  async function edit(
    change: LayoutEdit,
    prepare?: (created: number) => void,
    ask = true,
  ): Promise<void> {
    if (ask) askForKeys();
    let changed;
    pendingEdits += 1;
    try {
      changed = await editLayout(change);
    } catch {
      return;
    } finally {
      pendingEdits -= 1;
    }
    if (changed.created !== null) prepare?.(changed.created);
    adopt(changed.layout);
  }

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
  /** Every tab, in reading order. */
  const tabOrder = $derived(panes.flatMap((pane) => pane.tabs));

  /** The active pane follows the keyboard (#141), as App's `followKeys`. */
  function followKeys() {
    if (pendingEdits > 0 || keysAsked() || !layout) return;
    const holder =
      document.activeElement?.closest<HTMLElement>(".slot[data-tab]");
    const tab = Number(holder?.dataset.tab);
    const place = Number.isInteger(tab) ? placement.get(tab) : undefined;
    if (place && !(place.shown && place.pane === layout.activePane))
      void edit({ kind: "activate", tab }, undefined, false);
  }
  $effect(() => {
    void layout;
    untrack(followKeys);
  });
  const activeTab = $derived(activePane?.active ?? null);

  /** Drops the sessions of tabs no longer in the layout. */
  function forgetClosedTabs() {
    for (const key of Object.keys(sessions)) {
      const tab = Number(key);
      if (!placement.has(tab)) {
        delete sessions[tab];
        delete states[tab];
        delete sizes[tab];
      }
    }
  }

  // Terminals are placed over their pane's body, as in the main window.
  const panesArea = new PaneRects();
  const rects = $derived(panesArea.rects);
  $effect(() => {
    if (layout) void tick().then(() => panesArea.measure());
  });

  // The window's title names the session its active tab shows, and
  // follows it; the taskbar shows it.
  let product = $state("");
  appName().then((value) => (product = value));
  const activeName = $derived(
    activeTab === null ? "" : (sessions[activeTab]?.record.name ?? ""),
  );
  $effect(() => {
    if (!activeName) return;
    const title = product ? `${activeName} - ${product}` : activeName;
    document.title = title;
    void getCurrentWindow()
      .setTitle(title)
      .catch(() => {});
  });

  const statusSession = $derived.by(() => {
    const size = activeTab === null ? undefined : sizes[activeTab];
    return size && activeName ? { name: activeName, ...size } : null;
  });

  function tabState(tab: TabId): TabInfo["state"] {
    const state = states[tab] ?? "connecting";
    return state === "closed" ? "disconnected" : state;
  }

  /** Closes `tab`, first asking if its session is still live. */
  function closeTab(tab: TabId) {
    if (!sessions[tab]) return;
    if (tabState(tab) !== "disconnected") {
      closing = tab;
      return;
    }
    closeNow(tab);
  }

  /** Closes `tab` and its session; the last one closes the window. */
  function closeNow(tab: TabId) {
    if (tabOrder.length <= 1) void closeSessionWindow();
    else void edit({ kind: "closeTab", tab });
  }

  function confirmClose() {
    const tab = closing;
    closing = null;
    if (tab !== null) closeNow(tab);
  }

  // A session that ends while its closing waits needs no confirmation.
  $effect(() => {
    if (closing !== null && tabState(closing) === "disconnected")
      confirmClose();
  });

  /** Moves `tab`'s session to another window: into the open session
   * window `into`, or a new one when null. */
  async function moveOut(tab: TabId, into: number | null) {
    const terminal = terminals[tab];
    const place = placement.get(tab);
    const rect = place ? rects[place.pane] : undefined;
    if (!terminal || !rect || moving) return;
    moving = true;
    try {
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
      } catch (reason) {
        await terminal.takeBack().catch(() => {});
        error = `The session could not move: ${reason}`;
      }
    } finally {
      moving = false;
    }
  }

  /** Returns `tab`'s session to a tab of the main window, with its
   * scrollback. */
  async function dock(tab: TabId) {
    const terminal = terminals[tab];
    if (!terminal || moving) return;
    moving = true;
    try {
      const given = await terminal.handOver();
      if (!given) return;
      try {
        const shown = await dockSession(tab, given.id, given.snapshot);
        if (shown) adopt(shown);
      } catch (reason) {
        await terminal.takeBack().catch(() => {});
        error = `The session could not dock back: ${reason}`;
      }
    } finally {
      moving = false;
    }
  }

  /** Why Dock all back cannot happen now (a session that must stay), or
   * null. */
  function dockAllRefusal(): string | null {
    for (const tab of tabOrder) {
      const terminal = terminals[tab];
      // (moveRefusal's null means it may move: not `??`.)
      const refusal = terminal ? terminal.moveRefusal() : "Not yet";
      if (refusal !== null)
        return `${sessions[tab]?.record.name ?? "A session"}: ${refusal}`;
    }
    return null;
  }

  /**
   * Dock all back (#85): every session goes back to the main window, in
   * order, and the window closes. With `partly`, those that cannot move
   * now stay (closing the window then asks about them); otherwise none
   * moves unless all can. Returns the tabs left here.
   */
  async function dockEvery(partly: boolean): Promise<TabId[]> {
    if (moving) return [...tabOrder];
    if (!partly && dockAllRefusal() !== null) return [...tabOrder];
    moving = true;
    const given: { tab: TabId; terminal: TerminalSession; docking: Docking }[] =
      [];
    /** Shows here again every session given up so far. */
    const takeBackAll = async () => {
      for (const each of given) await each.terminal.takeBack().catch(() => {});
    };
    try {
      for (const tab of tabOrder) {
        const terminal = terminals[tab];
        if (!terminal || terminal.moveRefusal() !== null) continue;
        const handed = await terminal.handOver();
        if (handed)
          given.push({
            tab,
            terminal,
            docking: { tab, id: handed.id, snapshot: handed.snapshot },
          });
      }
      const left = tabOrder.filter(
        (tab) => !given.some((each) => each.tab === tab),
      );
      if (given.length === 0 || (!partly && left.length > 0)) {
        await takeBackAll();
        return [...tabOrder];
      }
      try {
        const shown = await dockAll(given.map((each) => each.docking));
        if (shown) adopt(shown);
        return left;
      } catch (reason) {
        await takeBackAll();
        error = `The sessions could not dock back: ${reason}`;
        return [...tabOrder];
      }
    } finally {
      moving = false;
    }
  }

  /** Whether `tab` is in the multi-send group. */
  function inGroup(tab: TabId): boolean {
    return layout?.group.includes(tab) ?? false;
  }

  /** The group, as the snippet question names it. */
  function groupName(): string {
    return `the multi-send group (${layout?.groupSize ?? 0})`;
  }

  /** Adds `tab` to the multi-send group, or takes it out; a production
   * session joins once the user agrees, asked over its pane. */
  async function toggleGroup(tab: TabId) {
    const id = terminals[tab]?.sessionId() ?? null;
    if (!sessions[tab] || joining) return;
    if (inGroup(tab)) {
      await edit({ kind: "leaveGroup", tab }, undefined, false);
      return;
    }
    let production: TabId[] = [];
    try {
      // What applies to its pastes, as any window pasting to it sees it.
      const safety =
        id === null ? sessions[tab].record.safety : await sessionSafety(id);
      production = await joinConfirmations([{ tab, safety }]);
    } catch {
      return;
    }
    if (production.length > 0) {
      const agreed = await new Promise<boolean>((answer) => {
        joining = {
          tab,
          production: [sessions[tab]?.record.name ?? ""],
          answer,
        };
      });
      joining = null;
      requestKeys();
      if (!agreed || !sessions[tab]) return;
    }
    // The shell refuses a production join that was not confirmed (#123).
    let changed;
    try {
      changed = await editLayout({
        kind: "joinGroup",
        tab,
        confirmed: production.length > 0,
      });
    } catch (reason) {
      error = `Cannot add to the multi-send group: ${reason}`;
      return;
    }
    adopt(changed.layout);
  }

  /** This window's group pastes (#87): planned for every member wherever
   * it is, asked once over the tab pasted in, then each member's lines
   * through its own terminal. */
  const groupPaste = new GroupPaste({
    terminals,
    ready: (tab) => !!sessions[tab] && states[tab] === "connected",
    ask: async (tab, question) => {
      const send = await new Promise<boolean>((answer) => {
        groupPasteQuestion = { tab, ...question, answer };
      });
      groupPasteQuestion = null;
      requestKeys();
      return send;
    },
    withdraw: () => groupPasteQuestion?.answer(false),
    failed: (reason) => {
      error = `Cannot paste to the multi-send group: ${reason}`;
    },
  });
  // A group paste's question open since before the change closes (#123).
  $effect(() => {
    const revision = layout?.groupRevision;
    if (revision !== undefined)
      untrack(() => groupPaste.groupChanged(revision));
  });

  // The group's paste question is shown over the tab pasted in, and the
  // join question over the joining one: if that tab goes or is hidden,
  // nothing is pasted, and it does not join.
  $effect(() => {
    const question = groupPasteQuestion;
    if (question && !placement.get(question.tab)?.shown) question.answer(false);
  });
  $effect(() => {
    const question = joining;
    if (question && !placement.get(question.tab)?.shown) question.answer(false);
  });

  /** What this window holds about `tab`'s session that a move would lose. */
  function windowQuestionOf(tab: TabId): string | null {
    if (asking?.tab === tab)
      return "Not while the snippet's variables are asked";
    if (groupPasteQuestion?.tab === tab || joining?.tab === tab)
      return "Not while a question about the session is open";
    return null;
  }

  // The variables question is shown over its tab: hidden or closed, the
  // snippet is not sent.
  $effect(() => {
    if (asking && !placement.get(asking.tab)?.shown) asking = null;
  });

  /**
   * Sends the snippet `name`, from the main window's Snippets view, to
   * `tab`'s terminal (the one used last, #86): the shell fills it from the
   * session's host, the variables it lacks are asked over that pane, and
   * it goes through the terminal's own paste path (its confirmation and
   * line delay).
   */
  async function sendSnippet(
    tab: TabId,
    name: string,
    answers: Record<string, string> = {},
    // To the group or not, decided when it is first sent (#123).
    group: boolean = inGroup(tab),
  ) {
    const terminal = terminals[tab];
    const id = terminal?.sessionId() ?? null;
    if (!terminal || id === null || states[tab] !== "connected") {
      error = `Cannot send ${name}: the session is no longer connected.`;
      return;
    }
    // The keys go to its terminal at the end, unless moved meanwhile.
    const mark = keyMark();
    if (activeTab !== tab) await edit({ kind: "activate", tab });
    if (group) {
      await sendSnippetToGroup(name, answers, tab, mark);
      return;
    }
    let expansion;
    try {
      expansion = await expandSnippetForSession(id, name, answers);
    } catch (reason) {
      error = `Cannot send ${name}: ${reason}`;
      return;
    }
    if (expansion.kind === "missing") {
      asking = { tab, name, names: expansion.names, group: false };
      return;
    }
    if (expansion.kind === "steps") {
      // Run on this session alone, the device waited for (#23).
      const refused = await terminal.runSteps(name, answers);
      if (refused) error = `Cannot run ${name}: ${refused}.`;
      return;
    }
    const text = expansion.lines.map((line) => `${line}\n`).join("");
    if (!terminal.sendText(text)) {
      error = `Cannot send ${name}: the session is no longer connected.`;
      return;
    }
    requestKeys(mark);
  }

  /**
   * Sends the snippet `name` to every connected member of the multi-send
   * group, as sent from `tab` (#87): filled per member from its own host,
   * wherever it is; variables some member lacks are asked once, over this
   * pane. Then it takes the group's paste path.
   */
  async function sendSnippetToGroup(
    name: string,
    answers: Record<string, string>,
    tab: TabId,
    mark: number = keyMark(),
  ) {
    let receiving;
    let expansion;
    try {
      // Refused (called off) if the tab is no longer a member.
      receiving = await groupPaste.receiving(tab);
      expansion = await expandForMembers(name, receiving.members, answers);
    } catch (reason) {
      error = `Cannot send ${name}: ${reason}`;
      return;
    }
    if (expansion.kind === "missing") {
      asking = { tab, name, names: expansion.names, group: true };
      return;
    }
    groupPaste.pasteEach(tab, receiving, expansion.texts);
    requestKeys(mark);
  }

  void onSnippetSend(({ tab, name }) => void sendSnippet(tab, name));
  // The shell asks every 2 s whether this page runs (#120): one that hangs
  // leaves it unanswered, and the main window offers to take its sessions
  // until it answers again.
  void onAnswerCheck(() => void sayAnswering().catch(() => {}));

  /** Asks the main window for something only it has, for `tab`'s session
   * (#86); it comes forward. */
  async function ask(
    tab: TabId,
    request: Parameters<typeof askMainWindow>[1],
  ): Promise<boolean> {
    try {
      await askMainWindow(tab, request);
      return true;
    } catch (reason) {
      error = String(reason);
      return false;
    }
  }

  // Closing the window (its Close button, Alt+F4, the taskbar): with a
  // session opening or connected, the user docks them back, closes them or
  // cancels (#88); otherwise it closes, with its closed sessions.
  void getCurrentWindow().onCloseRequested(async (event) => {
    event.preventDefault();
    if (closingWindow || moving) return;
    const live = tabOrder.filter((tab) => tabState(tab) !== "disconnected");
    if (live.length === 0) {
      await closeSessionWindow();
      return;
    }
    tabMenu = null;
    palette = false;
    closingWindow = {
      live: live.map((tab) => sessions[tab]?.record.name ?? "A session"),
      production: live.some(
        (tab) => sessions[tab]?.record.safety.environment === "production",
      ),
    };
  });

  /** Closing the window, its sessions docked back: this window's own
   * questions are dropped first (they would hold a session here); a
   * session that still cannot move stays, and the window says why. */
  async function closeByDocking() {
    closingWindow = null;
    asking = null;
    groupPasteQuestion?.answer(false);
    joining?.answer(false);
    closing = null;
    const mark = keyMark();
    await tick();
    const reasons = tabOrder
      .map((tab) => {
        const terminal = terminals[tab];
        // (moveRefusal's null means it may move: not `??`.)
        const refusal = terminal ? terminal.moveRefusal() : "Not yet";
        return refusal === null
          ? null
          : `${sessions[tab]?.record.name ?? "A session"}: ${refusal}`;
      })
      .filter((reason) => reason !== null);
    const left = await dockEvery(true);
    if (left.length > 0) {
      const count = left.length === 1 ? "A session" : `${left.length} sessions`;
      error = `${count} could not dock back, so the window stays open. ${reasons.join("; ")}`;
      void edit({ kind: "activate", tab: left[0] }, undefined, false);
      requestKeys(mark);
    }
  }

  /** Closing the window and every session in it. */
  function closeWithSessions() {
    closingWindow = null;
    void closeSessionWindow().catch((reason) => (error = String(reason)));
  }

  /** The palette's snippets go to the active terminal, while connected. */
  const snippetTarget = $derived(
    activeTab !== null && states[activeTab] === "connected"
      ? (sessions[activeTab]?.record.name ?? null)
      : null,
  );

  // The workspace the palette finds hosts in follows changes on disk, as
  // the main window's does (a host just written is found and connected).
  void onWorkspaceChanged((changed) => {
    if (inventory.workspace?.root === changed.root)
      inventory.workspace = changed;
  });

  /** Opens the palette with the workspace open now (read afresh: this
   * window is not told when the main window switches workspace). */
  async function openPalette() {
    try {
      const open = await currentWorkspace();
      if (open === null || open.root !== inventory.workspace?.root)
        setWorkspace(open);
      else inventory.workspace = open;
    } catch (reason) {
      error = `Cannot read the workspace: ${reason}`;
      return;
    }
    palette = true;
  }

  /**
   * Connects `host` from the palette into a new tab of this window's
   * active pane (#88), as the main window does: over telnet or serial at
   * once, else with its credential profile. A host whose credentials must
   * be asked is connected by the main window, which has the prompt; it
   * comes forward.
   */
  async function connectHost(host: Host) {
    const mark = keyMark();
    let connection;
    try {
      connection = hostConnection(host, await hostLogin(host.id));
    } catch {
      // The profiles cannot be read: the main window says so, and asks.
      connection = null;
    }
    if (!connection) {
      try {
        await connectFromMain(host.id);
      } catch (reason) {
        error = `Cannot connect ${host.name}: ${reason}`;
      }
      return;
    }
    const { target, title } = connection;
    const record: SessionRecord = {
      name: host.name,
      title,
      address: addressBesides(host, title),
      jumpHost: null,
      host: host.id,
      workspace: inventory.workspace?.root ?? null,
      safety: hostSafety(host.id) ?? NO_SAFETY,
      role: hostRole(host.id),
      insecure: null,
    };
    askForKeys(mark);
    await edit(
      { kind: "openTab" },
      (tab) => {
        sessions[tab] = {
          source: { kind: "target", target, name: host.name, title },
          record,
        };
      },
      false,
    );
    requestKeys(mark);
  }

  /** A session opened here: once it is open, its header shows what the app
   * recorded of it (its jump host, say), fetched once. */
  function fetchRecordOnce(tab: TabId) {
    const id = terminals[tab]?.sessionId() ?? null;
    if (id === null || recordsFetched.has(tab)) return;
    if (sessions[tab]?.source.kind !== "target") return;
    recordsFetched.add(tab);
    void sessionRecord(id).then((record) => {
      if (record && sessions[tab]) sessions[tab].record = record;
    });
  }

  /** The moves offered for `tab`'s session. */
  function movesOf(tab: TabId): SessionMove[] {
    const several = tabOrder.length > 1;
    return [
      {
        id: "dock",
        label: "Dock back",
        title: "Return this session to a tab of the main window",
        shortcut: "Ctrl+Shift+N",
        run: () => void dock(tab),
      },
      ...(several
        ? [
            {
              id: "dock-all",
              label: "Dock all back",
              title:
                "Return every session of this window to the main window, and close it",
              refusal: dockAllRefusal,
              run: () => void dockEvery(false),
            },
          ]
        : []),
      ...windowMoves((into) => void moveOut(tab, into)),
    ];
  }

  /** Opens a second session like `original`'s (core session `id`) in a
   * new tab beside it, here. */
  function duplicate(original: TabId, id: number) {
    const session = sessions[original];
    if (!session) return;
    void edit({ kind: "openTabBeside", tab: original }, (tab) => {
      sessions[tab] = {
        source: { kind: "duplicate", of: id },
        record: session.record,
      };
    });
  }

  const handlers: PaneHandlers = {
    tab(tab) {
      const session = sessions[tab];
      if (!session) return null;
      return {
        view: null,
        name: session.record.name,
        title: session.record.title,
        role: session.record.role,
        environment: session.record.safety.environment,
        state: tabState(tab),
        member: inGroup(tab),
      };
    },
    activate: (tab) => {
      requestKeys();
      void edit({ kind: "activate", tab });
    },
    activatePane: (pane) => void edit({ kind: "activatePane", pane }),
    close: (tab) => closeTab(tab),
    move: (tab, pane, index) => {
      requestKeys();
      void edit({ kind: "moveTab", tab, pane, index });
    },
    newTab: () => {},
    refocus: () => requestKeys(),
    split: (pane, direction) => void edit({ kind: "split", pane, direction }),
    closePane: (pane) => void edit({ kind: "closePane", pane }),
    joinPane: () => {},
    menu: (tab, x, y) => {
      tabMenu = {
        x,
        y,
        entries: [
          ...(terminals[tab]?.actions() ?? []),
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

  /** The window's shortcuts, as the main window's for its tabs. */
  function shortcut(event: KeyboardEvent): (() => void) | null {
    if (!event.ctrlKey || event.altKey || event.metaKey) return null;
    const letter = (key: string) =>
      event.code === `Key${key}` ||
      event.key.toLowerCase() === key.toLowerCase();
    if (
      event.code === "Equal" ||
      event.code === "NumpadAdd" ||
      event.key === "+" ||
      event.key === "="
    )
      return () => void zoomFont("in");
    if (
      !event.shiftKey &&
      (event.code === "Minus" ||
        event.code === "NumpadSubtract" ||
        event.key === "-")
    )
      return () => void zoomFont("out");
    if (
      !event.shiftKey &&
      (event.code === "Digit0" || event.code === "Numpad0")
    )
      return () => void zoomFont("reset");
    if (closing !== null || closingWindow !== null || palette) return null;
    const tab = activeTab;
    if (!event.shiftKey && letter("K")) return () => void openPalette();
    if (
      event.key === "Tab" ||
      event.key === "PageDown" ||
      event.key === "PageUp"
    ) {
      const forward =
        event.key === "PageDown" || (event.key === "Tab" && !event.shiftKey);
      if (event.key !== "Tab" && event.shiftKey) {
        return tab === null
          ? null
          : () =>
              void edit({ kind: "moveTabBy", tab, offset: forward ? 1 : -1 });
      }
      return () => void edit({ kind: forward ? "nextTab" : "previousTab" });
    }
    if (event.shiftKey && letter("W"))
      return tab === null ? null : () => closeTab(tab);
    if (event.shiftKey && letter("M"))
      return tab === null ? null : () => void toggleGroup(tab);
    // A refused dock says nothing (the menu entry gives the reason).
    if (event.shiftKey && letter("N"))
      return tab === null ? null : () => void dock(tab);
    return null;
  }

  // The window's shortcuts work with the terminal focused: caught first.
  function onkeydowncapture(event: KeyboardEvent) {
    const action = shortcut(event);
    if (!action) return;
    event.preventDefault();
    event.stopPropagation();
    action();
  }
</script>

<svelte:window {onkeydowncapture} />

<div class="window" data-session-window>
  <div class="area" bind:this={panesArea.area}>
    {#if layout}
      <PaneView
        node={layout.root}
        activePane={layout.activePane}
        several={panes.length > 1}
        {handlers}
        connects={false}
      />
      {#each Object.entries(sessions) as [key, session] (key)}
        {@const tab = Number(key)}
        {@const place = placement.get(tab)}
        {@const rect = place ? rects[place.pane] : undefined}
        {@const shown = !!place?.shown && rect !== undefined}
        {@const focused = shown && place?.pane === layout.activePane}
        {@const record = session.record}
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
            // While an edit is on its way `focused` may be out of date
            // (#141): sent anyway, so the last focus decides the pane.
            if (place && (!focused || (pendingEdits > 0 && !givingKeys())))
              void edit({ kind: "activate", tab }, undefined, false);
            void terminalUsed(tab);
          }}
        >
          <TerminalSession
            bind:this={terminals[tab]}
            source={session.source}
            {tab}
            groupSize={inGroup(tab) ? layout.groupSize : null}
            ontogglegroup={() => void toggleGroup(tab)}
            onendgroup={() => void edit({ kind: "endGroup" }, undefined, false)}
            ongrouppaste={(text) => groupPaste.paste(tab, text)}
            title={record.title}
            address={record.address}
            jumpHost={record.jumpHost}
            role={record.role}
            safety={record.safety ?? NO_SAFETY}
            name={record.name}
            {shown}
            {focused}
            {focusRequest}
            onsize={(next) => (sizes[tab] = next)}
            onstate={(next) => {
              states[tab] = next.state;
              fetchRecordOnce(tab);
            }}
            ondismiss={() => closeTab(tab)}
            onduplicate={(id) => duplicate(tab, id)}
            ondetails={record.host === null
              ? undefined
              : () => void ask(tab, { kind: "details" })}
            onfiles={record.host === null
              ? undefined
              : () => void ask(tab, { kind: "files" })}
            onpasswordlogin={record.host === null
              ? undefined
              : async (reason) => {
                  // The main window asks for the password; this tab goes.
                  if (await ask(tab, { kind: "passwordLogin", reason }))
                    closeNow(tab);
                }}
            moves={movesOf(tab)}
            windowQuestion={windowQuestionOf(tab)}
          />
          {#if groupPasteQuestion?.tab === tab}
            {@const question = groupPasteQuestion}
            <PasteConfirm
              text={question.text}
              lineCount={question.lineCount}
              target={record.name}
              environment={question.environment}
              lineDelayMs={0}
              members={question.members}
              onsend={() => question.answer(true)}
              oncancel={() => question.answer(false)}
            />
          {/if}
          {#if asking?.tab === tab}
            {@const { name: snippet, names, group } = asking}
            <SnippetVariables
              {snippet}
              target={group ? groupName() : record.name}
              {names}
              onsend={(answers) => {
                const send = { snippet, tab, group };
                asking = null;
                void sendSnippet(send.tab, send.snippet, answers, send.group);
              }}
              oncancel={() => {
                asking = null;
                requestKeys();
              }}
            />
          {/if}
        </div>
      {/each}
      {#if joining}
        {@const question = joining}
        {@const pane = placement.get(question.tab)?.pane}
        {@const rect = pane === undefined ? undefined : rects[pane]}
        <div
          class="question"
          data-pane-dialog={pane}
          style:left={rect ? `${rect.left}px` : "0"}
          style:top={rect ? `${rect.top}px` : "0"}
          style:width={rect ? `${rect.width}px` : "100%"}
          style:height={rect ? `${rect.height}px` : "100%"}
        >
          <ConfirmJoin
            production={question.production}
            joining={1}
            onjoin={() => question.answer(true)}
            oncancel={() => question.answer(false)}
          />
        </div>
      {/if}
      {#if closing !== null && sessions[closing]}
        {@const tab = closing}
        {@const state = tabState(tab)}
        {@const pane = placement.get(tab)?.pane}
        {@const rect = pane === undefined ? undefined : rects[pane]}
        <div
          class="question"
          data-pane-dialog={pane}
          style:left={rect ? `${rect.left}px` : "0"}
          style:top={rect ? `${rect.top}px` : "0"}
          style:width={rect ? `${rect.width}px` : "100%"}
          style:height={rect ? `${rect.height}px` : "100%"}
        >
          <ConfirmClose
            name={sessions[tab].record.name}
            state={state === "disconnected" ? "connected" : state}
            environment={sessions[tab].record.safety.environment}
            onclose={confirmClose}
            oncancel={() => {
              closing = null;
              requestKeys();
            }}
          />
        </div>
      {/if}
    {:else}
      <div class="frame" data-window-bar data-tauri-drag-region="deep"></div>
    {/if}
    {#if error}
      <p class="note" role="alert" data-session-window-error>{error}</p>
    {/if}
  </div>
  <StatusLine vault={null} session={statusSession} />
</div>

{#if palette}
  <CommandPalette
    onconnect={(host) => void connectHost(host)}
    onsnippet={(name) => {
      if (activeTab !== null) void sendSnippet(activeTab, name);
    }}
    target={snippetTarget}
    actions={[]}
    reveals={false}
    onclose={() => {
      palette = false;
      requestKeys();
    }}
  />
{/if}

{#if closingWindow}
  {@const question = closingWindow}
  <ConfirmCloseWindow
    live={question.live}
    production={question.production}
    ondock={() => void closeByDocking()}
    onclosesessions={closeWithSessions}
    oncancel={() => {
      closingWindow = null;
      requestKeys();
    }}
  />
{/if}

{#if tabMenu}
  <SessionMenu
    entries={tabMenu.entries}
    x={tabMenu.x}
    y={tabMenu.y}
    onclose={() => {
      tabMenu = null;
      requestKeys();
    }}
  />
{/if}

<style>
  .window {
    position: fixed;
    inset: 0;
    display: grid;
    grid-template-rows: minmax(0, 1fr) auto;
    background: var(--bg);
  }

  .area {
    position: relative;
    min-height: 0;
    overflow: hidden;
    display: flex;
    flex-direction: column;
  }

  .area > :global(.pane),
  .area > :global(.split) {
    flex: 1 1 0;
  }

  .frame {
    min-height: 38px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }

  /* Placed over its pane's body (see PaneRects). */
  .slot {
    position: absolute;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  /* A question about one pane's tab, over that pane's body only (#74). */
  .question {
    position: absolute;
    z-index: 5;
    min-width: 0;
    min-height: 0;
  }

  .note {
    position: absolute;
    left: var(--space-4);
    right: var(--space-4);
    bottom: var(--space-4);
    margin: 0;
    padding: var(--space-2) var(--space-3);
    color: var(--text);
    background: var(--surface-raised);
    border: 1px solid var(--danger);
    border-radius: var(--radius-md);
  }
</style>
