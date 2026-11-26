// The user's preferences (theme, terminal font, scrollback, copy and
// paste, the update check), as the core keeps them in the data folder
// (edited in the Preferences view tab, #68). The interface applies them: the theme to the
// whole page, the rest to every terminal.

import { invoke } from "@tauri-apps/api/core";
import type { ITheme } from "@xterm/xterm";
import {
  COLOR_NAMES,
  SCHEME_THEMES,
  themeKey,
  type ColorScheme,
} from "./colorSchemes";

export type { ColorScheme };

export type Theme = "system" | "light" | "dark";
export type FontZoom = "in" | "out" | "reset";
/** What the activity rail shows in the side view. */
export type SideView =
  "hosts" | "snippets" | "jobs" | "backups" | "credentials";

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
  /** Look for a newer version at start (off: no connection on its own). */
  checkForUpdates: boolean;
  /** The side view the rail selected; `leftPanelWidth` is its width and
   * `leftPanelHidden` whether it is folded away. */
  sideView: SideView;
  /** The side view's width in pixels (`PANEL_WIDTHS`). */
  leftPanelWidth: number;
  /** The side view folded away, for a wider terminal. */
  leftPanelHidden: boolean;
  /** The local shell a new local session opens (`LocalShell.id`); empty:
   * the first one the computer lists. */
  localShell: string;
  /** The terminal's colour scheme (#24). */
  colorScheme: ColorScheme;
  /** Colours set over the scheme's, by `COLOR_NAMES`, each `#rrggbb`. */
  customColors: Record<string, string>;
}

/** The core's limits (`enodix_core::preferences`), for the form. */
export const FONT_SIZES = { min: 8, max: 36 } as const;
export const MAX_SCROLLBACK = 100_000;
export const PANEL_WIDTHS = {
  min: 200,
  max: 800,
  left: 288,
} as const;

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
    checkForUpdates: true,
    sideView: "hosts",
    leftPanelWidth: PANEL_WIDTHS.left,
    leftPanelHidden: false,
    localShell: "",
    colorScheme: "default",
    customColors: {},
  } as Preferences,
  /** Whether the operating system is set to dark. */
  systemDark: systemDark.matches,
  /** Why the last change could not be saved, if it could not. */
  error: null as string | null,
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
/** Changes shown but not saved yet. Every answer from the core is shown
 * with these over it, so an earlier request's answer cannot undo a newer
 * change (two quick toggles would otherwise end where they started). */
let pending: Partial<Preferences>[] = [];

function adopt(
  request: () => Promise<Preferences>,
  saving?: Partial<Preferences>,
): Promise<void> {
  queue = queue.then(async () => {
    const shown = (answer: Preferences) => {
      if (saving) pending = pending.filter((change) => change !== saving);
      preferences.current = Object.assign({}, answer, ...pending);
    };
    try {
      shown(await request());
      preferences.error = null;
    } catch (error) {
      preferences.error = String(error);
      try {
        shown(await invoke<Preferences>("preferences_get"));
      } catch {
        // Keep what is shown; the error says it was not saved.
        if (saving) pending = pending.filter((change) => change !== saving);
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
  pending.push(changes);
  preferences.current = { ...preferences.current, ...changes };
  return adopt(
    () =>
      invoke<Preferences>("preferences_save", {
        // Over what the earlier requests left, when this one's turn comes.
        preferences: { ...preferences.current, ...changes },
      }),
    changes,
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

/** The terminal's colours: the chosen scheme (the app's own, light or dark
 * as `dark` says, unless one is chosen), with the user's own colours over it. */
export function terminalTheme(dark: boolean): ITheme {
  const { colorScheme, customColors } = preferences.current;
  const base =
    colorScheme !== "default" && SCHEME_THEMES[colorScheme]
      ? SCHEME_THEMES[colorScheme]
      : dark
        ? DARK_TERMINAL
        : LIGHT_TERMINAL;
  const theme: ITheme = { ...base };
  for (const name of COLOR_NAMES) {
    const color = customColors[name];
    if (color) (theme as Record<string, string>)[themeKey(name)] = color;
  }
  return theme;
}

/** Changes whenever the colours highlight rules resolve to change (so a
 * highlighter redraws when the scheme or an own colour does). */
export function paletteKey(): string {
  const { colorScheme, customColors } = preferences.current;
  return `${colorScheme}|${JSON.stringify(customColors)}`;
}

// xterm.js's own ANSI colours (Tango), written out so that highlight
// rules naming a palette colour get the same one as the device's own.
const DARK_TERMINAL: ITheme = {
  background: "#0b0d10",
  foreground: "#cfd6de",
  cursor: "#cfd6de",
  cursorAccent: "#0b0d10",
  selectionBackground: "#2d4f78",
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
