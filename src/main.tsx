import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { installMotion } from "./motion";
import { applyPrefs, loadPrefs } from "./prefs";
import "./styles.css";
import "./styles-hub.css";
import "./styles-motion.css";

installMotion();
// Before the first paint: a dark start never flashes light, and 隐私模式 masks from the start.
applyPrefs(loadPrefs());

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
