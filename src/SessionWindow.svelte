<script lang="ts">
  // A session window (#84): one session moved out of the main window, on
  // any monitor. The app's own frame (#80) with the session's tab, the
  // session's header and terminal, and the status line; no rail, side
  // views or welcome view, and no multi-send until #87. What it shows of
  // the session comes from the app's record of it, not from the window
  // that gave it up. Dock back (its menu, Ctrl+Shift+N, or closing the
  // window) returns it to a tab of the main window.
  import { listen } from "@tauri-apps/api/event";
  import { getCurrentWindow } from "@tauri-apps/api/window";
  import type { Attachment } from "svelte/attachments";
  import ConfirmClose from "./lib/ConfirmClose.svelte";
  import PaneView, { type PaneHandlers } from "./lib/PaneView.svelte";
  import SessionMenu, { type SessionAction } from "./lib/SessionMenu.svelte";
  import StatusLine from "./lib/StatusLine.svelte";
  import TerminalSession from "./lib/TerminalSession.svelte";
  import { panesOf, type Layout } from "./lib/layout";
  import { NO_SAFETY } from "./lib/paste";
  import {
    fontStack,
    isDark,
    loadPreferences,
    preferences,
    zoomFont,
    type Preferences,
  } from "./lib/preferences.svelte";
  import { appName, type SessionState } from "./lib/session";
  import {
    closeSessionWindow,
    dockSession,
    takeSession,
    type TakenSession,
  } from "./lib/windows";

  /** What the window shows, once the shell has handed it over. */
  let taken = $state<TakenSession | null>(null);
  let layout = $state<Layout | null>(null);
  let error = $state<string | null>(null);
  let terminal = $state<TerminalSession | null>(null);
  let status = $state<SessionState["state"]>("connecting");
  let size = $state<{ cols: number; rows: number } | null>(null);
  /** Whether closing the live session waits for the user's yes. */
  let closing = $state(false);
  /** The tab's context menu, when open. */
  let tabMenu = $state<{
    x: number;
    y: number;
    entries: SessionAction[];
  } | null>(null);
  let focusRequest = $state(0);
  /** Set while the session goes back: nothing else happens meanwhile. */
  let docking = false;

  void loadPreferences();
  // Changed in another window: shown here too.
  void listen<Preferences>("preferences-changed", (event) => {
    preferences.current = event.payload;
  });

  $effect(() => {
    document.documentElement.dataset.theme = isDark() ? "dark" : "light";
    document.documentElement.style.setProperty(
      "--font-mono",
      fontStack(preferences.current.fontFamily),
    );
  });

  takeSession().then(
    (value) => {
      taken = value;
      layout = value.layout;
    },
    (reason) => (error = String(reason)),
  );

  const record = $derived(taken?.record ?? null);
  const name = $derived(record?.name ?? "");
  let product = $state("");
  appName().then((value) => (product = value));
  // The window's title names its session (its bar shows it as the tab).
  $effect(() => {
    if (name) document.title = product ? `${name} - ${product}` : name;
  });

  const pane = $derived(layout ? panesOf(layout.root)[0] : null);

  /** Where the pane's body is: the terminal is placed over it, as in the
   * main window. */
  let rect = $state<{
    left: number;
    top: number;
    width: number;
    height: number;
  } | null>(null);
  let area = $state<HTMLElement>();
  function paneBody(): Attachment<HTMLElement> {
    return (body) => {
      const measure = () => {
        if (!area) return;
        const origin = area.getBoundingClientRect();
        const box = body.getBoundingClientRect();
        rect = {
          left: box.left - origin.left,
          top: box.top - origin.top,
          width: box.width,
          height: box.height,
        };
      };
      const observer = new ResizeObserver(measure);
      observer.observe(body);
      measure();
      return () => observer.disconnect();
    };
  }

  const tabState = $derived(status === "closed" ? "disconnected" : status);

  /**
   * Returns the session to a tab of the main window, with its scrollback;
   * this window then closes. Whether it went (it cannot while a question
   * or a paste would be lost).
   */
  async function dock(): Promise<boolean> {
    if (!taken || !terminal || docking) return false;
    docking = true;
    try {
      const given = await terminal.handOver();
      if (!given) return false;
      try {
        await dockSession(taken.tab, given.id, given.snapshot);
        return true;
      } catch (reason) {
        await terminal.takeBack().catch(() => {});
        error = `The session could not dock back: ${reason}`;
        return false;
      }
    } finally {
      docking = false;
    }
  }

  /** Closes the session's tab, and so this window (asking first while the
   * session is live). */
  function closeTab() {
    if (taken && tabState !== "disconnected") {
      closing = true;
      return;
    }
    void closeSessionWindow();
  }

  // Closing the window (its Close button, Alt+F4, the taskbar) docks the
  // session back first; if it cannot move now, closing it is asked.
  void getCurrentWindow().onCloseRequested(async (event) => {
    event.preventDefault();
    if (!taken || !terminal) {
      await closeSessionWindow();
      return;
    }
    if (!(await dock())) closeTab();
  });

  const handlers: PaneHandlers = {
    tab: () =>
      record
        ? {
            view: null,
            name: record.name,
            title: record.title,
            role: record.role,
            environment: record.safety.environment,
            state: tabState,
            member: false,
          }
        : null,
    activate: () => (focusRequest += 1),
    activatePane: () => {},
    close: () => closeTab(),
    move: () => {},
    newTab: () => {},
    split: () => {},
    closePane: () => {},
    joinPane: () => {},
    menu: (_tab, x, y) => {
      tabMenu = {
        x,
        y,
        entries: [
          ...(terminal?.actions() ?? []),
          {
            id: "close-tab",
            label: "Close tab",
            shortcut: "Ctrl+Shift+W",
            run: () => closeTab(),
          },
        ],
      };
    },
    body: () => paneBody(),
  };

  // The window's shortcuts work with the terminal focused: caught first.
  function onkeydowncapture(event: KeyboardEvent) {
    if (!event.ctrlKey || event.altKey || event.metaKey) return;
    const letter = (key: string) =>
      event.code === `Key${key}` ||
      event.key.toLowerCase() === key.toLowerCase();
    let action: (() => void) | null = null;
    if (event.code === "Equal" || event.key === "+" || event.key === "=")
      action = () => void zoomFont("in");
    else if (!event.shiftKey && (event.code === "Minus" || event.key === "-"))
      action = () => void zoomFont("out");
    else if (!event.shiftKey && event.code === "Digit0")
      action = () => void zoomFont("reset");
    else if (closing) action = null;
    else if (event.shiftKey && letter("N")) action = () => void dock();
    else if (event.shiftKey && letter("W")) action = () => closeTab();
    if (!action) return;
    event.preventDefault();
    event.stopPropagation();
    action();
  }
</script>

<svelte:window {onkeydowncapture} />

<div class="window" data-session-window>
  <div class="area" bind:this={area}>
    {#if layout && pane}
      <PaneView
        node={pane}
        activePane={pane.id}
        several={false}
        {handlers}
        tools={false}
      />
    {:else}
      <div class="frame" data-window-bar data-tauri-drag-region="deep"></div>
    {/if}
    {#if taken && record && rect}
      {@const tab = taken.tab}
      <div
        class="slot"
        data-tab={tab}
        data-active
        style:left="{rect.left}px"
        style:top="{rect.top}px"
        style:width="{rect.width}px"
        style:height="{rect.height}px"
      >
        <TerminalSession
          bind:this={terminal}
          source={{
            kind: "adopt",
            id: taken.session,
            snapshot: taken.snapshot,
          }}
          {tab}
          groupSize={null}
          ongrouppaste={() => {}}
          title={record.title}
          address={record.address}
          jumpHost={record.jumpHost}
          role={record.role}
          safety={record.safety ?? NO_SAFETY}
          name={record.name}
          shown={true}
          focused={true}
          {focusRequest}
          onsize={(next) => (size = next)}
          onstate={(next) => (status = next.state)}
          ondismiss={() => closeTab()}
          move={{ id: "dock", label: "Dock back", run: () => void dock() }}
        />
        {#if closing && tabState !== "disconnected"}
          <div class="question" data-pane-dialog={pane?.id}>
            <ConfirmClose
              {name}
              state={tabState}
              environment={record.safety.environment}
              onclose={() => {
                closing = false;
                void closeSessionWindow();
              }}
              oncancel={() => {
                closing = false;
                focusRequest += 1;
              }}
            />
          </div>
        {/if}
      </div>
    {/if}
    {#if error}
      <p class="note" role="alert" data-session-window-error>{error}</p>
    {/if}
  </div>
  <StatusLine vault={null} session={size && name ? { name, ...size } : null} />
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
    display: flex;
    flex-direction: column;
  }

  .area > :global(.pane) {
    flex: 1 1 0;
  }

  .frame {
    min-height: 38px;
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }

  .slot {
    position: absolute;
    min-width: 0;
    min-height: 0;
    overflow: hidden;
  }

  /* The close question covers the session only, as in the main window. */
  .question {
    position: absolute;
    inset: 0;
    z-index: 5;
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
