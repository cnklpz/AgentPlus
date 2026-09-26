// App self-update (GitHub Releases, see src-tauri/src/update.rs). One shared state, so the
// startup check in App and the 设置 › 关于 section show the same thing.
import { api, type UpdateInfo } from "./api";
import { createStore } from "./store";
import { errText } from "./util";

export type UpdateState =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "latest" }
  | { kind: "available"; info: UpdateInfo }
  | { kind: "downloading"; info: UpdateInfo; done: number; total: number | null }
  | { kind: "installing"; info: UpdateInfo }
  /** `info` is kept when an install failed, so it can be retried. */
  | { kind: "error"; error: string; info: UpdateInfo | null };

/** Where each release is published (release notes, manual download). */
export const RELEASES_URL = "https://github.com/cnklpz/AgentPlus/releases";

let state: UpdateState = { kind: "idle" };
const store = createStore(() => state);
const set = (s: UpdateState) => {
  state = s;
  store.notify();
};

export const updateState = () => state;

export function useUpdate(): UpdateState {
  return store.use();
}

/** A check, download or install is running. */
export const updateBusy = (s: UpdateState) => s.kind === "checking" || s.kind === "downloading" || s.kind === "installing";

/**
 * Asks GitHub for a newer release. `quiet` (the startup check) leaves a failure as "idle":
 * being offline at launch is not worth an error in 设置.
 */
export async function checkUpdate(quiet = false): Promise<UpdateInfo | null> {
  if (updateBusy(state)) return null;
  set({ kind: "checking" });
  try {
    const info = await api.updateCheck();
    set(info ? { kind: "available", info } : { kind: "latest" });
    return info;
  } catch (e) {
    set(quiet ? { kind: "idle" } : { kind: "error", error: errText(e), info: null });
    return null;
  }
}

/** The version an install was started for, so the restarted app can say it is done. */
const INSTALLED_KEY = "agentplus.updatingTo";
const bareVersion = (v: string) => v.trim().replace(/^v/i, "");

function rememberInstall(version: string | null): void {
  try {
    if (version) localStorage.setItem(INSTALLED_KEY, version);
    else localStorage.removeItem(INSTALLED_KEY);
  } catch {
    // Only the "updated" notice after the restart is lost.
  }
}

/**
 * Called once at startup with the running version: the version an update was just installed
 * to, or null. The mark is cleared either way, so a cancelled installer (the old version starts
 * again) says nothing, and the notice shows only once.
 */
export function takeJustUpdated(current: string): string | null {
  let to: string | null = null;
  try {
    to = localStorage.getItem(INSTALLED_KEY);
    if (to !== null) localStorage.removeItem(INSTALLED_KEY);
  } catch {
    return null;
  }
  return to && bareVersion(to) === bareVersion(current) ? to : null;
}

/** Downloads and installs the update found by the last check. On success the app restarts. */
export async function installUpdate(): Promise<void> {
  const info = state.kind === "available" || state.kind === "error" ? state.info : null;
  if (!info) return;
  set({ kind: "downloading", info, done: 0, total: null });
  // Written before the install: on Windows the installer ends this process without returning.
  rememberInstall(info.version);
  try {
    await api.updateInstall((p) => {
      set(p.kind === "download" ? { kind: "downloading", info, done: p.done, total: p.total } : { kind: "installing", info });
    });
  } catch (e) {
    rememberInstall(null);
    set({ kind: "error", error: errText(e), info });
  }
}

/** Test hook: back to the initial state. */
export function resetUpdate() {
  set({ kind: "idle" });
}
