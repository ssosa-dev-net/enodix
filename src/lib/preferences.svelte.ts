// The user's preferences (theme, terminal font, scrollback, copy and
// paste), as the core keeps them in the data folder, and whether the
// preferences dialog is open. The interface applies them: the theme to the
// whole page, the rest to every terminal.

import { invoke } from "@tauri-apps/api/core";
import type { ITheme } from "@xterm/xterm";

export type Theme = "system" | "light" | "dark";
export type FontZoom = "in" | "out" | "reset";

export interface Preferences {
  theme: Theme;
  fontFamily: string;
  fontSize: number;
  scrollback: number;
  copyOnSelect: boolean;
  rightClickPaste: boolean;
  /** Keyword highlighting in every terminal. */
  highlighting: boolean;
  /** A multi-line paste asks first (production hosts always ask). */
  confirmMultiLinePaste: boolean;
}

/** The core's limits (`netterm_core::preferences`), for the form. */
export const FONT_SIZES = { min: 8, max: 36 } as const;
export const MAX_SCROLLBACK = 100_000;

/** Offered in the font field; any installed font can be typed. */
export const SUGGESTED_FONTS = [
  "Cascadia Mono",
  "Cascadia Code",
  "Consolas",
  "Lucida Console",
  "Courier New",
  "JetBrains Mono",
  "Fira Code",
  "Source Code Pro",
  "DejaVu Sans Mono",
];

const systemDark = window.matchMedia("(prefers-color-scheme: dark)");

export const preferences = $state({
  /** The core's defaults until it answers (see `loadPreferences`). */
  current: {
    theme: "system",
    fontFamily: "Cascadia Mono",
    fontSize: 14,
    scrollback: 10_000,
    copyOnSelect: false,
    rightClickPaste: false,
    highlighting: true,
    confirmMultiLinePaste: true,
  } as Preferences,
  /** Whether the operating system is set to dark. */
  systemDark: systemDark.matches,
  /** Why the last change could not be saved, if it could not. */
  error: null as string | null,
  /** Whether the preferences dialog is open. */
  dialog: false,
});

// The system setting is followed live, not only at launch.
systemDark.addEventListener(
  "change",
  (event) => (preferences.systemDark = event.matches),
);

/** Whether the app is dark now: the preference, or the system's. */
export function isDark(): boolean {
  const theme = preferences.current.theme;
  return theme === "system" ? preferences.systemDark : theme === "dark";
}

// Requests go to the core one at a time, in order, so that a zoom never
// overtakes an earlier change (or the other way round).
let queue: Promise<void> = Promise.resolve();

/** Runs `request` after the earlier ones and shows what the core saved; on
 * failure, says why and shows what the core still has. */
function adopt(request: () => Promise<Preferences>): Promise<void> {
  queue = queue.then(async () => {
    try {
      preferences.current = await request();
      preferences.error = null;
    } catch (error) {
      preferences.error = String(error);
      try {
        preferences.current = await invoke<Preferences>("preferences_get");
      } catch {
        // Keep what is shown; the error says it was not saved.
      }
    }
  });
  return queue;
}

export function loadPreferences() {
  return adopt(() => invoke<Preferences>("preferences_get"));
}

/** Saves `changes` over the current preferences (the core normalises). */
export function changePreferences(changes: Partial<Preferences>) {
  // Shown at once; the core's answer then replaces it.
  preferences.current = { ...preferences.current, ...changes };
  return adopt(() =>
    invoke<Preferences>("preferences_save", {
      // Over what the earlier requests left, when this one's turn comes.
      preferences: { ...preferences.current, ...changes },
    }),
  );
}

/** Steps the terminal font size, or resets it. */
export function zoomFont(zoom: FontZoom) {
  return adopt(() => invoke<Preferences>("preferences_zoom", { zoom }));
}

/** The CSS font stack for a family name, falling back to monospace. */
export function fontStack(family: string): string {
  const named = /[,"']/.test(family) ? family : `"${family}"`;
  return `${named}, "Cascadia Mono", Consolas, "Courier New", monospace`;
}

/** The terminal's colours for the app's theme. */
export function terminalTheme(dark: boolean): ITheme {
  return dark ? DARK_TERMINAL : LIGHT_TERMINAL;
}

// xterm.js's own ANSI colours (Tango), written out so that highlight
// rules naming a palette colour get the same one as the device's own.
const DARK_TERMINAL: ITheme = {
  background: "#0e1116",
  foreground: "#d7dce2",
  cursor: "#d7dce2",
  cursorAccent: "#0e1116",
  selectionBackground: "#3a5a8c",
  selectionForeground: "#ffffff",
  black: "#2e3436",
  red: "#cc0000",
  green: "#4e9a06",
  yellow: "#c4a000",
  blue: "#3465a4",
  magenta: "#75507b",
  cyan: "#06989a",
  white: "#d3d7cf",
  brightBlack: "#555753",
  brightRed: "#ef2929",
  brightGreen: "#8ae234",
  brightYellow: "#fce94f",
  brightBlue: "#729fcf",
  brightMagenta: "#ad7fa8",
  brightCyan: "#34e2e2",
  brightWhite: "#eeeeec",
};

/**
 * A highlight rule's colour as `#rrggbb`: a palette name (`bright-cyan`)
 * becomes the theme's colour.
 */
export function highlightColor(color: string, dark: boolean): string {
  if (color.startsWith("#")) return color;
  const key = color.replace(/-(\w)/g, (_, c: string) => c.toUpperCase());
  const value = terminalTheme(dark)[key as keyof ITheme];
  return typeof value === "string" ? value : terminalTheme(dark).foreground!;
}

// Darker ANSI colours than xterm's, so that yellow, cyan and white stay
// readable on a light background.
const LIGHT_TERMINAL: ITheme = {
  background: "#fbfbfc",
  foreground: "#1f2328",
  cursor: "#1f2328",
  cursorAccent: "#fbfbfc",
  selectionBackground: "#b6d3ff",
  selectionForeground: "#1f2328",
  black: "#1f2328",
  red: "#c4161c",
  green: "#1a7f37",
  yellow: "#8a6100",
  blue: "#0b5cd6",
  magenta: "#8f2bb3",
  cyan: "#0f7b84",
  white: "#6e7781",
  brightBlack: "#57606a",
  brightRed: "#d73a49",
  brightGreen: "#2da44e",
  brightYellow: "#a87c00",
  brightBlue: "#2f74e0",
  brightMagenta: "#a742c9",
  brightCyan: "#1b8f99",
  brightWhite: "#8c959f",
};
