import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { installMotion } from "./motion";
import { setLang } from "./i18n";
import { applyTheme, loadPrefs } from "./prefs";
import "./styles.css";
import "./styles-hub.css";
import "./styles-motion.css";

installMotion();
const prefs = loadPrefs();
setLang(prefs.lang);
applyTheme(prefs.theme); // before the first paint, so a dark start never flashes light

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
