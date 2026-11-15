// Where each pane's body is, so that a window can place its terminals
// over the panes rather than inside them: moving a tab to another pane
// then keeps its terminal (and session). The main window and session
// windows (#85) both do.

import type { Attachment } from "svelte/attachments";
import type { PaneId } from "./layout";

export interface PaneRect {
  left: number;
  top: number;
  width: number;
  height: number;
}

/**
 * The inner size of a new session window (#84) whose terminal keeps the
 * columns and rows it has over `rect`: a tab bar, the session, and the
 * status line, measured in this window where they are drawn. The tab bar
 * is a pane's own (`[data-tab-bar]`): `[data-window-bar]` is also on a
 * side view's header, which comes first in the main window and need not
 * be as tall.
 */
export function sessionWindowSize(rect: PaneRect) {
  const height = (selector: string, otherwise: number) =>
    document.querySelector(selector)?.getBoundingClientRect().height ||
    otherwise;
  return {
    width: rect.width,
    height:
      rect.height +
      height("[data-tab-bar]", 38) +
      height("[data-status-line]", 22),
  };
}

export class PaneRects {
  /** Each pane's body, relative to `area`. */
  rects = $state<Record<PaneId, PaneRect>>({});
  /** The element the panes are drawn in, and the slots placed over. */
  area: HTMLElement | undefined = undefined;
  #bodies = new Map<PaneId, HTMLElement>();
  #resizes = new ResizeObserver(() => this.measure());

  /** Measures every pane's body again (panes can move without changing
   * size: another pane closing). */
  measure() {
    if (!this.area) return;
    const origin = this.area.getBoundingClientRect();
    const next: Record<PaneId, PaneRect> = {};
    for (const [pane, body] of this.#bodies) {
      const box = body.getBoundingClientRect();
      next[pane] = {
        left: box.left - origin.left,
        top: box.top - origin.top,
        width: box.width,
        height: box.height,
      };
    }
    this.rects = next;
  }

  /** Attaches to `pane`'s body, measured from then on. */
  body(pane: PaneId): Attachment<HTMLElement> {
    return (body) => {
      this.#bodies.set(pane, body);
      this.#resizes.observe(body);
      this.measure();
      return () => {
        this.#bodies.delete(pane);
        this.#resizes.unobserve(body);
      };
    };
  }
}
