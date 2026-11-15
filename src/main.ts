import { mount } from "svelte";
// The interface typeface, bundled (no font is fetched at run time).
import "@fontsource-variable/inter/wght.css";
import "@xterm/xterm/css/xterm.css";
import "./app.css";
import App from "./App.svelte";

export default mount(App, { target: document.getElementById("app")! });
