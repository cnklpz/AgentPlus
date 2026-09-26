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
    // Mid-pull the edge being pulled stays the edge, whatever the stretch does to the scroll size.
    const pulled = n === obHost && pulling && stretchDir === (down ? 1 : -1);
    const atEdge = pulled || (down ? n.scrollTop + n.clientHeight >= n.scrollHeight - 1 : n.scrollTop <= 0);
    if (!atEdge) {
      stretches = 0;
      return;
    }
    edge ??= n;
  }
  if (!edge) return;
  const px = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
  const dir = Math.sign(px);
  if (coast && obPull && obHost === edge) {
    // Momentum, or fingers slowing against the edge (they look alike): let go gradually along
    // with it rather than snapping back, so a pull that is still going just carries on.
    bumped = true;
    obPull *= 0.75;
    // Eased back almost to rest: that stretch is done, and the next push is a new one.
    if (Math.abs(obPull) < peak * 0.1) endStretch();
  } else if (coast) {
    if (bumped) return;
    bumped = true;
    settle(true);
    obHost = edge;
    // How hard the fling hit the edge sets the bump, briefly, then it springs back.
    startStretch(edge, dir);
    obPull = -dir * Math.min(Math.abs(px) * 3, 60);
    peak = Math.abs(obPull);
  } else {
    if (obHost !== edge) {
      settle(true);
      obHost = edge;
      obPull = 0;
    }
    startStretch(edge, dir);
    obPull += -px * 0.5;
    peak = Math.max(peak, Math.abs(obPull));
    tension += Math.abs(px);
    // Stretched and sprung back TEAR_AFTER times, and the fingers push hard again: it gives.
    if (stretches >= TEAR_AFTER && tension > TEAR_PX) {
      tear(edge, dir);
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
 * The pull stretches what is on screen like rubber pinned at the edge being pulled: past the
 * top, the content lengthens downward from the top (and narrows a little); past the bottom,
 * upward from the bottom. Growing away from the edge also keeps it inside what can be
 * scrolled: stretched past the bottom it would add room to scroll into. `off` is how far the
 * content moves at the far side, in px.
 */
function stretch(host: HTMLElement, off: number) {
  // A short scroller would stretch to twice its height: measure against a page-sized one, capped.
  const sy = 1 + Math.min(Math.abs(off) / Math.max(host.clientHeight, 360), 0.14);
  // The fixed line, in the host's content coordinates.
  const y = off > 0 ? host.scrollTop : host.scrollTop + host.clientHeight;
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
  // Let go by itself: one stretch done. Cut short (another edge, a tear): not counted.
  if (now) resetPull();
  else endStretch();
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
/** Stretches seen at an edge before a pull can tear it. */
const TEAR_AFTER = 2;
/** How far the fingers push into the edge (px of wheel) to tear it, after TEAR_AFTER stretches. */
const TEAR_PX = 200;
/** A pull this big shows as a stretch; smaller ones aren't counted. */
const SEEN_PULL = 20;
const MEND_PX = 600;
/** Stretches in a row at one edge, one way, each within 1.5 s of the previous one. */
let stretches = 0;
let stretchHost: HTMLElement | null = null;
let stretchDir = 0;
let stretchEndAt = 0;
/** The stretch under way: its largest pull, and how far the fingers pushed into it. */
let pulling = false;
let peak = 0;
let tension = 0;

/** A push starts a stretch; at another edge, the other way or after a pause, the count starts over. */
function startStretch(host: HTMLElement, dir: number) {
  if (pulling) return;
  if (host !== stretchHost || dir !== stretchDir || performance.now() - stretchEndAt > 1500) stretches = 0;
  stretchHost = host;
  stretchDir = dir;
  pulling = true;
  peak = 0;
  tension = 0;
}

/** The stretch sprang back (let go, or eased off with the momentum): it counts if it showed. */
function endStretch() {
  if (pulling && peak >= SEEN_PULL) {
    stretches++;
    stretchEndAt = performance.now();
  }
  resetPull();
}

function resetPull() {
  pulling = false;
  peak = 0;
  tension = 0;
}

/** One paper scrap: where it is relative to its spot in the page, and how it moves. */
interface Scrap {
  el: HTMLElement;
  x: number; y: number; r: number;
  vx: number; vy: number; vr: number;
  /** Its centre on screen at rest, half its height, where it comes to lie (Infinity: it falls
   *  out of view), its sway phase. */
  ox: number; oy: number; hb: number; rest: number; phase: number;
  landed: boolean;
  /** Where it was when gathering back began. */
  from?: { x: number; y: number; r: number };
}

let torn: {
  host: HTMLElement; dir: number;
  scraps: Scrap[];
  /** Pieces hidden while their scraps fall; cards turned see-through; parents given a position. */
  pieces: HTMLElement[]; shells: HTMLElement[]; lifted: HTMLElement[];
  /** The view the scraps fall in (walls and floor). */
  box: DOMRect;
  frame: number; t0: number;
  /** Gathering back: how far the wheel has taken it (0–1), and how far it is shown. */
  goal: number; shown: number; quick: boolean;
} | null = null;

/**
 * The pieces on screen: rows and small blocks. A card that gets split leaves an empty shell,
 * so it turns see-through while the page is torn and each scrap carries the card's surface.
 */
function shards(root: HTMLElement, box: DOMRect): { pieces: HTMLElement[]; shells: HTMLElement[] } {
  const pieces: HTMLElement[] = [];
  const shells: HTMLElement[] = [];
  const walk = (el: Element) => {
    for (const c of el.children) {
      if (!(c instanceof HTMLElement) || pieces.length >= 30 || c.classList.contains("ap-glider")) continue;
      const r = c.getBoundingClientRect();
      if (r.width === 0 || r.height === 0 || r.bottom < box.top || r.top > box.bottom) continue;
      if (r.height <= 96 || c.children.length === 0) pieces.push(c);
      else {
        shells.push(c);
        walk(c);
      }
    }
  };
  walk(root);
  return { pieces, shells };
}

const rnd = (a: number) => (Math.random() - 0.5) * 2 * a;

/**
 * Tears a w x h piece into at most `max` scraps like paper: a jittered grid whose inner seams
 * zigzag, the outer edges straight. Neighbours share their seams, so the scraps fit back
 * together exactly.
 */
export function scraps(w: number, h: number, max = 24): { pts: [number, number][]; cx: number; cy: number }[] {
  let cols = Math.min(6, Math.max(2, Math.round(w / 110)));
  let rows = Math.min(4, Math.max(1, Math.round(h / 34)));
  while (cols * rows > Math.max(2, max)) {
    if (rows > 1 && rows * 3 >= cols) rows--;
    else cols--;
  }
  const [cw, ch] = [w / cols, h / rows];
  // Grid corners: inner ones wander, border ones only slide along their border.
  const P = Array.from({ length: rows + 1 }, (_, i) => Array.from({ length: cols + 1 }, (_, j): [number, number] => [
    j === 0 || j === cols ? j * cw : j * cw + rnd(cw * 0.3),
    i === 0 || i === rows ? i * ch : i * ch + rnd(ch * 0.3),
  ]));
  // A zigzag point in the middle of each inner seam.
  const mid = (a: [number, number], b: [number, number], across: "x" | "y", amp: number): [number, number] => {
    const m: [number, number] = [(a[0] + b[0]) / 2, (a[1] + b[1]) / 2];
    m[across === "x" ? 0 : 1] += rnd(amp);
    return m;
  };
  const H = P.map((row, i) => row.slice(0, cols).map((p, j) => (i === 0 || i === rows ? null : mid(p, row[j + 1], "y", ch * 0.25))));
  const V = P.slice(0, rows).map((row, i) => row.map((p, j) => (j === 0 || j === cols ? null : mid(p, P[i + 1][j], "x", cw * 0.2))));
  const out = [];
  for (let i = 0; i < rows; i++) {
    for (let j = 0; j < cols; j++) {
      const pts = [P[i][j], H[i][j], P[i][j + 1], V[i][j + 1], P[i + 1][j + 1], H[i + 1][j], P[i + 1][j], V[i][j]].filter((p): p is [number, number] => !!p);
      const [a, b, c, d] = [P[i][j], P[i][j + 1], P[i + 1][j + 1], P[i + 1][j]];
      out.push({ pts, cx: (a[0] + b[0] + c[0] + d[0]) / 4, cy: (a[1] + b[1] + c[1] + d[1]) / 4 });
    }
  }
  return out;
}

/** Scraps in all, spread over the pieces. */
const SCRAPS = 72;
/** Paper physics, per 60 Hz frame: gravity, the slow top speed of falling paper, air drag. */
const GRAVITY = 0.28;
const FALL_MAX = 3.4;
const DRAG = 0.985;

const show = (s: Scrap) => {
  s.el.style.transform = `translate(${s.x.toFixed(1)}px, ${s.y.toFixed(1)}px) rotate(${s.r.toFixed(1)}deg)`;
};

function tear(host: HTMLElement, dir: number) {
  settle(true);
  stretches = 0;
  // The stretch may still be easing back (some blocks transition every property): stop it at
  // rest, or the pieces would be measured, and their scraps placed, where it had them.
  const stilled = [...host.children].filter((c): c is HTMLElement => c instanceof HTMLElement);
  for (const c of stilled) c.style.transition = "none";
  const box = host.getBoundingClientRect();
  const { pieces, shells } = shards(host, box);
  if (!pieces.length) {
    for (const c of stilled) c.style.removeProperty("transition");
    return;
  }
  const each = Math.max(2, Math.floor(SCRAPS / pieces.length));
  // First the cards stop clipping and the pieces' parents get a position (a card that no
  // longer clips drops any scroll it had inside), then everything is measured in that final
  // layout, then built: one layout, not one per piece.
  for (const s of shells) s.classList.add("ap-shell");
  const lifted: HTMLElement[] = [];
  for (const el of pieces) {
    const parent = el.parentElement!;
    if (!lifted.includes(parent) && getComputedStyle(parent).position === "static") {
      parent.style.position = "relative";
      lifted.push(parent);
    }
  }
  // Layout positions, not screen ones: a piece may be mid-animation (an entrance, a glide),
  // and its scraps belong where it rests.
  const plans = pieces.map((el) => {
    const parent = el.parentElement!;
    const pr = parent.getBoundingClientRect();
    let [left, top, w, h] = [el.offsetLeft, el.offsetTop, el.offsetWidth, el.offsetHeight];
    if (el.offsetParent !== parent) {
      const r = el.getBoundingClientRect();
      [left, top, w, h] = [r.left - pr.left - parent.clientLeft + parent.scrollLeft, r.top - pr.top - parent.clientTop + parent.scrollTop, r.width, r.height];
    }
    // Where it rests on screen, for the fall.
    const r = { left: pr.left + parent.clientLeft - parent.scrollLeft + left, top: pr.top + parent.clientTop - parent.scrollTop + top, width: w, height: h };
    return { el, parent, r, left, top };
  });
  const list: Scrap[] = [];
  const cx0 = box.left + box.width / 2;
  for (const p of plans) {
    const frag = document.createDocumentFragment();
    for (const bit of scraps(p.r.width, p.r.height, each)) {
      // A copy of the piece next to it (so the page's styles still reach it), showing one bit.
      const c = p.el.cloneNode(true) as HTMLElement;
      for (const n of [c, ...c.querySelectorAll("[id]")]) n.removeAttribute("id");
      c.classList.add("ap-scrap");
      c.setAttribute("aria-hidden", "true");
      c.inert = true;
      Object.assign(c.style, {
        position: "absolute", left: `${p.left}px`, top: `${p.top}px`, width: `${p.r.width}px`, height: `${p.r.height}px`,
        margin: "0", boxSizing: "border-box", transformOrigin: `${bit.cx}px ${bit.cy}px`,
        clipPath: `polygon(${bit.pts.map(([x, y]) => `${x.toFixed(1)}px ${y.toFixed(1)}px`).join(", ")})`,
      });
      frag.appendChild(c);
      const ys = bit.pts.map(([, y]) => y);
      const ox = p.r.left + bit.cx;
      const side = (ox - cx0) / (box.width / 2 || 1);
      // Torn apart: thrown out to the sides and up (harder when the pull dragged it up,
      // dir > 0), spinning; then it falls like paper.
      list.push({
        el: c, x: 0, y: 0, r: 0,
        vx: side * (2 + Math.random() * 6) + rnd(3),
        vy: -(dir > 0 ? 5 : 2) - Math.random() * 6,
        vr: rnd(9),
        ox, oy: p.r.top + bit.cy, hb: (Math.max(...ys) - Math.min(...ys)) / 2,
        rest: 0, phase: Math.random() * Math.PI * 2, landed: false,
      });
    }
    p.parent.appendChild(frag);
    // Scraps come to lie on the floor, a little heaped; ones that start down there already
    // (a row across the bottom of the view) would jump into the heap, so they fall out instead.
    for (const s of list) {
      if (s.rest) continue;
      const lie = box.bottom - 4 - Math.random() * 18 - s.hb;
      s.rest = s.oy + s.hb < lie - 8 ? lie : Infinity;
    }
    p.el.classList.add("ap-torn-away");
  }
  for (const c of stilled) c.style.removeProperty("transition");
  host.classList.add("ap-torn");
  torn = { host, dir, scraps: list, pieces, shells, lifted, box, frame: 0, t0: performance.now(), goal: 0, shown: 0, quick: false };
  torn.frame = requestAnimationFrame(tick);
  document.addEventListener("wheel", mendWheel, { passive: false, capture: true });
  document.addEventListener("keydown", mendKey, { capture: true });
}

let lastTick = 0;

/** One frame: the scraps fall (until they lie still), or gather back as far as asked. */
function tick(now: number) {
  const t = torn;
  if (!t) return;
  if (!t.host.isConnected) return mend(true);
  const dt = Math.min(3, lastTick ? (now - lastTick) / 16.7 : 1);
  lastTick = now;
  if (t.goal > 0 || t.quick) {
    // Gathering: ease the shown progress toward the wheel's, all scraps home from where they lay.
    const target = t.quick ? 1 : t.goal;
    t.shown += (target - t.shown) * Math.min(1, (t.quick ? 0.16 : 0.22) * dt);
    if (target === 1 && t.shown > 0.995) return clearTorn(t);
    const k = 1 - t.shown;
    const ease = k * k * (3 - 2 * k);
    for (const s of t.scraps) {
      s.from ??= { x: s.x, y: s.y, r: s.r };
      s.el.style.transform = `translate(${(s.from.x * ease).toFixed(1)}px, ${(s.from.y * ease).toFixed(1)}px) rotate(${(s.from.r * ease).toFixed(1)}deg)`;
    }
    // Caught up with the wheel: rest until it moves again.
    t.frame = Math.abs(target - t.shown) > 0.001 ? requestAnimationFrame(tick) : 0;
    if (!t.frame) lastTick = 0;
    return;
  }
  const { box } = t;
  let moving = false;
  for (const s of t.scraps) {
    if (s.landed) continue;
    moving = true;
    s.vy = Math.min(s.vy + GRAVITY * dt, FALL_MAX);
    s.vx *= Math.pow(DRAG, dt);
    s.vr *= Math.pow(0.99, dt);
    // Paper doesn't drop straight: it sways side to side and rocks as it falls.
    const sway = s.vy > 0 ? Math.sin((now - t.t0) * 0.005 + s.phase) : 0;
    s.x += (s.vx + sway * 1.4) * dt;
    s.y += s.vy * dt;
    s.r += (s.vr + sway * 2.2) * dt;
    const cx = s.ox + s.x;
    if (cx < box.left + 10 || cx > box.right - 10) {
      s.x = (cx < box.left + 10 ? box.left + 10 : box.right - 10) - s.ox;
      s.vx *= -0.4;
    }
    if (s.vy > 0 && s.oy + s.y >= s.rest) {
      s.y = s.rest - s.oy;
      s.landed = true;
    } else if (s.oy + s.y - s.hb > box.bottom + 40) s.landed = true; // fallen out of view
    show(s);
  }
  t.frame = moving && now - t.t0 < 8000 ? requestAnimationFrame(tick) : 0;
  if (!t.frame) lastTick = 0;
}

function mendWheel(e: WheelEvent) {
  const t = torn;
  if (!t) return;
  if (!t.host.isConnected) return mend(true);
  // Torn, the page doesn't scroll: the wheel only gathers the scraps.
  e.preventDefault();
  const px = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
  if (Math.sign(px) === t.dir || px === 0) return;
  t.goal = Math.min(1, t.goal + Math.abs(px) / MEND_PX);
  if (!t.frame) t.frame = requestAnimationFrame(tick);
}

function mendKey(e: KeyboardEvent) {
  if (e.key === "Escape" && torn) {
    e.preventDefault();
    e.stopPropagation();
    mend(false);
  }
}

/** Puts the page back: at once, or with the scraps flying home. */
function mend(now: boolean) {
  const t = torn;
  if (!t) return;
  if (now) return clearTorn(t);
  t.quick = true;
  if (!t.frame) t.frame = requestAnimationFrame(tick);
}

function clearTorn(t: NonNullable<typeof torn>) {
  if (torn === t) torn = null;
  cancelAnimationFrame(t.frame);
  lastTick = 0;
  document.removeEventListener("wheel", mendWheel, { capture: true });
  document.removeEventListener("keydown", mendKey, { capture: true });
  for (const s of t.scraps) s.el.remove();
  for (const p of t.pieces) p.classList.remove("ap-torn-away");
  for (const p of t.lifted) p.style.removeProperty("position");
  for (const s of t.shells) s.classList.remove("ap-shell");
  t.host.classList.remove("ap-torn");
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
