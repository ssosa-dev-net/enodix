// The WebGL renderer for terminals on screen. Browsers allow only a few
// WebGL contexts at once (Chromium: 16, dropping the oldest beyond that), so
// only visible terminals get one, and never more than `MAX_WEBGL_TERMINALS`;
// the others use xterm.js's DOM renderer until a context is free. A context
// given up is released at once (not left to garbage collection), so showing
// tabs one after another never piles contexts up. A lost context (the GPU
// reset, or the browser reclaimed it) falls back to the DOM renderer and is
// attached again shortly after, a few times at most while the terminal stays
// on screen.

import { WebglAddon } from "@xterm/addon-webgl";
import type { Terminal } from "@xterm/xterm";

/** Well below Chromium's limit, leaving room for anything else. */
const MAX_WEBGL_TERMINALS = 8;
/** How long after losing its context a terminal tries again. */
const RETRY_AFTER_MS = 1000;
/** Losses tolerated while on screen before keeping the DOM renderer. */
const MAX_LOSSES = 3;

let inUse = 0;
/** Visible terminals turned away at the limit, waiting for a context. */
const waiting = new Set<() => void>();

/** Gives the next waiting terminal the context just freed. */
function contextFreed() {
  const [next] = waiting;
  if (!next) return;
  waiting.delete(next);
  next();
}

/**
 * Gives `terminal` the WebGL renderer while it is on screen. Call `show()`
 * when it becomes visible and `hide()` when it is hidden; `dispose()` when it
 * goes.
 */
export function webglRenderer(terminal: Terminal) {
  let addon: WebglAddon | null = null;
  let visible = false;
  let losses = 0;
  let retry: ReturnType<typeof setTimeout> | null = null;

  /** Gives the context up; `release` frees it at once (it is not lost). */
  function detach(release: boolean) {
    if (!addon) return;
    const attached = addon;
    addon = null;
    // The addon's canvas; its context outlives `dispose` until collected.
    const canvas = terminal.element?.querySelector<HTMLCanvasElement>(
      ".xterm-screen canvas:not(.xterm-link-layer)",
    );
    attached.dispose();
    if (release) {
      canvas
        ?.getContext("webgl2")
        ?.getExtension("WEBGL_lose_context")
        ?.loseContext();
    }
    inUse -= 1;
    contextFreed();
  }

  function attach() {
    if (addon || !visible || losses >= MAX_LOSSES) return;
    if (inUse >= MAX_WEBGL_TERMINALS) {
      waiting.add(attach);
      return;
    }
    let created: WebglAddon;
    try {
      created = new WebglAddon();
      terminal.loadAddon(created);
    } catch {
      // No WebGL now (no GPU, or blocked): the DOM renderer stays until
      // the terminal is shown again.
      losses = MAX_LOSSES;
      return;
    }
    addon = created;
    inUse += 1;
    created.onContextLoss(() => {
      losses += 1;
      detach(false);
      if (retry === null) {
        retry = setTimeout(() => {
          retry = null;
          attach();
        }, RETRY_AFTER_MS);
      }
    });
  }

  return {
    show() {
      if (visible) return;
      visible = true;
      losses = 0;
      attach();
    },
    hide() {
      visible = false;
      waiting.delete(attach);
      detach(true);
    },
    dispose() {
      visible = false;
      waiting.delete(attach);
      if (retry !== null) clearTimeout(retry);
      detach(true);
    },
  };
}
