// Per-install UI preferences (animations, auto latency, language). Browser storage may be
// unavailable, so every access is guarded and defaults always work.
import type { LangPref } from "./i18n";

export type Motion = "rich" | "full" | "reduced" | "off";
/** How a restart shows its progress: a dialog with each step, or just a notice at the bottom. */
export type RestartProgressPref = "dialog" | "toast";
export type Theme = "auto" | "light" | "dark";

export interface Prefs {
  motion: Motion;
  autoLatency: boolean;
  /** Detected agents the user chose not to show in the sidebar and search. */
  hiddenAgents: string[];
  /** UI language; "auto" follows the system. */
  lang: LangPref;
  /** Light or dark UI; "auto" follows the system. */
  theme: Theme;
  restartProgress: RestartProgressPref;
}

const KEY = "agentplus.prefs";
const DEFAULTS: Prefs = { motion: "full", autoLatency: true, hiddenAgents: [], lang: "auto", theme: "auto", restartProgress: "dialog" };

export function loadPrefs(): Prefs {
  try {
    const raw = localStorage.getItem(KEY);
    return raw ? { ...DEFAULTS, ...JSON.parse(raw) } : DEFAULTS;
  } catch {
    return DEFAULTS;
  }
}

export function savePrefs(p: Prefs): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(p));
  } catch {
    // Not persisted; the choice still applies for this session.
  }
}

export function applyMotion(m: Motion): void {
  document.documentElement.dataset.motion = m;
}

const systemDark = typeof matchMedia === "function" ? matchMedia("(prefers-color-scheme: dark)") : null;
let theme: Theme = "auto";
const paintTheme = () => {
  const dark = theme === "dark" || (theme === "auto" && !!systemDark?.matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
};
// "auto" keeps following the system while the app is open.
systemDark?.addEventListener("change", () => { if (theme === "auto") paintTheme(); });

export function applyTheme(t: Theme): void {
  theme = t;
  paintTheme();
}
