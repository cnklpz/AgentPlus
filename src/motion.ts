// The "Excessive" motion level: effects that need pointer or DOM state, which CSS alone
// can't do. Installed once at startup; every handler is a no-op unless
// <html data-motion="rich"> and the system isn't asking for reduced motion.
// Styles for the classes used here live in styles-motion.css.

import { type WheelTrack, isMomentum } from "./wheel";

const SPRING =
  "linear(0, 0.103, 0.331, 0.589, 0.815, 0.98, 1.079, 1.122, 1.123, 1.101, 1.069, 1.038, 1.012, 0.996, 0.987, 0.984, 0.986, 0.989, 0.993, 0.997, 1, 1.001, 1.002, 1.002, 1.002, 1.001, 1.001, 1, 1)";
const reduce = window.matchMedia("(prefers-reduced-motion: reduce)");
const rich = () => document.documentElement.dataset.motion === "rich" && !reduce.matches;

// ---------------------------------------------------------------- Click ripple
const RIPPLE = [
  ".btn", ".icon-btn", ".pbtn", ".seg > button", ".tab", ".agent-row", ".side-link", ".rail-item", ".dd-item", ".ctx-item",
  ".palette-item", ".opt", ".chip", ".env-card", ".env-btn", ".apick", ".hcard", ".pcard", ".hcard-add", ".pcard-add", ".acct",
  ".gear-btn", ".gw-foot", ".agent-tile", ".lat-btn",
].join(", ");

function ripple(e: PointerEvent) {
  if (e.button !== 0) return;
  const el = (e.target as Element).closest<HTMLElement>(RIPPLE);
  if (!el || el.matches(":disabled")) return;
  const r = el.getBoundingClientRect();
  const x = e.clientX - r.left, y = e.clientY - r.top;
  const size = 2 * Math.hypot(Math.max(x, r.width - x), Math.max(y, r.height - y));
  const host = document.createElement("span");
  host.className = "ap-ripple-host";
  host.setAttribute("aria-hidden", "true");
  const dot = document.createElement("span");
  dot.className = "ap-ripple";
  Object.assign(dot.style, { width: `${size}px`, height: `${size}px`, left: `${x - size / 2}px`, top: `${y - size / 2}px` });
  host.appendChild(dot);
  // Positioned only while the ripple runs, so layouts that rely on static children stay intact.
  const restore = getComputedStyle(el).position === "static" ? el.style.position : null;
  if (restore !== null) el.style.position = "relative";
  // First child, so `:last-child` rules (like the env button's chevron) keep matching.
  el.insertBefore(host, el.firstChild);
  window.setTimeout(() => {
    host.remove();
    if (restore !== null && !el.querySelector(":scope > .ap-ripple-host")) el.style.position = restore;
  }, 700);
}

// ---------------------------------------------------------------- Overscroll bounce
function overflowing(el: HTMLElement) {
  const oy = getComputedStyle(el).overflowY;
  return (oy === "auto" || oy === "scroll") && el.scrollHeight > el.clientHeight + 1;
}

let obHost: HTMLElement | null = null;
let obPull = 0;
let obRelease = 0;
let obClear = 0;
// Trackpads keep sending wheel events after the fingers lift (momentum), each smaller than
// the last, for a second or more. Fed into the pull they would hold it at the edge until
// the inertia dies out, so once the stream is decaying the pull lets go and the rest of it
// is ignored.
// A fling that runs into the edge still gets one short bump, as with a native scroller.
let wheelAt = 0;
let wheelLast = 0;
let decaying = 0;
let coasting = false;
let bumped = false;

function momentum(e: WheelEvent): boolean {
  const s: WheelTrack = { at: wheelAt, last: wheelLast, decaying, coasting };
  const was = coasting;
  isMomentum(s, e.timeStamp, Math.abs(e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY));
  ({ at: wheelAt, last: wheelLast, decaying, coasting } = s);
  if (!coasting || !was) bumped = false;
  return coasting;
}

function rubberBand(e: WheelEvent) {
  if (torn || e.ctrlKey || Math.abs(e.deltaY) < Math.abs(e.deltaX) || e.deltaY === 0) return;
  const coast = momentum(e);
  const down = e.deltaY > 0;
  // The browser chains a wheel to the next ancestor that can still scroll; only when
  // none can does the innermost scroller at its edge stretch.
  let edge: HTMLElement | null = null;
  for (let n = e.target as HTMLElement | null; n && n !== document.body; n = n.parentElement) {
    if (!overflowing(n)) continue;
    const atEdge = down ? n.scrollTop + n.clientHeight >= n.scrollHeight - 1 : n.scrollTop <= 0;
    if (!atEdge) {
      streak = 0;
      return;
    }
    edge ??= n;
  }
  if (!edge) return;
  const px = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
  // One gesture is wheel events at the edge without a pause; bounces are counted per gesture,
  // so a pull that slows down or stutters never counts twice.
  const now = performance.now();
  const newGesture = now - edgeWheelAt > 200;
  const sinceLast = now - edgeWheelAt;
  edgeWheelAt = now;
  if (coast && obPull && obHost === edge) {
    // Momentum, or fingers slowing against the edge (they look alike): let go gradually along
    // with it rather than snapping back, so a pull that is still going just carries on.
    bumped = true;
    obPull *= 0.75;
  } else if (coast) {
    if (bumped) return;
    bumped = true;
    settle(true);
    obHost = edge;
    // How hard the fling hit the edge sets the bump, briefly, then it springs back.
    obPull = -Math.sign(px) * Math.min(Math.abs(px) * 3, 60);
  } else {
    if (obHost !== edge) {
      settle(true);
      obHost = edge;
      obPull = 0;
    }
    if (newGesture) countBounce(edge, Math.sign(px), sinceLast);
    obPull += -px * 0.5;
    // Bounced off this edge again and again, and still pulling: it gives.
    if (streak >= TEAR_AFTER && Math.abs(obPull) > 160) {
      tear(edge, Math.sign(px));
      return;
    }
  }
  const max = 56;
  const off = Math.sign(obPull) * max * (1 - Math.exp(-Math.abs(obPull) / (max * 1.6)));
  window.clearTimeout(obClear);
  if (!edge.classList.contains("ap-ob")) pinChildren(edge);
  edge.classList.add("ap-ob", "ap-pulling");
  stretch(edge, off);
  window.clearTimeout(obRelease);
  obRelease = window.setTimeout(() => settle(false), coast ? 90 : 160);
}

/**
 * The pull stretches what is on screen like rubber held at the far edge: pulled past the top,
 * the content lengthens down from the visible bottom (and narrows a little); past the bottom,
 * up from the visible top. `off` is how far the pulled edge moves, in px.
 */
function stretch(host: HTMLElement, off: number) {
  // A short scroller would stretch to twice its height: measure against a page-sized one, capped.
  const sy = 1 + Math.min(Math.abs(off) / Math.max(host.clientHeight, 360), 0.14);
  // The fixed line, in the host's content coordinates.
  const y = off > 0 ? host.scrollTop + host.clientHeight : host.scrollTop;
  if (off !== 0) host.style.setProperty("--ob-y", `${y.toFixed(1)}px`);
  host.style.setProperty("--ob-sy", sy.toFixed(4));
  host.style.setProperty("--ob-sx", (1 - (sy - 1) * 0.35).toFixed(4));
}

/** Each child scales about the same line: its own origin is that line minus its offset. */
function pinChildren(host: HTMLElement) {
  const top = host.getBoundingClientRect().top + host.clientTop - host.scrollTop;
  for (const c of host.children) {
    if (c instanceof HTMLElement && !c.classList.contains("ap-glider")) c.style.setProperty("--ob-t", `${(c.getBoundingClientRect().top - top).toFixed(1)}px`);
  }
}

function settle(now: boolean) {
  const host = obHost;
  if (!host) return;
  window.clearTimeout(obRelease);
  host.classList.remove("ap-pulling");
  // The spring overshoots: the stretched content squashes a touch before it comes to rest.
  stretch(host, 0);
  obPull = 0;
  const clear = () => {
    host.classList.remove("ap-ob");
    for (const p of ["--ob-y", "--ob-sy", "--ob-sx"]) host.style.removeProperty(p);
    for (const c of host.children) if (c instanceof HTMLElement) c.style.removeProperty("--ob-t");
    if (obHost === host) obHost = null;
  };
  if (now) clear();
  else obClear = window.setTimeout(clear, 650);
}

// ---------------------------------------------------------------- Torn page
// Pulled past the same edge several times in a row and still pulling, the page tears: the
// blocks on screen fly apart, away from that edge. Scrolling the other way gathers them back
// as far as it scrolls; Esc puts them back at once.
const TEAR_AFTER = 4;
const MEND_PX = 600;
let streak = 0;
let streakHost: HTMLElement | null = null;
let streakDir = 0;
/** When the last wheel event at a scroll edge arrived. */
let edgeWheelAt = 0;

interface Shard { el: HTMLElement; x: number; y: number; r: number }
let torn: { host: HTMLElement; dir: number; shards: Shard[]; back: number } | null = null;

/** A new gesture against `host`'s edge; `gap` is the pause since the previous one. */
function countBounce(host: HTMLElement, dir: number, gap: number) {
  const again = host === streakHost && dir === streakDir && gap < 1500;
  streak = again ? streak + 1 : 1;
  streakHost = host;
  streakDir = dir;
}

/** The blocks on screen: whole cards fly as one; only one taller than most of the view splits into its children. */
function shards(root: HTMLElement, box: DOMRect): HTMLElement[] {
  const out: HTMLElement[] = [];
  const walk = (el: Element) => {
    for (const c of el.children) {
      if (!(c instanceof HTMLElement) || out.length >= 48 || c.classList.contains("ap-glider")) continue;
      const r = c.getBoundingClientRect();
      if (r.width === 0 || r.height === 0 || r.bottom < box.top || r.top > box.bottom) continue;
      if (r.height <= box.height * 0.6 || c.children.length === 0) out.push(c);
      else walk(c);
    }
  };
  walk(root);
  return out;
}

const place = (s: Shard, k: number, how: string) => {
  s.el.style.transition = how;
  s.el.style.translate = `${(s.x * k).toFixed(1)}px ${(s.y * k).toFixed(1)}px`;
  s.el.style.rotate = `${(s.r * k).toFixed(2)}deg`;
};

function tear(host: HTMLElement, dir: number) {
  settle(true);
  streak = 0;
  const box = host.getBoundingClientRect();
  const list = shards(host, box).map((el): Shard => {
    const r = el.getBoundingClientRect();
    const side = (r.left + r.width / 2 - (box.left + box.width / 2)) / (box.width / 2 || 1);
    // Near the pulled edge they fly furthest. Past the top (dir < 0) the page is dragged down.
    const near = dir < 0 ? 1 - (r.top - box.top) / box.height : (r.bottom - box.top) / box.height;
    const y = -dir * (70 + Math.max(0, near) * 260 + Math.random() * 90);
    const x = side * (50 + Math.random() * 130) + (Math.random() - 0.5) * 90;
    return { el, x, y, r: (Math.random() - 0.5) * 50 };
  });
  torn = { host, dir, shards: list, back: 0 };
  host.classList.add("ap-torn");
  for (const s of list) place(s, 1, `translate .7s ${SPRING}, rotate .7s ${SPRING}`);
  document.addEventListener("wheel", mendWheel, { passive: false, capture: true });
  document.addEventListener("keydown", mendKey, { capture: true });
}

function mendWheel(e: WheelEvent) {
  const t = torn;
  if (!t) return;
  if (!t.host.isConnected) return mend(true);
  // Torn, the page doesn't scroll: the wheel only gathers the pieces.
  e.preventDefault();
  const px = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
  if (Math.sign(px) === t.dir || px === 0) return;
  t.back += Math.abs(px);
  const left = 1 - Math.min(1, t.back / MEND_PX);
  if (left === 0) return mend(false);
  for (const s of t.shards) place(s, left, "translate .16s ease-out, rotate .16s ease-out");
}

function mendKey(e: KeyboardEvent) {
  if (e.key === "Escape" && torn) {
    e.preventDefault();
    e.stopPropagation();
    mend(false);
  }
}

function mend(now: boolean) {
  const t = torn;
  if (!t) return;
  torn = null;
  document.removeEventListener("wheel", mendWheel, { capture: true });
  document.removeEventListener("keydown", mendKey, { capture: true });
  const clear = () => {
    for (const s of t.shards) for (const p of ["transition", "translate", "rotate"]) s.el.style.removeProperty(p);
    t.host.classList.remove("ap-torn");
  };
  if (now) return clear();
  for (const s of t.shards) place(s, 0, `translate .55s ${SPRING}, rotate .55s ${SPRING}`);
  window.setTimeout(clear, 600);
}

// ---------------------------------------------------------------- Edges: keyboard runs out, a disabled item is clicked
const HI = ".palette-list .palette-item.on, .dd-menu .dd-item.hi, .ctx-menu .ctx-item.hi";

function keyEdge(e: KeyboardEvent) {
  if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
  const before = document.querySelector<HTMLElement>(HI);
  if (!before) return;
  const dir = e.key === "ArrowUp" ? -1 : 1;
  requestAnimationFrame(() => {
    if (document.querySelector(HI) !== before) return;
    before.animate([{ translate: "0 0" }, { translate: `0 ${dir * 5}px`, offset: 0.3 }, { translate: "0 0" }], { duration: 420, easing: SPRING });
  });
}

function nope(e: PointerEvent) {
  const el = (e.target as Element).closest<HTMLElement>("button:disabled, .disabled, [aria-disabled='true']");
  if (!el) return;
  el.animate([{ translate: "0" }, { translate: "-5px" }, { translate: "4px" }, { translate: "-3px" }, { translate: "2px" }, { translate: "0" }], { duration: 380, easing: "ease-out" });
}

// ---------------------------------------------------------------- Card tilt
let tiltEl: HTMLElement | null = null;
let tiltFrame = 0;

function tilt(e: PointerEvent) {
  const el = (e.target as Element).closest<HTMLElement>(".hcard, .pcard");
  if (tiltEl && tiltEl !== el) {
    tiltEl.style.removeProperty("--mx");
    tiltEl.style.removeProperty("--my");
  }
  tiltEl = el;
  if (!el || tiltFrame) return;
  const { clientX, clientY } = e;
  tiltFrame = requestAnimationFrame(() => {
    tiltFrame = 0;
    const r = el.getBoundingClientRect();
    el.style.setProperty("--mx", ((clientX - r.left) / r.width).toFixed(3));
    el.style.setProperty("--my", ((clientY - r.top) / r.height).toFixed(3));
  });
}

// ---------------------------------------------------------------- Sliding selection highlight
const GLIDE: { item: string; cls: string }[] = [
  { item: ".seg > button", cls: "on" },
  { item: ".sidebar > .agent-row", cls: "active" },
];
const gliding = new WeakMap<HTMLElement, () => void>();

function hadClass(old: string | null, cls: string) {
  return (old ?? "").split(/\s+/).includes(cls);
}

function glide(records: MutationRecord[]) {
  for (const { item, cls } of GLIDE) {
    const gained = records.find((r) => r.target instanceof HTMLElement && r.target.matches(item) && r.target.classList.contains(cls) && !hadClass(r.oldValue, cls));
    if (!gained) continue;
    const next = gained.target as HTMLElement;
    const prev = records.find((r) => r.target !== next && r.target instanceof HTMLElement && r.target.parentElement === next.parentElement && hadClass(r.oldValue, cls) && !r.target.classList.contains(cls))?.target as HTMLElement | undefined;
    const box = next.parentElement;
    if (!prev || !box) continue;
    gliding.get(box)?.();
    const g = document.createElement("span");
    g.className = "ap-glider";
    g.setAttribute("aria-hidden", "true");
    const from = { x: prev.offsetLeft, y: prev.offsetTop, w: prev.offsetWidth, h: prev.offsetHeight };
    const to = { x: next.offsetLeft, y: next.offsetTop, w: next.offsetWidth, h: next.offsetHeight };
    Object.assign(g.style, { width: `${to.w}px`, height: `${to.h}px`, transform: `translate(${to.x}px, ${to.y}px)` });
    box.insertBefore(g, box.firstChild);
    next.classList.add("ap-glide-to");
    const anim = g.animate(
      [
        { width: `${from.w}px`, height: `${from.h}px`, transform: `translate(${from.x}px, ${from.y}px)` },
        { width: `${to.w}px`, height: `${to.h}px`, transform: `translate(${to.x}px, ${to.y}px)` },
      ],
      { duration: 480, easing: SPRING },
    );
    const done = () => {
      gliding.delete(box);
      // Drop the override with transitions off so the real highlight appears exactly where the glider stopped.
      next.style.transition = "none";
      next.classList.remove("ap-glide-to");
      void next.offsetWidth;
      next.style.removeProperty("transition");
      g.remove();
    };
    // A timer, not anim.finished: a hidden or minimized window stops ticking animations,
    // and the real highlight must come back regardless.
    const timer = window.setTimeout(() => { anim.cancel(); done(); }, 520);
    gliding.set(box, () => { window.clearTimeout(timer); anim.cancel(); done(); });
  }
}

// ---------------------------------------------------------------- Exits
const EXIT = ".modal-bg, .toast, .dd-menu, .ctx-menu";
const EXIT_MS = 320;

function exits(records: MutationRecord[]) {
  for (const r of records) {
    for (const n of r.removedNodes) {
      if (!(n instanceof HTMLElement) || !n.matches(EXIT) || n.classList.contains("ap-ghost")) continue;
      const parent = r.target;
      if (!parent.isConnected) continue;
      const ghost = n.cloneNode(true) as HTMLElement;
      ghost.classList.add("ap-ghost");
      ghost.setAttribute("aria-hidden", "true");
      ghost.inert = true;
      ghost.removeAttribute("id");
      ghost.querySelectorAll("[id]").forEach((x) => x.removeAttribute("id"));
      const before = r.nextSibling && r.nextSibling.parentNode === parent ? r.nextSibling : null;
      parent.insertBefore(ghost, before);
      window.setTimeout(() => ghost.remove(), EXIT_MS);
    }
  }
}

export function installMotion(): void {
  const on = <K extends keyof DocumentEventMap>(type: K, fn: (e: DocumentEventMap[K]) => void, opts?: AddEventListenerOptions) =>
    document.addEventListener(type, (e) => rich() && fn(e), opts);
  on("pointerdown", ripple, { capture: true, passive: true });
  on("pointerdown", nope, { capture: true, passive: true });
  on("pointermove", tilt, { passive: true });
  on("wheel", rubberBand, { passive: true });
  on("keydown", keyEdge, { capture: true });
  new MutationObserver((records) => {
    if (!rich()) return;
    glide(records.filter((r) => r.type === "attributes"));
    exits(records.filter((r) => r.type === "childList"));
  }).observe(document.body, { subtree: true, childList: true, attributes: true, attributeFilter: ["class"], attributeOldValue: true });
}
