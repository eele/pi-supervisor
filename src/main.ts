import { mount } from "svelte";
import App from "./App.svelte";
import TrayMenu from "./TrayMenu.svelte";
import "./theme.css";

// The tray-menu window loads the same bundle with #tray-menu (set by the
// Rust WebviewWindowBuilder URL); everything else is the main window.
const Target = window.location.hash.includes("tray-menu") ? TrayMenu : App;

const app = mount(Target, {
  target: document.getElementById("app") as HTMLElement,
});

export default app;
