// The terminal's built-in colour schemes (#24), besides the app's own
// light and dark terminal colours (`preferences.svelte.ts`). A scheme sets
// the terminal's colours and the 16 ANSI colours, so a highlight rule that
// names a palette colour follows the scheme.

import type { ITheme } from "@xterm/xterm";

/** `default`: the app's own colours, light or dark as its theme is. */
export type ColorScheme =
  | "default"
  | "solarized-dark"
  | "solarized-light"
  | "dracula"
  | "nord"
  | "gruvbox-dark";

export const COLOR_SCHEMES: { id: ColorScheme; name: string }[] = [
  { id: "default", name: "Same as the app's theme" },
  { id: "solarized-dark", name: "Solarized Dark" },
  { id: "solarized-light", name: "Solarized Light" },
  { id: "dracula", name: "Dracula" },
  { id: "nord", name: "Nord" },
  { id: "gruvbox-dark", name: "Gruvbox Dark" },
];

/** The colours a user can set on their own, over the scheme's: the names
 * highlight rules use for the palette, and the terminal's own four. The
 * core keeps this list too (`enodix_core::preferences::COLOR_NAMES`). */
export const COLOR_NAMES = [
  "background",
  "foreground",
  "cursor",
  "selection",
  "black",
  "red",
  "green",
  "yellow",
  "blue",
  "magenta",
  "cyan",
  "white",
  "bright-black",
  "bright-red",
  "bright-green",
  "bright-yellow",
  "bright-blue",
  "bright-magenta",
  "bright-cyan",
  "bright-white",
] as const;

export type ColorName = (typeof COLOR_NAMES)[number];

/** The `ITheme` key a colour name sets (`bright-cyan` → `brightCyan`,
 * `selection` → `selectionBackground`). */
export function themeKey(name: ColorName): keyof ITheme {
  if (name === "selection") return "selectionBackground";
  return name.replace(/-(\w)/g, (_, c: string) =>
    c.toUpperCase(),
  ) as keyof ITheme;
}

/** One scheme from its terminal colours and its 16 ANSI colours (normal
 * black to white, then bright black to white). */
function scheme(
  [background, foreground, cursor, selection]: [string, string, string, string],
  ansi: string[],
): ITheme {
  const [black, red, green, yellow, blue, magenta, cyan, white] = ansi;
  return {
    background,
    foreground,
    cursor,
    cursorAccent: background,
    selectionBackground: selection,
    black,
    red,
    green,
    yellow,
    blue,
    magenta,
    cyan,
    white,
    brightBlack: ansi[8],
    brightRed: ansi[9],
    brightGreen: ansi[10],
    brightYellow: ansi[11],
    brightBlue: ansi[12],
    brightMagenta: ansi[13],
    brightCyan: ansi[14],
    brightWhite: ansi[15],
  };
}

const SOLARIZED_ANSI = [
  "#073642",
  "#dc322f",
  "#859900",
  "#b58900",
  "#268bd2",
  "#d33682",
  "#2aa198",
  "#eee8d5",
  "#002b36",
  "#cb4b16",
  "#586e75",
  "#657b83",
  "#839496",
  "#6c71c4",
  "#93a1a1",
  "#fdf6e3",
];

/** Every scheme but `default`. */
export const SCHEME_THEMES: Record<Exclude<ColorScheme, "default">, ITheme> = {
  "solarized-dark": scheme(
    ["#002b36", "#839496", "#93a1a1", "#073642"],
    SOLARIZED_ANSI,
  ),
  "solarized-light": scheme(
    ["#fdf6e3", "#657b83", "#586e75", "#eee8d5"],
    SOLARIZED_ANSI,
  ),
  dracula: scheme(
    ["#282a36", "#f8f8f2", "#f8f8f2", "#44475a"],
    [
      "#21222c",
      "#ff5555",
      "#50fa7b",
      "#f1fa8c",
      "#bd93f9",
      "#ff79c6",
      "#8be9fd",
      "#f8f8f2",
      "#6272a4",
      "#ff6e6e",
      "#69ff94",
      "#ffffa5",
      "#d6acff",
      "#ff92df",
      "#a4ffff",
      "#ffffff",
    ],
  ),
  nord: scheme(
    ["#2e3440", "#d8dee9", "#d8dee9", "#434c5e"],
    [
      "#3b4252",
      "#bf616a",
      "#a3be8c",
      "#ebcb8b",
      "#81a1c1",
      "#b48ead",
      "#88c0d0",
      "#e5e9f0",
      "#4c566a",
      "#bf616a",
      "#a3be8c",
      "#ebcb8b",
      "#81a1c1",
      "#b48ead",
      "#8fbcbb",
      "#eceff4",
    ],
  ),
  "gruvbox-dark": scheme(
    ["#282828", "#ebdbb2", "#ebdbb2", "#504945"],
    [
      "#282828",
      "#cc241d",
      "#98971a",
      "#d79921",
      "#458588",
      "#b16286",
      "#689d6a",
      "#a89984",
      "#928374",
      "#fb4934",
      "#b8bb26",
      "#fabd2f",
      "#83a598",
      "#d3869b",
      "#8ec07c",
      "#ebdbb2",
    ],
  ),
};
