// Small helpers shared by the components: error text, clipboard, set toggles, key handling.
import type { KeyboardEvent } from "react";
import { t } from "./i18n";

/** Shows a notice at the bottom of the window (App's toast); errors stay up longer. */
export type Flash = (text: string, error?: boolean) => void;

/**
 * Text of a caught error. Backend (Tauri) rejections are plain strings; errors thrown in the
 * frontend (`new Error(t(...))`) are Error objects, whose String() would start with "Error: ".
 */
export function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Copies `text` and says so (`ok`, default "Copied"), or that copying failed. */
export function copyText(text: string, flash: Flash, ok: string = t("common.copied")): Promise<void> {
  return navigator.clipboard.writeText(text).then(() => flash(ok)).catch(() => flash(t("common.copyFailed"), true));
}

/** A copy of `s` with `v` added, or removed when it was there. */
export function toggled<T>(s: ReadonlySet<T>, v: T): Set<T> {
  const n = new Set(s);
  if (n.has(v)) n.delete(v);
  else n.add(v);
  return n;
}

/** A copy of `a` with `v` appended, or removed when it was there. */
export function toggledIn<T>(a: readonly T[], v: T): T[] {
  return a.includes(v) ? a.filter((x) => x !== v) : [...a, v];
}

/** An http(s) address (surrounding spaces ignored). */
export function isHttpUrl(s: string): boolean {
  return /^https?:\/\/\S+$/.test(s.trim());
}

/**
 * onKeyDown for an element with role="button": Enter / Space activate it. Only the element
 * itself: on a button inside it, the key belongs to that button.
 */
export function onActivateKey(fn: () => void) {
  return (e: KeyboardEvent) => {
    if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
      e.preventDefault();
      fn();
    }
  };
}
