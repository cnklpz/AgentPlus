// Drag to reorder a vertical list (the sidebar's agents). Pointer events rather than HTML5
// drag and drop: the row itself follows the pointer instead of a drag image, the others
// slide out of its way, and on the "Excessive" motion level the held row turns into a
// slab of liquid glass (glass.ts). Styles are on [data-sorting] / [data-sort] in styles.css.

import { glass } from "./glass";
import { dropIndex } from "./order";

/** Pointer travel (px) before a press becomes a drag rather than a click. */
const THRESHOLD = 5;
let busy = false;

type Fx = "rich" | "full" | "none";

function fx(): Fx {
  const m = document.documentElement.dataset.motion;
  if (m === "off" || m === "reduced" || matchMedia("(prefers-reduced-motion: reduce)").matches) return "none";
  return m === "rich" ? "rich" : "full";
}

const SETTLE_MS: Record<Fx, number> = { rich: 420, full: 200, none: 0 };
/** How long the glass takes to melt back into the row once it has landed. */
const MELT_MS = 450;
const EASE: Record<Fx, string> = { rich: "var(--ease-spring)", full: "var(--ease-out)", none: "linear" };

/** Past the first or last slot the row follows only a little, like a rubber band. */
const band = (over: number) => 26 * (1 - Math.exp(-over / 60));

/**
 * Rows React moves in the DOM would replay their entrance animation. The mark stays on these
 * elements (clearing it would replay the animation too); rows added later still animate in.
 */
export function markReordered(rows: Iterable<HTMLElement>): void {
  for (const r of rows) r.dataset.reordered = "";
}

/** The click that ends a drag (the press and release land on the same row) is not a click. */
function eatClick() {
  const eat = (e: MouseEvent) => {
    e.stopPropagation();
    e.preventDefault();
  };
  window.addEventListener("click", eat, { capture: true, once: true });
  window.setTimeout(() => window.removeEventListener("click", eat, { capture: true }), 0);
}

/**
 * Starts watching a press on `row`, one of its parent's children matching `selector`. If it
 * moves far enough it becomes a drag; on release `onDrop(from, to)` must put the new order in
 * the DOM synchronously (flushSync), since the rows' offsets are cleared right after.
 */
export function dragSort(e: PointerEvent, row: HTMLElement, selector: string, onDrop: (from: number, to: number) => void): void {
  const host = row.parentElement;
  if (busy || e.button !== 0 || !host) return;
  const rows = [...host.querySelectorAll<HTMLElement>(`:scope > ${selector}`)];
  const from = rows.indexOf(row);
  if (rows.length < 2 || from < 0) return;
  const pointer = e.pointerId;
  const [y0, s0] = [e.clientY, host.scrollTop];
  const level = fx();
  let on = false;
  let to = from;
  let tops: number[] = [];
  let hs: number[] = [];
  let mids: number[] = [];
  /** How far a row moves to let the dragged one by: its height plus the gap. */
  let step = 0;
  let unglass: ((ms?: number) => void) | null = null;

  const start = () => {
    on = true;
    busy = true;
    tops = rows.map((r) => r.offsetTop);
    hs = rows.map((r) => r.offsetHeight);
    mids = tops.map((t, i) => t + hs[i] / 2);
    const [a, b] = from + 1 < rows.length ? [from, from + 1] : [from - 1, from];
    step = hs[from] + (tops[b] - tops[a] - hs[a]);
    host.dataset.sorting = "";
    const slide = level === "none" ? "none" : `transform ${SETTLE_MS[level]}ms ${EASE[level]}`;
    for (const r of rows) {
      r.dataset.sort = r === row ? "lift" : "shift";
      // Inline: the rich level stacks every sidebar child at z-index 1 (for its gliders), and
      // the held row must be above the rows it passes, or it can't refract them.
      if (r === row) r.style.zIndex = "3";
      if (r !== row) r.style.transition = slide;
    }
    // The held row follows the pointer exactly; its look eases in.
    row.style.transition = level === "none" ? "none" : "background-color .2s, border-color .2s, box-shadow .2s";
    try { row.setPointerCapture(pointer); } catch { /* released already */ }
    if (level === "rich") unglass = glass(row);
  };

  /** The other rows make room at `to`. */
  const layout = () => {
    rows.forEach((r, i) => {
      if (r === row) return;
      const shift = from < i && i <= to ? -step : to <= i && i < from ? step : 0;
      r.style.transform = shift ? `translateY(${shift}px)` : "";
    });
  };

  const move = (ev: PointerEvent) => {
    if (ev.pointerId !== pointer) return;
    const raw = ev.clientY - y0 + host.scrollTop - s0;
    if (!on) {
      if (Math.abs(raw) < THRESHOLD) return;
      start();
    }
    const n = rows.length;
    const lo = tops[0] - tops[from];
    const hi = tops[n - 1] + hs[n - 1] - tops[from] - hs[from];
    const dy = raw < lo ? lo - band(lo - raw) : raw > hi ? hi + band(raw - hi) : raw;
    row.style.transform = `translateY(${dy.toFixed(1)}px) scale(1.02)`;
    const next = dropIndex(mids, from, mids[from] + dy);
    if (next !== to) {
      to = next;
      layout();
    }
  };

  const stop = () => {
    window.removeEventListener("pointermove", move);
    window.removeEventListener("pointerup", up);
    window.removeEventListener("pointercancel", cancel);
    window.removeEventListener("lostpointercapture", cancel);
    window.removeEventListener("keydown", key, { capture: true });
  };

  /** Glides the row into slot `target` (its own slot: cancelled), then commits. */
  const settle = (target: number) => {
    stop();
    if (!on) return;
    eatClick();
    to = target;
    layout();
    const off = target > from ? tops[target] + hs[target] - tops[from] - hs[from] : tops[target] - tops[from];
    const ms = SETTLE_MS[level];
    row.style.transition = ms ? `transform ${ms}ms ${EASE[level]}, box-shadow ${ms}ms` : "none";
    row.style.transform = `translateY(${off}px)`;
    window.setTimeout(() => finish(target), ms);
  };

  const finish = (target: number) => {
    if (target !== from) markReordered(rows);
    try {
      if (target !== from) onDrop(from, target);
    } finally {
      for (const r of rows) {
        r.style.transition = "none";
        r.style.removeProperty("transform");
        r.style.removeProperty("z-index");
        delete r.dataset.sort;
      }
      delete host.dataset.sorting;
      // Landed: the glass melts back into the row rather than vanishing.
      unglass?.(MELT_MS);
      void host.offsetWidth;
      for (const r of rows) r.style.removeProperty("transition");
      busy = false;
    }
  };

  const up = (ev: PointerEvent) => { if (ev.pointerId === pointer) settle(to); };
  const cancel = (ev: PointerEvent) => { if (ev.pointerId === pointer) settle(from); };
  const key = (ev: KeyboardEvent) => {
    if (ev.key !== "Escape" || !on) return;
    ev.preventDefault();
    ev.stopPropagation();
    settle(from);
  };

  // On the window: until the drag starts the pointer isn't captured and may leave the row.
  window.addEventListener("pointermove", move);
  window.addEventListener("pointerup", up);
  window.addEventListener("pointercancel", cancel);
  window.addEventListener("lostpointercapture", cancel);
  window.addEventListener("keydown", key, { capture: true });
}
