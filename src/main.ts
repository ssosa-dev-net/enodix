import { mount } from "svelte";
// The interface typeface, bundled (no font is fetched at run time).
import "@fontsource-variable/inter/wght.css";
import "@xterm/xterm/css/xterm.css";
import "./app.css";
import App from "./App.svelte";
import Detached from "./Detached.svelte";
import { detachedRun } from "./lib/windows";

const target = document.getElementById("app")!;
// A detached run window (#77) shows that run alone; the main window, all.
const run = detachedRun();
export default run === null
  ? mount(App, { target })
  : mount(Detached, { target, props: { run } });
