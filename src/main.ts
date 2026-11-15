import { mount } from "svelte";
// The interface typeface, bundled (no font is fetched at run time).
import "@fontsource-variable/inter/wght.css";
import "@xterm/xterm/css/xterm.css";
import "./app.css";
import App from "./App.svelte";
import Detached from "./Detached.svelte";
import { detachedView } from "./lib/windows";

const target = document.getElementById("app")!;
// A detached window shows one run (#77) or one host's config history
// (#78) alone; the main window, all.
const view = detachedView();
export default view === null
  ? mount(App, { target })
  : mount(Detached, { target, props: { view } });
