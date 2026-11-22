<script lang="ts" module>
  import Columns2 from "@lucide/svelte/icons/columns-2";
  import Plus from "@lucide/svelte/icons/plus";
  import Rows2 from "@lucide/svelte/icons/rows-2";
  import SquareX from "@lucide/svelte/icons/square-x";
  import UsersRound from "@lucide/svelte/icons/users-round";
  import X from "@lucide/svelte/icons/x";
  import type { Attachment } from "svelte/attachments";
  import type {
    LayoutNode,
    PaneId,
    SplitDirection,
    TabId,
    ViewName,
  } from "./layout";
  import type { Environment, Role } from "./workspace";

  /** What a tab shows about its session, or about its view (#68). */
  export interface TabInfo {
    /** The view the tab shows; null for a session's tab. A view tab has
     * the view's icon and no connection state, and is never a member. */
    view: ViewName | null;
    /** The host name (or address, or serial port). */
    name: string;
    /** More detail, for the tooltip (`user@host:port`). */
    title: string;
    role: Role | null;
    /** The host's environment, which colours the tab; null: none. */
    environment: Environment | null;
    state: "connecting" | "connected" | "disconnected";
    /** Whether the tab is in the multi-send group. */
    member: boolean;
  }

  export interface PaneHandlers {
    tab: (tab: TabId) => TabInfo | null;
    activate: (tab: TabId) => void;
    activatePane: (pane: PaneId) => void;
    close: (tab: TabId) => void;
    /** `index` counts the target pane's other tabs. */
    move: (tab: TabId, pane: PaneId, index: number) => void;
    newTab: (pane: PaneId) => void;
    split: (pane: PaneId, direction: SplitDirection) => void;
    closePane: (pane: PaneId) => void;
    /** Adds every tab of the pane to the multi-send group. */
    joinPane: (pane: PaneId) => void;
    /** Gives the shown terminal the keys again (a menu of the bar closed). */
    refocus: () => void;
    /** Opens the tab's context menu (its session's actions) at x, y. */
    menu: (tab: TabId, x: number, y: number) => void;
    /** Attaches to each pane's body, where its terminal is placed. */
    body: (pane: PaneId) => Attachment<HTMLElement>;
  }
</script>

<script lang="ts">
  import PaneView from "./PaneView.svelte";
  import { panesOf } from "./layout";
  import RoleIcon from "./RoleIcon.svelte";
  import { VIEWS } from "./views";
  import { ENVIRONMENT_NAMES } from "./paste";
  import WindowControls from "./WindowControls.svelte";
  import SessionMenu, { type SessionAction } from "./SessionMenu.svelte";
  import Ellipsis from "@lucide/svelte/icons/ellipsis";

  interface Props {
    node: LayoutNode;
    activePane: PaneId;
    /** Whether there is more than one pane (each can then be closed). */
    several: boolean;
    handlers: PaneHandlers;
    /** Whether the pane (or split) reaches the window's top edge: its tab
     * bar is then part of the window's top bar, and drags the window
     * (#80). */
    top?: boolean;
    /** Whether it reaches the window's right edge: the top-right pane's
     * bar ends with the window's buttons. */
    right?: boolean;
    /**
     * Whether a pane offers a new connection (its bar's new tab, an empty
     * pane's Connect here) and multi-send (joining the pane); a session
     * window's (#85) offers neither, only split and close pane.
     */
    connects?: boolean;
  }

  let {
    node,
    activePane,
    several,
    handlers,
    top = true,
    right = true,
    connects = true,
  }: Props = $props();

  const STATE_TEXT = {
    connecting: "Connecting",
    connected: "Connected",
    disconnected: "Disconnected",
  };

  /** Pixels the pointer moves before a press on a tab becomes a drag. */
  const DRAG_THRESHOLD = 5;

  /** The tab being pressed or dragged. */
  let drag = $state<{
    tab: TabId;
    x: number;
    y: number;
    moving: boolean;
  } | null>(null);
  /** Set just after a drag ends, so the click that follows does nothing. */
  let dragged = false;

  /** Like the tabs, the pane's tools leave the keys where they were (the
   * terminal): a mouse click on Split right took them (#94). */
  function keepKeys(event: MouseEvent) {
    event.preventDefault();
  }

  /** Whether the pane offers to add its tabs to the multi-send group: a
   * session tab of it is not a member yet. */
  function joinable(tabs: TabId[]): boolean {
    return (
      connects &&
      tabs.some((tab) => {
        const info = handlers.tab(tab);
        return info !== null && !info.view && !info.member;
      })
    );
  }

  // In the bar with the window's buttons, the pane's tools give way first;
  // once they no longer fit, they fold into a "…" button whose menu holds
  // them and the new-tab button (#100: clipped, they could not be clicked
  // at all; the "…" alone left no room for the window's buttons).
  let bar = $state<HTMLElement>();
  let tools = $state<HTMLElement>();
  let spacer = $state<HTMLElement>();
  let more = $state<HTMLButtonElement>();
  /** Whether the tools are folded into the "…" button. */
  let folded = $state(false);
  /** Where the "…" menu is open; null: closed. */
  let toolsMenu = $state<{ x: number; y: number } | null>(null);

  /** Folds the tools when they are clipped, and unfolds them once the
   * room they would take (the free space and the "…" button's) is there. */
  function fitTools() {
    if (!bar || !tools || !spacer) return;
    const natural = tools.scrollWidth;
    if (!folded) {
      if (natural > tools.clientWidth + 0.5) folded = true;
      return;
    }
    const gap = parseFloat(getComputedStyle(bar).columnGap) || 0;
    // Unfolded, the new-tab button comes back too (as wide as the "…").
    const button = (more?.offsetWidth ?? 0) + gap;
    const needed = natural + (connects ? button : 0);
    // The spacer keeps its least width (room to drag the window, #120).
    const kept = parseFloat(getComputedStyle(spacer).minWidth) || 0;
    if (needed <= spacer.clientWidth - kept + button) folded = false;
  }

  $effect(() => {
    if (node.kind !== "pane" || !(top && right) || !bar) {
      folded = false;
      return;
    }
    const watched = [bar, tools, spacer, bar.querySelector(".tabs")];
    const observer = new ResizeObserver(() => fitTools());
    for (const element of watched) if (element) observer.observe(element);
    fitTools();
    return () => observer.disconnect();
  });

  // A tool appearing or going (Close pane, multi-send) changes the room
  // they need: measured again.
  $effect(() => {
    void several;
    if (node.kind === "pane") void joinable(node.tabs);
    queueMicrotask(fitTools);
  });

  function openToolsMenu() {
    if (toolsMenu) {
      toolsMenu = null;
      return;
    }
    const box = more?.getBoundingClientRect();
    if (box) toolsMenu = { x: box.right - 192, y: box.bottom + 2 };
  }

  /** The tools, as the "…" menu offers them. */
  function toolEntries(pane: PaneId, tabs: TabId[]): SessionAction[] {
    return [
      ...(connects
        ? [
            {
              id: "new-tab",
              label: "New connection in this pane",
              run: () => handlers.newTab(pane),
            },
          ]
        : []),
      ...(joinable(tabs)
        ? [
            {
              id: "join-pane",
              label: "Add this pane's tabs to multi-send",
              run: () => handlers.joinPane(pane),
            },
          ]
        : []),
      {
        id: "split-right",
        label: "Split right",
        run: () => handlers.split(pane, "right"),
      },
      {
        id: "split-down",
        label: "Split down",
        run: () => handlers.split(pane, "down"),
      },
      ...(several
        ? [
            {
              id: "close-pane",
              label: "Close pane",
              title: "Its tabs move to the pane next to it",
              run: () => handlers.closePane(pane),
            },
          ]
        : []),
    ];
  }

  function onpointerdown(event: PointerEvent, tab: TabId) {
    if (event.button !== 0) return;
    if ((event.target as Element).closest("[data-tab-close]")) return;
    (event.currentTarget as Element).setPointerCapture(event.pointerId);
    drag = { tab, x: event.clientX, y: event.clientY, moving: false };
  }

  function onpointermove(event: PointerEvent) {
    if (!drag || drag.moving) return;
    const distance = Math.hypot(event.clientX - drag.x, event.clientY - drag.y);
    if (distance >= DRAG_THRESHOLD) drag.moving = true;
  }

  /** Drops the dragged tab where the pointer is: into a pane's tab bar at
   * that place, or onto a pane (at the end of its tabs). */
  function onpointerup(event: PointerEvent) {
    const ended = drag;
    drag = null;
    if (!ended?.moving) return;
    // The click that may follow the drag is not a click on the tab.
    dragged = true;
    setTimeout(() => (dragged = false));
    const target = document
      .elementFromPoint(event.clientX, event.clientY)
      ?.closest<HTMLElement>("[data-pane]");
    if (!target) return;
    const pane = Number(target.dataset.pane);
    const bar = target.closest("[data-tab-bar]");
    let index = Number.MAX_SAFE_INTEGER;
    if (bar) {
      const others = [
        ...bar.querySelectorAll<HTMLElement>("[data-tab]"),
      ].filter((element) => Number(element.dataset.tab) !== ended.tab);
      index = others.filter((element) => {
        const box = element.getBoundingClientRect();
        return box.left + box.width / 2 < event.clientX;
      }).length;
    }
    handlers.move(ended.tab, pane, index);
  }

  /** The tab bar has no scrollbar: the wheel moves it sideways. */
  function scrollTabs(event: WheelEvent) {
    const bar = event.currentTarget as HTMLElement;
    if (bar.scrollWidth <= bar.clientWidth) return;
    const by = event.deltaX !== 0 ? event.deltaX : event.deltaY;
    if (by === 0) return;
    event.preventDefault();
    bar.scrollLeft += by;
  }

  function onclick(tab: TabId) {
    if (!dragged) handlers.activate(tab);
  }
</script>

{#if node.kind === "split"}
  <div class="split {node.direction}" class:corner={top && right}>
    {#each node.children as child, index (child.kind === "pane" ? `p${child.id}` : `s${panesOf(child)[0].id}`)}
      {@const last = index === node.children.length - 1}
      <PaneView
        node={child}
        {activePane}
        {several}
        {handlers}
        top={top && (node.direction === "right" || index === 0)}
        right={right && (node.direction === "down" || last)}
        {connects}
      />
    {/each}
  </div>
{:else}
  {@const pane = node}
  <div
    class="pane"
    class:active={pane.id === activePane}
    class:corner={top && right}
    data-pane={pane.id}
  >
    <!-- At the top, the bar's empty parts drag the window and a double
         click maximizes it; a tab, a button or a field never does (Tauri's
         drag region skips them). -->
    <div
      class="bar"
      class:framed={top && right}
      bind:this={bar}
      data-tab-bar={pane.id}
      data-pane={pane.id}
      data-window-bar={top || undefined}
      data-tauri-drag-region={top ? "deep" : undefined}
    >
      <div class="tabs" role="tablist" aria-label="Tabs" onwheel={scrollTabs}>
        {#each pane.tabs as tab (tab)}
          {@const info = handlers.tab(tab)}
          {#if info}
            <div
              class="tab"
              class:dragging={drag?.tab === tab && drag.moving}
              role="tab"
              tabindex="-1"
              aria-selected={pane.active === tab}
              data-tab={tab}
              data-tab-view={info.view ?? undefined}
              data-tab-state={info.view ? undefined : info.state}
              data-environment={info.environment}
              data-multi-send={info.member
                ? info.state === "connected"
                  ? "receiving"
                  : "not-receiving"
                : undefined}
              title={info.view
                ? info.title
                : `${info.title} (${STATE_TEXT[info.state]}${info.environment ? `, ${ENVIRONMENT_NAMES[info.environment]}` : ""}${info.member ? ", in the multi-send group" : ""})`}
              style:--role-color={info.role?.color}
              {@attach (element) => {
                // The shown tab stays in sight when the tabs overflow.
                if (pane.active === tab)
                  element.scrollIntoView({
                    block: "nearest",
                    inline: "nearest",
                  });
              }}
              onpointerdown={(event) => onpointerdown(event, tab)}
              {onpointermove}
              {onpointerup}
              onpointercancel={() => (drag = null)}
              onmousedown={(event) => {
                // The keys stay with the terminal.
                if (!(event.target as Element).closest("[data-tab-close]"))
                  event.preventDefault();
              }}
              onclick={() => onclick(tab)}
              onauxclick={(event) => {
                if (event.button === 1) handlers.close(tab);
              }}
              oncontextmenu={(event) => {
                event.preventDefault();
                handlers.menu(tab, event.clientX, event.clientY);
              }}
              onkeydown={(event) => {
                if (event.key === "Enter" || event.key === " ")
                  handlers.activate(tab);
              }}
            >
              {#if info.view}
                {@const Icon = VIEWS[info.view].icon}
                <span class="view-icon"
                  ><Icon size={14} aria-hidden="true" /></span
                >
              {:else}
                <span class="state {info.state}" aria-hidden="true"></span>
              {/if}
              {#if info.role}
                <span class="role"><RoleIcon role={info.role} /></span>
              {/if}
              <span class="name" data-tab-name>{info.name}</span>
              {#if info.member}
                <span class="member" data-tab-multi-send>MULTI</span>
              {/if}
              {#if info.environment === "production"}
                <span class="production" data-tab-production>PROD</span>
              {/if}
              {#if info.environment}
                <span class="visually-hidden"
                  >{ENVIRONMENT_NAMES[info.environment]}</span
                >
              {/if}
              {#if !info.view}
                <span class="visually-hidden">{STATE_TEXT[info.state]}</span>
              {/if}
              <button
                class="close"
                data-tab-close
                aria-label={`Close ${info.name}`}
                title="Close (Ctrl+Shift+W)"
                onclick={(event) => {
                  event.stopPropagation();
                  handlers.close(tab);
                }}><X size={13} aria-hidden="true" /></button
              >
            </div>
          {/if}
        {/each}
      </div>
      {#if connects && !folded}
        <button
          class="icon"
          data-new-tab
          aria-label="New connection in this pane"
          title="New connection in this pane"
          onclick={() => handlers.newTab(pane.id)}
          ><Plus size={16} aria-hidden="true" /></button
        >
      {/if}
      <span class="spacer" bind:this={spacer}></span>
      <!-- Next to the window's buttons, these give way first in a narrow
           pane, then fold into the "…" button (#100). -->
      <div class="tools" class:folded bind:this={tools} inert={folded}>
        {#if joinable(pane.tabs)}
          <button
            class="icon"
            data-join-pane
            aria-label="Add this pane's tabs to the multi-send group"
            title="Add this pane's tabs to the multi-send group"
            onclick={() => handlers.joinPane(pane.id)}
            onmousedown={keepKeys}
          >
            <UsersRound size={15} aria-hidden="true" />
          </button>
        {/if}
        <button
          class="icon"
          data-split="right"
          aria-label="Split right"
          title="Split right"
          onclick={() => handlers.split(pane.id, "right")}
          onmousedown={keepKeys}
        >
          <Columns2 size={15} aria-hidden="true" />
        </button>
        <button
          class="icon"
          data-split="down"
          aria-label="Split down"
          title="Split down"
          onclick={() => handlers.split(pane.id, "down")}
          onmousedown={keepKeys}
        >
          <Rows2 size={15} aria-hidden="true" />
        </button>
        {#if several}
          <button
            class="icon"
            data-close-pane
            aria-label="Close pane"
            title="Close pane (its tabs move to the pane next to it)"
            onclick={() => handlers.closePane(pane.id)}
            onmousedown={keepKeys}
          >
            <SquareX size={15} aria-hidden="true" />
          </button>
        {/if}
      </div>
      {#if folded}
        <button
          class="icon"
          bind:this={more}
          data-pane-tools-more
          aria-label="Pane actions"
          title="Pane actions: split, close pane"
          aria-haspopup="menu"
          aria-expanded={toolsMenu !== null}
          onclick={openToolsMenu}
          onmousedown={keepKeys}
        >
          <Ellipsis size={16} aria-hidden="true" />
        </button>
      {/if}
      {#if toolsMenu && folded}
        <SessionMenu
          entries={toolEntries(pane.id, pane.tabs)}
          x={toolsMenu.x}
          y={toolsMenu.y}
          opener={more}
          onclose={() => {
            toolsMenu = null;
            handlers.refocus();
          }}
        />
      {/if}
      {#if top && right}
        <WindowControls />
      {/if}
    </div>
    <div class="body" data-pane={pane.id} {@attach handlers.body(pane.id)}>
      {#if !connects && pane.active === null}
        <!-- A session window has no connect view (#85). -->
        <div class="empty" data-empty-pane="drop">
          <p>Drag a tab into this pane.</p>
        </div>
      {:else if pane.active === null && pane.id !== activePane}
        <div class="empty">
          <button onclick={() => handlers.activatePane(pane.id)}
            >Connect here</button
          >
          <p>Or drag a tab into this pane.</p>
        </div>
      {/if}
    </div>
  </div>
{/if}

<style>
  .split {
    display: flex;
    min-width: 0;
    min-height: 0;
    flex: 1 1 0;
    gap: 2px;
    background: var(--border);
  }

  /* The panes and splits give way in a narrow window, the top-right pane
     last: it keeps room for one tab, an empty part that drags the window,
     the "…" button and the window's buttons (#120: narrower, those left
     the window). Its splits are as wide as it needs (their other panes
     count for nothing, so tab names never widen them). */
  .split,
  .pane {
    contain: inline-size;
  }

  .split.corner {
    contain: none;
    min-width: min-content;
  }

  .pane.corner {
    min-width: var(--corner-pane-min);
  }

  .split.right {
    flex-direction: row;
  }

  .split.down {
    flex-direction: column;
  }

  .pane {
    flex: 1 1 0;
    min-width: 0;
    min-height: 0;
    display: grid;
    grid-template-rows: auto minmax(0, 1fr);
    background: var(--bg);
    /* A pane squeezed narrower than its bar clips it rather than drawing
       over the pane beside it (clip: never scrolled, as by a tab brought
       into view). */
    overflow: clip;
  }

  .bar {
    display: flex;
    align-items: center;
    gap: var(--space-0-5);
    min-width: 0;
    min-height: 38px;
    padding: 0 var(--space-1-5);
    background: var(--surface);
    border-bottom: 1px solid var(--border);
  }

  /* The window's buttons sit flush with the window's top-right corner. */
  .bar.framed {
    padding-right: 0;
  }

  .bar.framed > :global([data-window-controls]) {
    margin-left: var(--space-1-5);
  }

  .tools {
    display: flex;
    align-items: center;
    gap: var(--space-0-5);
    flex: none;
  }

  /* In the bar with the window's buttons, a narrow pane's tools give way
     first (clipped), so that the shown tab and the window's buttons stay
     in sight; other bars are as they were. */
  .bar.framed .tools {
    flex: 0 100 auto;
    min-width: 0;
    overflow: hidden;
  }

  /* Folded into the "…" button: no room taken, still measured. */
  .bar.framed .tools.folded {
    flex: 0 0 0;
    visibility: hidden;
  }

  .bar.framed .tabs:has([data-tab]) {
    min-width: 3.5rem;
  }

  .tabs {
    display: flex;
    align-items: center;
    gap: var(--space-0-5);
    padding: var(--space-1) 0;
    min-width: 0;
    flex: 0 1 auto;
    overflow-x: auto;
    scrollbar-width: none;
  }

  .pane.active .bar {
    border-bottom-color: var(--accent);
  }

  .tab {
    display: inline-flex;
    align-items: center;
    gap: var(--space-1-5);
    flex: 0 0 auto;
    max-width: 14rem;
    min-height: 28px;
    padding: var(--space-0-5) var(--space-1) var(--space-0-5) var(--space-2-5);
    font-size: var(--text-sm);
    color: var(--muted);
    background: transparent;
    border-radius: var(--radius-md);
    box-shadow:
      inset 0 2px var(--role-color, transparent),
      inset 0 -3px var(--env-color, transparent);
    cursor: pointer;
    user-select: none;
    touch-action: none;
  }

  .tab:hover {
    color: var(--text);
  }

  .tab[aria-selected="true"] {
    color: var(--text);
    background: var(--surface-raised);
  }

  .tab[data-environment="lab"] {
    --env-color: var(--env-lab);
  }

  .tab[data-environment="staging"] {
    --env-color: var(--env-staging);
  }

  /* Production cannot be mistaken for anything else: a red tab. */
  .tab[data-environment="production"] {
    --env-color: var(--env-production);
    color: var(--text);
    background: var(--env-production-bg);
  }

  .tab[data-environment="production"][aria-selected="true"] {
    outline: 1px solid var(--env-production);
    outline-offset: -1px;
  }

  .production {
    flex: 0 0 auto;
    padding: 0 var(--space-1-5);
    border-radius: var(--radius-sm);
    color: var(--on-env-production);
    background: var(--env-production);
    font-size: var(--text-2xs);
    font-weight: var(--weight-bold);
    letter-spacing: 0.06em;
  }

  .member {
    flex: 0 0 auto;
    padding: 0 var(--space-1-5);
    border: 1px solid var(--multi-send);
    border-radius: var(--radius-sm);
    color: var(--multi-send-text);
    background: var(--multi-send-bg);
    font-size: var(--text-2xs);
    font-weight: var(--weight-bold);
    letter-spacing: 0.06em;
  }

  .tab[data-multi-send] {
    outline: 1px dashed var(--multi-send);
    outline-offset: -2px;
  }

  .tab[data-multi-send="not-receiving"] .member {
    border-style: dashed;
  }

  .tab.dragging {
    opacity: 0.6;
  }

  .name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .view-icon {
    display: inline-flex;
    flex: 0 0 auto;
    color: var(--accent);
  }

  .role {
    display: inline-flex;
    color: var(--role-color);
  }

  /* The session state: hollow while connecting, filled once connected,
     darker once closed (lightness differs, not only hue). */
  .state {
    flex: 0 0 auto;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--state-connecting);
  }

  .state.connecting {
    background: transparent;
    border: 1.5px solid var(--state-connecting);
  }

  .state.connected {
    background: var(--state-connected);
  }

  .state.disconnected {
    background: var(--state-disconnected);
  }

  .close {
    display: inline-grid;
    place-items: center;
    min-height: 0;
    padding: var(--space-0-5);
    color: var(--muted);
    background: none;
    border: none;
    border-radius: var(--radius-sm);
  }

  .close:hover:not(:disabled) {
    color: var(--text);
    background: var(--border-strong);
  }

  .icon {
    flex: 0 0 auto;
    display: inline-grid;
    place-items: center;
    width: var(--pane-tool-size);
    height: var(--pane-tool-size);
    padding: 0;
    color: var(--muted);
    background: none;
    border: none;
    border-radius: var(--radius-md);
  }

  .icon:hover:not(:disabled) {
    color: var(--text);
    background: var(--surface-raised);
  }

  .spacer {
    flex: 1 1 auto;
  }

  /* The top-right bar always keeps an empty part to drag the window by. */
  .bar.framed .spacer {
    min-width: var(--space-6);
  }

  .body {
    min-width: 0;
    min-height: 0;
    position: relative;
  }

  .empty {
    height: 100%;
    display: grid;
    place-content: center;
    justify-items: center;
    gap: var(--space-2);
    color: var(--muted);
    font-size: var(--text-sm);
  }

  .empty p {
    margin: 0;
  }

  .visually-hidden {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }
</style>
