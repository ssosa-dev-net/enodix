<script lang="ts" module>
  import Columns2 from "@lucide/svelte/icons/columns-2";
  import Plus from "@lucide/svelte/icons/plus";
  import Rows2 from "@lucide/svelte/icons/rows-2";
  import SquareX from "@lucide/svelte/icons/square-x";
  import UsersRound from "@lucide/svelte/icons/users-round";
  import X from "@lucide/svelte/icons/x";
  import type { Attachment } from "svelte/attachments";
  import type { LayoutNode, PaneId, SplitDirection, TabId } from "./layout";
  import type { Environment, Role } from "./workspace";

  /** What a tab shows about its session. */
  export interface TabInfo {
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
    /** Attaches to each pane's body, where its terminal is placed. */
    body: (pane: PaneId) => Attachment<HTMLElement>;
  }
</script>

<script lang="ts">
  import PaneView from "./PaneView.svelte";
  import { panesOf } from "./layout";
  import RoleIcon from "./RoleIcon.svelte";
  import { ENVIRONMENT_NAMES } from "./paste";

  interface Props {
    node: LayoutNode;
    activePane: PaneId;
    /** Whether there is more than one pane (each can then be closed). */
    several: boolean;
    handlers: PaneHandlers;
  }

  let { node, activePane, several, handlers }: Props = $props();

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

  function onclick(tab: TabId) {
    if (!dragged) handlers.activate(tab);
  }
</script>

{#if node.kind === "split"}
  <div class="split {node.direction}">
    {#each node.children as child (child.kind === "pane" ? `p${child.id}` : `s${panesOf(child)[0].id}`)}
      <PaneView node={child} {activePane} {several} {handlers} />
    {/each}
  </div>
{:else}
  {@const pane = node}
  <div class="pane" class:active={pane.id === activePane} data-pane={pane.id}>
    <div class="bar" data-tab-bar={pane.id} data-pane={pane.id}>
      <div class="tabs" role="tablist" aria-label="Sessions">
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
              data-tab-state={info.state}
              data-environment={info.environment}
              data-multi-send={info.member
                ? info.state === "connected"
                  ? "receiving"
                  : "not-receiving"
                : undefined}
              title={`${info.title} (${STATE_TEXT[info.state]}${info.environment ? `, ${ENVIRONMENT_NAMES[info.environment]}` : ""}${info.member ? ", in the multi-send group" : ""})`}
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
              onkeydown={(event) => {
                if (event.key === "Enter" || event.key === " ")
                  handlers.activate(tab);
              }}
            >
              <span class="state {info.state}" aria-hidden="true"></span>
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
              <span class="visually-hidden">{STATE_TEXT[info.state]}</span>
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
      <button
        class="icon"
        data-new-tab
        aria-label="New connection in this pane"
        title="New connection in this pane"
        onclick={() => handlers.newTab(pane.id)}
        ><Plus size={16} aria-hidden="true" /></button
      >
      <span class="spacer"></span>
      {#if pane.tabs.some((tab) => handlers.tab(tab)?.member === false)}
        <button
          class="icon"
          data-join-pane
          aria-label="Add this pane's tabs to the multi-send group"
          title="Add this pane's tabs to the multi-send group"
          onclick={() => handlers.joinPane(pane.id)}
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
      >
        <Columns2 size={15} aria-hidden="true" />
      </button>
      <button
        class="icon"
        data-split="down"
        aria-label="Split down"
        title="Split down"
        onclick={() => handlers.split(pane.id, "down")}
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
        >
          <SquareX size={15} aria-hidden="true" />
        </button>
      {/if}
    </div>
    <div class="body" data-pane={pane.id} {@attach handlers.body(pane.id)}>
      {#if pane.active === null && pane.id !== activePane}
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

  .tabs {
    display: flex;
    align-items: center;
    gap: var(--space-0-5);
    padding: var(--space-1) 0;
    min-width: 0;
    flex: 0 1 auto;
    overflow-x: auto;
    scrollbar-width: thin;
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
    opacity: 0.7;
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
    width: 28px;
    height: 28px;
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
