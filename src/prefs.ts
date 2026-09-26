// Per-install UI preferences (animations, auto latency, language). Browser storage may be
// unavailable, so every access is guarded and defaults always work.
import { setHints } from "./hintsFx";
import { type LangPref, setLang } from "./i18n";
import { setPrivacy } from "./privacy";

export type Motion = "rich" | "full" | "reduced" | "off";
/** How a restart shows its progress: a dialog with each step, or just a notice at the bottom. */
export type RestartProgressPref = "dialog" | "toast";
export type Theme = "auto" | "light" | "dark";
/** What closing the window does: ask each time, hide it in the tray, or quit. */
export type CloseAction = "ask" | "tray" | "quit";
/** How much explanatory text the UI shows: every description, or just the option names. */
export type Hints = "full" | "brief";

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
  closeAction: CloseAction;
  /** Privacy mode: mask keys, addresses, user folders and conversation text on screen. */
  privacy: boolean;
  /** Look for a new AgentPlus release on GitHub at startup. */
  autoUpdate: boolean;
  /** "brief" hides descriptions and hints (elements with the `hint` class). */
  hints: Hints;
}

const KEY = "agentplus.prefs";
const DEFAULTS: Prefs = { motion: "full", autoLatency: true, hiddenAgents: [], lang: "auto", theme: "auto", restartProgress: "dialog", closeAction: "ask", privacy: false, autoUpdate: true, hints: "full" };

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

let motion: Motion | null = null;
function applyMotion(m: Motion): void {
  if (motion === m) return;
  const switching = motion !== null && typeof document.getAnimations === "function";
  motion = m;
  // Each level names its own entrance animations (the page title fades at one, drops in at
  // another), and a new animation name replays the entrance on everything already on
  // screen: the title would flash. Animations this switch starts skip to their end, except
  // on elements that were about to animate anyway (text that changed with this click) and
  // looping ones (spinners, the gateway's pulse).
  const starting = switching ? new Set(document.getAnimations().filter((a) => !a.currentTime).map(target)) : null;
  document.documentElement.dataset.motion = m;
  if (!starting) return;
  for (const a of document.getAnimations()) {
    if (a instanceof CSSAnimation && !a.currentTime && a.effect?.getTiming().iterations !== Infinity && !starting.has(target(a))) a.finish();
  }
}
const target = (a: Animation) => (a.effect instanceof KeyframeEffect ? a.effect.target : null);

const systemDark = typeof matchMedia === "function" ? matchMedia("(prefers-color-scheme: dark)") : null;
let theme: Theme = "auto";
const paintTheme = () => {
  const dark = theme === "dark" || (theme === "auto" && !!systemDark?.matches);
  document.documentElement.dataset.theme = dark ? "dark" : "light";
};
// "auto" keeps following the system while the app is open.
systemDark?.addEventListener("change", () => { if (theme === "auto") paintTheme(); });

function applyTheme(t: Theme): void {
  theme = t;
  paintTheme();
}

/** Puts every preference that changes the page into effect (each one is a no-op when unchanged). */
export function applyPrefs(p: Prefs): void {
  applyMotion(p.motion);
  applyTheme(p.theme);
  // CSS hook: `.hint` elements (descriptions, explanations) are hidden in brief mode.
  setHints(p.hints);
  void setLang(p.lang);
  setPrivacy(p.privacy);
}
