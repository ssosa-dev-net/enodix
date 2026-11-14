<script lang="ts" module>
  import type { Attachment } from "svelte/attachments";
  import type { LayoutNode, PaneId, SplitDirection, TabId } from "./layout";
  import type { Role } from "./workspace";

  /** What a tab shows about its session. */
  export interface TabInfo {
    /** The host name (or address, or serial port). */
    name: string;
    /** More detail, for the tooltip (`user@host:port`). */
    title: string;
    role: Role | null;
    state: "connecting" | "connected" | "disconnected";
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
    /** Attaches to each pane's body, where its terminal is placed. */
    body: (pane: PaneId) => Attachment<HTMLElement>;
  }
</script>

<script lang="ts">
  import PaneView from "./PaneView.svelte";
  import { panesOf } from "./layout";
  import RoleIcon from "./RoleIcon.svelte";

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
              title={`${info.title} (${STATE_TEXT[info.state]})`}
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
              <span class="visually-hidden">{STATE_TEXT[info.state]}</span>
              <button
                class="close"
                data-tab-close
                aria-label={`Close ${info.name}`}
                title="Close (Ctrl+Shift+W)"
                onclick={(event) => {
                  event.stopPropagation();
                  handlers.close(tab);
                }}>×</button
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
        onclick={() => handlers.newTab(pane.id)}>+</button
      >
      <span class="spacer"></span>
      <button
        class="icon"
        data-split="right"
        aria-label="Split right"
        title="Split right"
        onclick={() => handlers.split(pane.id, "right")}
      >
        <svg viewBox="0 0 16 16" aria-hidden="true"
          ><rect x="1.5" y="2.5" width="13" height="11" rx="1" /><path
            d="M8 2.5v11"
          /></svg
        >
      </button>
      <button
        class="icon"
        data-split="down"
        aria-label="Split down"
        title="Split down"
        onclick={() => handlers.split(pane.id, "down")}
      >
        <svg viewBox="0 0 16 16" aria-hidden="true"
          ><rect x="1.5" y="2.5" width="13" height="11" rx="1" /><path
            d="M1.5 8h13"
          /></svg
        >
      </button>
      {#if several}
        <button
          class="icon"
          data-close-pane
          aria-label="Close pane"
          title="Close pane (its tabs move to the pane next to it)"
          onclick={() => handlers.closePane(pane.id)}
        >
          <svg viewBox="0 0 16 16" aria-hidden="true"
            ><path d="M4 4l8 8M12 4l-8 8" /></svg
          >
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
    align-items: stretch;
    min-width: 0;
    background: var(--bg);
    border-bottom: 1px solid var(--border);
  }

  .tabs {
    display: flex;
    align-items: stretch;
    gap: 1px;
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
    gap: 0.35rem;
    flex: 0 0 auto;
    max-width: 14rem;
    padding: 0.25rem 0.3rem 0.25rem 0.6rem;
    font-size: 0.8rem;
    color: var(--muted);
    background: var(--bg);
    border-right: 1px solid var(--border);
    box-shadow: inset 0 2px var(--role-color, transparent);
    cursor: pointer;
    user-select: none;
    touch-action: none;
  }

  .tab[aria-selected="true"] {
    color: var(--text);
    background: var(--surface);
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

  .state {
    flex: 0 0 auto;
    width: 0.5rem;
    height: 0.5rem;
    border-radius: 50%;
    background: var(--muted);
  }

  .state.connecting {
    background: transparent;
    border: 1.5px solid var(--warning);
  }

  .state.connected {
    background: var(--ok);
  }

  .state.disconnected {
    background: var(--danger);
  }

  .close {
    padding: 0 0.3rem;
    font-size: 0.9rem;
    line-height: 1.2;
    color: var(--muted);
    background: none;
    border: none;
  }

  .close:hover {
    color: var(--text);
  }

  .icon {
    flex: 0 0 auto;
    display: inline-grid;
    place-items: center;
    padding: 0 0.5rem;
    color: var(--muted);
    background: none;
    border: none;
    border-radius: 0;
  }

  .icon:hover {
    color: var(--text);
  }

  .icon svg {
    width: 0.95rem;
    height: 0.95rem;
    fill: none;
    stroke: currentColor;
    stroke-width: 1.3;
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
    gap: 0.5rem;
    color: var(--muted);
    font-size: 0.85rem;
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
