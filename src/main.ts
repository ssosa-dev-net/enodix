import { mount } from "svelte";
// The interface typeface, bundled (no font is fetched at run time).
import "@fontsource-variable/inter/wght.css";
import "@xterm/xterm/css/xterm.css";
import "./app.css";
import App from "./App.svelte";
import Detached from "./Detached.svelte";
import SessionWindow from "./SessionWindow.svelte";
import { detachedView, isSessionWindow } from "./lib/windows";
import { noWritingSuggestions } from "./lib/writingSuggestions";

// Before anything is drawn: Edge's writing suggestions off in every field.
noWritingSuggestions();

const target = document.getElementById("app")!;
// A detached window shows one run (#77) or one host's config history
// (#78) alone, a session window one session (#84); the main window, all.
const view = detachedView();
export default isSessionWindow()
  ? mount(SessionWindow, { target })
  : view === null
    ? mount(App, { target })
    : mount(Detached, { target, props: { view } });
