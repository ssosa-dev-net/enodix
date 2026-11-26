// Keyword highlighting in the terminals. The core decides what stands out
// in a line (`highlight_lines`, by the open workspace's rules); here the
// lines a terminal shows are sent to it and the answer is drawn as xterm.js
// decorations over the cells. Only the screen is looked at, never the
// session's bytes: what is sent, received and logged is the same with
// highlighting on or off, and the cost does not grow with the output, only
// with the terminal's size.

import type { IBufferLine, IDecoration, IMarker, Terminal } from "@xterm/xterm";
import { inventory } from "./inventory.svelte";
import { highlightColor, paletteKey } from "./preferences.svelte";
import {
  builtInHighlightRules,
  highlightLines,
  type HighlightSpan,
  type ListedRule,
} from "./workspace";

export const highlighting = $state({
  /** The built-in rules, for when no workspace is open. */
  builtIn: [] as ListedRule[],
  /** Raised whenever the rules that apply change. */
  version: 0,
});

/** The rules that apply now: the open workspace's, else the built-in ones. */
export function currentRules(): ListedRule[] {
  return inventory.workspace?.highlight_rules ?? highlighting.builtIn;
}

/** What the core answered per line of text, for the current rules. */
const answers = new Map<string, HighlightSpan[]>();
/** Lines kept at most; then they are asked for again. */
const MAX_ANSWERS = 20_000;
/** Rows beyond the screen a wrapped line is followed into. */
const WRAP_MARGIN = 4;

/**
 * Follows the rules that apply (call once, from a component): when they
 * change, every terminal highlights again.
 */
export function trackHighlightRules() {
  builtInHighlightRules().then(
    (rules) => (highlighting.builtIn = rules),
    () => {},
  );
  let seen = "";
  $effect(() => {
    const rules = JSON.stringify(currentRules());
    if (rules === seen) return;
    seen = rules;
    answers.clear();
    highlighting.version += 1;
  });
}

/** One highlighted row of a terminal and how it was drawn. */
interface Drawn {
  marker: IMarker;
  /** What it was drawn from: the line's text, the row in it, the colours. */
  key: string;
  decorations: IDecoration[];
  /** For the smoke test: the cells coloured. */
  cells: { col: number; width: number; color: string }[];
}

/** A line as the terminal shows it, perhaps wrapped over several rows. */
interface Line {
  /** Its first row, in buffer lines. */
  start: number;
  rows: number;
  text: string;
  /** Per code point of `text`: where it is drawn. */
  places: { row: number; col: number; width: number }[];
}

export interface TerminalHighlighter {
  /** Highlights (or stops), after the preference, theme or rules changed. */
  update(enabled: boolean, dark: boolean): void;
  /** The highlighted cells on screen, by buffer line (for the smoke test). */
  shown(): { line: number; col: number; width: number; color: string }[];
  dispose(): void;
}

/** Highlights what `terminal` shows. */
export function highlightTerminal(terminal: Terminal): TerminalHighlighter {
  let enabled = false;
  let dark = false;
  let palette = paletteKey();
  let version = -1;
  let drawn: Drawn[] = [];
  let scheduled = false;
  let asking = false;
  let disposed = false;
  const cell = terminal.buffer.active.getNullCell();

  const undraw = (row: Drawn) => {
    for (const decoration of row.decorations) decoration.dispose();
    row.marker.dispose();
  };
  const clear = () => {
    drawn.forEach(undraw);
    drawn = [];
  };

  const schedule = () => {
    if (scheduled || disposed) return;
    scheduled = true;
    requestAnimationFrame(() => {
      scheduled = false;
      if (!disposed) pass();
    });
  };

  /**
   * The line the buffer line `y` is part of (wrapped rows join), but no
   * further than `WRAP_MARGIN` rows beyond the screen (`top` to `bottom`):
   * output without line ends can wrap over the whole scrollback.
   */
  const lineAt = (y: number, top: number, bottom: number): Line => {
    const buffer = terminal.buffer.active;
    const first = Math.max(0, top - WRAP_MARGIN);
    const last = Math.min(buffer.length, bottom + WRAP_MARGIN);
    let start = y;
    while (start > first && buffer.getLine(start)?.isWrapped) start -= 1;
    let end = y + 1;
    while (end < last && buffer.getLine(end)?.isWrapped) end += 1;
    let text = "";
    const places: Line["places"] = [];
    for (let row = start; row < end; row += 1) {
      const line = buffer.getLine(row) as IBufferLine;
      for (let col = 0; col < line.length; col += 1) {
        line.getCell(col, cell);
        const width = cell.getWidth();
        // The second half of a wide character.
        if (width === 0) continue;
        for (const point of cell.getChars() || " ") {
          text += point;
          places.push({ row, col, width });
        }
      }
    }
    // Blanks at the end are not text (and would spoil the reuse of answers).
    const trimmed = text.trimEnd();
    places.length = [...trimmed].length;
    return { start, rows: end - start, text: trimmed, places };
  };

  /** Asks the core about `lines`, then draws again. */
  const ask = (lines: string[]) => {
    if (asking) return;
    asking = true;
    const asked = highlighting.version;
    highlightLines(lines)
      .then(
        (spans) => {
          if (asked !== highlighting.version) return;
          if (answers.size + lines.length > MAX_ANSWERS) answers.clear();
          lines.forEach((line, i) => answers.set(line, spans[i] ?? []));
        },
        // Not asked again: nothing to highlight in them.
        () => lines.forEach((line) => answers.set(line, [])),
      )
      .finally(() => {
        asking = false;
        schedule();
      });
  };

  /** Draws the highlights of the rows on screen. */
  const pass = () => {
    if (!enabled) {
      clear();
      return;
    }
    const buffer = terminal.buffer.active;
    const top = buffer.viewportY;
    const bottom = Math.min(top + terminal.rows, buffer.length);
    const lines: Line[] = [];
    for (let y = top; y < bottom;) {
      const line = lineAt(y, top, bottom);
      lines.push(line);
      y = line.start + line.rows;
    }
    const unknown = [
      ...new Set(lines.filter((l) => !answers.has(l.text)).map((l) => l.text)),
    ];
    if (unknown.length > 0) ask(unknown);

    const previous = new Map<number, Drawn>();
    for (const row of drawn) {
      const line = row.marker.line;
      if (row.marker.isDisposed || line < top || line >= bottom) undraw(row);
      else previous.set(line, row);
    }
    const next: Drawn[] = [];
    const cursor = buffer.baseY + buffer.cursorY;
    for (const line of lines) {
      const spans = answers.get(line.text);
      for (let i = 0; i < line.rows; i += 1) {
        const row = line.start + i;
        if (row < top || row >= bottom) continue;
        const key = `${version}|${dark}|${palette}|${i}|${line.text}`;
        const old = previous.get(row);
        if (old && old.key === key) {
          next.push(old);
          previous.delete(row);
          continue;
        }
        // Not known yet: drawn once the core answers.
        if (!spans) continue;
        const cells = cellsOf(line, row, spans);
        if (cells.length === 0) continue;
        const marker = terminal.registerMarker(row - cursor);
        if (!marker) continue;
        const decorations = cells.flatMap(({ col, width, color }) => {
          const decoration = terminal.registerDecoration({
            marker,
            x: col,
            width,
            foregroundColor: color,
          });
          return decoration ? [decoration] : [];
        });
        next.push({ marker, key, decorations, cells });
      }
    }
    // Rows whose text changed, or that no longer have highlights.
    previous.forEach(undraw);
    drawn = next;
  };

  /** The runs of cells of `row` that `spans` colour. */
  const cellsOf = (line: Line, row: number, spans: HighlightSpan[]) => {
    const cells: Drawn["cells"] = [];
    for (const span of spans) {
      const color = highlightColor(span.color, dark);
      let run: Drawn["cells"][number] | null = null;
      for (let i = span.start; i < span.end && i < line.places.length; i++) {
        const place = line.places[i];
        if (place.row !== row) {
          run = null;
          continue;
        }
        const end = place.col + place.width;
        if (run && place.col <= run.col + run.width) {
          run.width = Math.max(run.width, end - run.col);
        } else {
          run = { col: place.col, width: place.width, color };
          cells.push(run);
        }
      }
    }
    return cells;
  };

  const subscriptions = [
    terminal.onRender(schedule),
    terminal.onScroll(schedule),
    terminal.onResize(schedule),
    terminal.buffer.onBufferChange(() => {
      // Markers belong to the buffer they were made in.
      clear();
      schedule();
    }),
  ];

  return {
    update(on, isDark) {
      const colours = paletteKey();
      const changed =
        on !== enabled ||
        isDark !== dark ||
        colours !== palette ||
        version !== highlighting.version;
      enabled = on;
      dark = isDark;
      palette = colours;
      version = highlighting.version;
      if (!changed) return;
      if (!enabled) clear();
      schedule();
    },
    shown: () =>
      drawn.flatMap((row) =>
        row.cells.map((cells) => ({ line: row.marker.line, ...cells })),
      ),
    dispose() {
      disposed = true;
      subscriptions.forEach((subscription) => subscription.dispose());
      clear();
    },
  };
}
