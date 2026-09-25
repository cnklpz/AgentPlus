// What differs between Windows and macOS in the UI: caption buttons, shortcut labels,
// the file manager's name, and whether WSL environments exist.
import { t } from "./i18n";

/** Running on macOS (native traffic lights, ⌘ shortcuts, Finder, no WSL). */
export const isMac = typeof navigator !== "undefined" && /Mac/i.test(navigator.platform || navigator.userAgent);

/** A Ctrl (⌘ on macOS) shortcut as the system writes it: "Ctrl K" / "⌘K", with Shift
 * "Ctrl+Shift+H" / "⇧⌘H". `sep` joins the Windows parts. */
export function shortcut(key: string, shift = false, sep = " "): string {
  if (isMac) return `${shift ? "⇧" : ""}⌘${key}`;
  return ["Ctrl", ...(shift ? ["Shift"] : []), key].join(sep);
}

/** Label of this machine's environment before the backend has named it. */
export function localEnvLabel(): string {
  return t(isMac ? "common.localMac" : "common.localWindows");
}
