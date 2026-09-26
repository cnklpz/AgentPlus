import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { logClient } from "./api";
import { installMotion } from "./motion";
import { applyPrefs, loadPrefs } from "./prefs";
import { isMac } from "./platform";
import "./styles.css";
import "./styles-hub.css";
import "./styles-motion.css";

// Errors nothing caught go to the diagnostic log (Settings → General).
window.addEventListener("error", (e) => logClient("error", "window", e.error ?? `${e.message} (${e.filename}:${e.lineno})`));
// A failed command rejects with a plain string, already logged with its name by `invoke`.
window.addEventListener("unhandledrejection", (e) => { if (typeof e.reason !== "string") logClient("error", "promise", e.reason); });

// Styles that differ on macOS (room for the traffic lights).
if (isMac) document.documentElement.dataset.os = "mac";
installMotion();
// Before the first paint: a dark start never flashes light, and Privacy mode masks from the start.
applyPrefs(loadPrefs());

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
