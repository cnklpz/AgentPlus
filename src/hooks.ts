import { type CSSProperties, type DependencyList, type RefObject, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { type AgentState, type DiffGroup, api } from "./api";
import { type Draft, opsToWrite } from "./draft";
import { useLang } from "./i18n";
import { type Flash, errText } from "./util";

/** A ref that always holds the latest `value` (for listeners that are registered once). */
export function useLatest<T>(value: T) {
  const ref = useRef(value);
  ref.current = value;
  return ref;
}

// ---------------------------------------------------------------- Esc layers

/** Open layers (dialogs, palettes) that close on Esc, in the order they opened. */
const layers: { current: () => void }[] = [];

/** A dialog or palette is open: page-level Esc handlers (closing a side panel) leave the key to it. */
export const escapeLayerOpen = () => layers.length > 0;

function onEscape(e: KeyboardEvent) {
  if (e.key !== "Escape" || e.defaultPrevented) return;
  const top = layers[layers.length - 1];
  if (!top) return;
  // Only the topmost layer closes; the ones underneath (and page-level Esc handlers) stay.
  e.preventDefault();
  e.stopImmediatePropagation();
  top.current();
}

/**
 * Esc calls `onClose` — only for the most recently opened layer, so a palette over a
 * dialog closes alone. The listener is registered once; the latest callback is read
 * from a ref, so parents that re-render (and pass a new arrow each time) don't
 * re-run anything else in the dialog. Popovers inside (menus, combo boxes) take Esc
 * before it gets here (see usePopover).
 */
export function useEscape(onClose: () => void) {
  const ref = useLatest(onClose);
  useEffect(() => {
    const layer = { current: () => ref.current() };
    if (!layers.length) document.addEventListener("keydown", onEscape);
    layers.push(layer);
    return () => {
      const i = layers.indexOf(layer);
      if (i >= 0) layers.splice(i, 1);
      if (!layers.length) document.removeEventListener("keydown", onEscape);
    };
  }, []);
}

/**
 * Esc for something on the page itself (a side panel, the settings page): only when no
 * dialog, palette or popover has taken the key. `skipInputs`: not while typing in a field
 * (the field may use Esc itself).
 */
export function usePageEscape(onEscape: () => void, active = true, opts: { skipInputs?: boolean } = {}) {
  const ref = useLatest(onEscape);
  const skipInputs = !!opts.skipInputs;
  useEffect(() => {
    if (!active) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || e.defaultPrevented || escapeLayerOpen()) return;
      if (skipInputs && e.target instanceof Element && e.target.closest("input, textarea, select")) return;
      ref.current();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [active, skipInputs]);
}

/**
 * A panel that closes on a mouse press outside the elements matching `keep` (a selector:
 * the panel, the cards that open it, dialogs and toasts), or on a page-level Esc.
 */
export function useDismiss(active: boolean, keep: string, onClose: () => void) {
  const ref = useLatest(onClose);
  useEffect(() => {
    if (!active) return;
    const onDown = (e: MouseEvent) => {
      if (!(e.target instanceof Element && e.target.closest(keep))) ref.current();
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [active, keep]);
  usePageEscape(onClose, active);
}

// ---------------------------------------------------------------- popovers

/**
 * Closes an open popover (menu, suggestion list): on a mouse press outside `inside`, and on
 * Esc — which stops there, so the dialog or page underneath keeps its own Esc. Optionally
 * also when something outside it scrolls, the window is resized or loses focus (a menu
 * placed with fixed coordinates would be left behind).
 */
export function usePopover(
  open: boolean,
  close: () => void,
  inside: RefObject<Element>[],
  opts: { scroll?: boolean; resize?: boolean; blur?: boolean } = {},
) {
  const closeRef = useLatest(close);
  const insideRef = useLatest(inside);
  const { scroll = false, resize = false, blur = false } = opts;
  useEffect(() => {
    if (!open) return;
    const isIn = (n: EventTarget | null) => n instanceof Node && insideRef.current.some((r) => r.current?.contains(n));
    const shut = () => closeRef.current();
    const onDown = (e: MouseEvent) => { if (!isIn(e.target)) shut(); };
    const onScroll = (e: Event) => { if (!isIn(e.target)) shut(); };
    // Window, capture phase: before any document listener (Esc layers, page handlers).
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopImmediatePropagation();
      shut();
    };
    document.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey, true);
    if (scroll) document.addEventListener("scroll", onScroll, true);
    if (resize) window.addEventListener("resize", shut);
    if (blur) window.addEventListener("blur", shut);
    return () => {
      document.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
      document.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", shut);
      window.removeEventListener("blur", shut);
    };
  }, [open, scroll, resize, blur]);
}

/**
 * Keyboard highlight for a list: ArrowUp / ArrowDown move it (clamped at the ends, or
 * wrapping around), skipping rows `enabled` rejects. A row reached by keyboard is scrolled
 * into view inside `list` (the scrolling box); `selector` finds the highlighted row there.
 */
export function useListNav(count: number, opts: { wrap?: boolean; enabled?: (i: number) => boolean; initial?: number; selector?: string } = {}) {
  const { wrap = false, enabled, initial = 0, selector = ".hi" } = opts;
  const [hi, setHi] = useState(initial);
  const list = useRef<HTMLDivElement>(null);
  const byKey = useRef(false);

  const step = (h: number, dir: 1 | -1): number => {
    if (!enabled && !wrap) return Math.max(0, Math.min(h + dir, count - 1));
    const acts = Array.from({ length: count }, (_, i) => i).filter((i) => !enabled || enabled(i));
    if (!acts.length) return -1;
    const at = acts.indexOf(h);
    if (at < 0) return dir > 0 || !wrap ? acts[0] : acts[acts.length - 1];
    return wrap ? acts[(at + dir + acts.length) % acts.length] : acts[Math.max(0, Math.min(at + dir, acts.length - 1))];
  };

  /** Handles ArrowUp / ArrowDown; true when the key was used. */
  const onKey = (e: { key: string; preventDefault(): void }): boolean => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return false;
    e.preventDefault();
    byKey.current = true;
    setHi((h) => step(h, e.key === "ArrowDown" ? 1 : -1));
    return true;
  };

  useLayoutEffect(() => {
    if (!byKey.current) return;
    byKey.current = false;
    const box = list.current;
    const el = box?.querySelector(selector);
    if (!box || !el) return;
    const b = box.getBoundingClientRect();
    const r = el.getBoundingClientRect();
    // Only the list itself scrolls: popovers close when anything around them does.
    if (r.top < b.top) box.scrollTop -= b.top - r.top;
    else if (r.bottom > b.bottom) box.scrollTop += r.bottom - b.bottom;
  }, [hi]);

  return { hi, setHi, onKey, list };
}

export interface FloatingMenu {
  /** Opens upwards (hangs from its bottom edge). */
  up: boolean;
  style: CSSProperties;
}

/**
 * Places a fixed-position menu under `anchor`, or above it when the window has no room
 * below, so scrolling containers (dialogs, side panels) never clip it; a menu wider than
 * the anchor is slid back inside the window. `estimate` is the menu's expected height.
 * `matchWidth`: exactly the anchor's width (else at least it).
 */
export function useFloatingMenu(
  anchor: RefObject<HTMLElement>,
  menu: RefObject<HTMLElement>,
  open: boolean,
  estimate: number,
  opts: { maxHeight?: number; matchWidth?: boolean } = {},
): FloatingMenu | null {
  const { maxHeight = 280, matchWidth = false } = opts;
  const [pos, setPos] = useState<{ left: number; top?: number; bottom?: number; width: number; maxHeight: number; up: boolean } | null>(null);
  const est = useLatest(estimate);

  useLayoutEffect(() => {
    if (!open || !anchor.current) {
      setPos(null);
      return;
    }
    const r = anchor.current.getBoundingClientRect();
    const want = Math.min(maxHeight, est.current);
    const below = window.innerHeight - r.bottom - 14;
    const above = r.top - 14;
    // Open on the side with room; if neither fits, the roomier side, scrolling inside.
    const up = below < want && above > below;
    const h = Math.max(80, Math.min(want, up ? above : below));
    // Upwards it hangs from its bottom edge: `estimate` is only a guess of the height, and a
    // shorter menu placed by its top would float off the anchor.
    const edge = up ? { bottom: window.innerHeight - r.top + 6 } : { top: r.bottom + 6 };
    setPos({ left: r.left, ...edge, width: r.width, maxHeight: h, up });
  }, [open]);

  // A menu wider than the anchor can run off the right edge: slide it back in.
  useLayoutEffect(() => {
    const m = menu.current?.getBoundingClientRect();
    if (!pos || !m) return;
    const left = Math.max(12, Math.min(pos.left, window.innerWidth - 12 - m.width));
    if (left !== pos.left) setPos({ ...pos, left });
  }, [pos]);

  if (!pos) return null;
  return {
    up: pos.up,
    style: {
      position: "fixed", left: pos.left, top: pos.top ?? "auto", bottom: pos.bottom ?? "auto", maxHeight: pos.maxHeight,
      ...(matchWidth ? { width: pos.width } : { minWidth: pos.width }),
    },
  };
}

// ---------------------------------------------------------------- data

/**
 * Loads data for a component and reloads it when `deps` change (the backend's text follows
 * the UI language: put `lang` in them). Only the latest request counts, so a slow older
 * answer never replaces a newer one. A failure clears `data` and sets `error`; `clear`
 * also empties `data` while a (re)load runs, so a loading placeholder shows.
 */
export function useLoad<T>(fetch: () => Promise<T>, deps: DependencyList, opts: { clear?: boolean } = {}) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<string | null>(null);
  const fetchRef = useLatest(fetch);
  const seq = useRef(0);
  const clearDefault = !!opts.clear;
  const reload = useCallback((clear = clearDefault): Promise<void> => {
    const n = ++seq.current;
    setError(null);
    if (clear) setData(null);
    return fetchRef.current().then(
      (v) => { if (n === seq.current) setData(v); },
      (e) => { if (n === seq.current) { setData(null); setError(errText(e)); } },
    );
  }, [clearDefault]);
  useEffect(() => {
    void reload();
    // Unmounted or reloading: whatever is still on its way is dropped.
    return () => { seq.current++; };
  }, deps);
  return { data, error, reload, setData };
}

/**
 * Runs one action at a time with a busy flag: a returned text is shown as a notice, an
 * error as an error notice. Resolves true when it succeeded.
 */
export function useAction(flash: Flash) {
  const [busy, setBusy] = useState(false);
  const run = async (fn: () => Promise<string | void>): Promise<boolean> => {
    setBusy(true);
    try {
      const msg = await fn();
      if (msg) flash(msg);
      return true;
    } catch (e) {
      flash(errText(e), true);
      return false;
    } finally {
      setBusy(false);
    }
  };
  return { busy, run };
}

/**
 * The diff each agent's pending changes would write, by agent id (an error text when the
 * preview failed). Re-read when the drafts change, and in the new language after a switch.
 */
export function usePreviews(agents: AgentState[], drafts: Record<string, Draft>): Record<string, DiffGroup[] | string> {
  const [diffs, setDiffs] = useState<Record<string, DiffGroup[] | string>>({});
  const lang = useLang();
  useEffect(() => {
    let alive = true;
    for (const a of agents) {
      if (!Object.keys(drafts[a.id] ?? {}).length) continue;
      api.preview(a.id, opsToWrite(a, drafts[a.id]))
        .then((d) => { if (alive) setDiffs((m) => ({ ...m, [a.id]: d })); })
        .catch((e) => { if (alive) setDiffs((m) => ({ ...m, [a.id]: errText(e) })); });
    }
    return () => { alive = false; };
  }, [agents, drafts, lang]);
  return diffs;
}
