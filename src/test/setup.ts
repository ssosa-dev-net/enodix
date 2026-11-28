// What jsdom lacks that the interface and xterm.js use, as a page in the
// app's webview has it. Nothing here stands in for the app's own code.

import { clearMocks } from "@tauri-apps/api/mocks";
import { afterEach } from "vitest";

// The system's light or dark setting (`preferences.svelte.ts` reads it on
// import) and xterm.js's pixel ratio: never matching, never changing.
window.matchMedia = (query: string) =>
  ({
    matches: false,
    media: query,
    onchange: null,
    addEventListener() {},
    removeEventListener() {},
    addListener() {},
    removeListener() {},
    dispatchEvent: () => false,
  }) as MediaQueryList;

// A terminal fits its pane when the pane's size changes; jsdom lays nothing
// out, so it never does.
globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
};

// jsdom draws nothing: no 2D or WebGL context, as on a webview without a
// GPU (the terminal keeps xterm.js's DOM renderer). Without this jsdom
// prints a "not implemented" error for each terminal.
HTMLCanvasElement.prototype.getContext = (() =>
  null) as typeof HTMLCanvasElement.prototype.getContext;

// Each test starts with no shell (`src/test/shell.ts` puts one in place).
afterEach(() => {
  clearMocks();
  document.body.replaceChildren();
});
