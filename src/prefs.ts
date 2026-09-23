// Per-install UI preferences (animations, auto latency). Browser storage may be
// unavailable, so every access is guarded and defaults always work.
export type Motion = "full" | "reduced" | "off";

export interface Prefs {
  motion: Motion;
  autoLatency: boolean;
}

const KEY = "agentplus.prefs";
const DEFAULTS: Prefs = { motion: "full", autoLatency: true };

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
