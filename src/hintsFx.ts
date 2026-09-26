// Switching between detailed and brief hints (Settings → Interface). Instead of snapping,
// at the "Excessive" motion level the hints on screen shatter character by character, last
// character first, while their space folds away; or their space unfolds and they come into
// focus character by character. Lower levels fade (and fold) whole hints. Hints off screen
// just switch.
//
// The real text is never replaced or redrawn, so it keeps its exact font and rendering:
// single characters are dimmed or hidden with the CSS Custom Highlight API (ranges styled
// by `::highlight()`, no DOM changes), which also leaves React's nodes alone. Only the
// shards of a shattering character are extra elements: copies of that glyph, in the same
// font, cut into pieces that fly off on the compositor.
import type { Hints } from "./prefs";

/** Longest a character waits for its turn, and the most one hint's wave may spread over. */
const STEP_MS = 16;
const SPREAD_MS = 320;
/** Each further hint starts its wave this much later, so they cascade over the page. */
const HINT_LAG_MS = 18;
/** Folding or unfolding a hint's space. */
const FOLD_MS = 360;
/** One character coming into focus, in LEVELS steps (a highlight per step). */
const IN_MS = 420;
const LEVELS = 10;
/** Lifetime of a shard (randomized between the two). */
const SHARD_MIN_MS = 380;
const SHARD_MAX_MS = 560;
/** Shards for the whole page; fewer pieces per character when there is a lot of text. */
const MAX_SHARDS = 1800;
/** At most this many hints animate; a page with more just switches the rest. */
const MAX_HINTS = 60;
const EASE = "cubic-bezier(.22, .8, .26, 1)";
const GONE = "ap-hint-gone";

/** Delay (ms) of each of `count` characters: an even wave that never spreads past `spread`. */
export function stagger(count: number, reverse = false, step = STEP_MS, spread = SPREAD_MS): number[] {
  const gap = count > 1 ? Math.min(step, spread / (count - 1)) : 0;
  return Array.from({ length: count }, (_, i) => Math.round((reverse ? count - 1 - i : i) * gap));
}

/** Parses `rgb(r, g, b)` / `rgba(r, g, b, a)` (how computed colors read). */
export function rgba(css: string): [number, number, number, number] | null {
  const m = css.match(/rgba?\(\s*([\d.]+)[,\s]+([\d.]+)[,\s]+([\d.]+)(?:\s*[,/]\s*([\d.]+%?))?\s*\)/);
  if (!m) return null;
  const a = m[4] === undefined ? 1 : m[4].endsWith("%") ? parseFloat(m[4]) / 100 : parseFloat(m[4]);
  return [+m[1], +m[2], +m[3], a];
}

/** Style of focus step `k` of LEVELS for a character of colour `c`: from nothing, through a
 *  soft glow, to almost its own colour (the last step removes the highlight). */
export function levelStyle(k: number, c: [number, number, number, number]): string {
  const q = k / LEVELS;
  const [r, g, b, a] = c;
  const ink = (a * Math.max(0, (q - 0.2) / 0.8)).toFixed(3);
  const glow = (a * 0.55 * Math.min(1, q * 3) * (1 - q)).toFixed(3);
  const blur = (6 * (1 - q)).toFixed(1);
  return `color: rgba(${r}, ${g}, ${b}, ${ink}); text-shadow: 0 0 ${blur}px rgba(${r}, ${g}, ${b}, ${glow});`;
}

/** Polygons (px, over a w×h box) that cut a glyph into `n` shards around a point. */
export function shardPolys(w: number, h: number, n: number, rand = Math.random): [number, number][][] {
  if (n <= 1) return [[[0, 0], [w, 0], [w, h], [0, h]]];
  if (n === 2) {
    // One slanted cut through the middle.
    const t = w * (0.2 + rand() * 0.6), u = w * (0.2 + rand() * 0.6);
    return [[[0, 0], [t, 0], [u, h], [0, h]], [[t, 0], [w, 0], [w, h], [u, h]]];
  }
  const px = w * (0.3 + rand() * 0.4), py = h * (0.3 + rand() * 0.4), ax = w * (0.25 + rand() * 0.5);
  return [[[0, 0], [ax, 0], [px, py], [0, h]], [[ax, 0], [w, 0], [w, h], [px, py]], [[px, py], [w, h], [0, h]]];
}

interface Box { left: number; top: number; right: number; bottom: number }
const meet = (a: Box, b: Box): Box => ({
  left: Math.max(a.left, b.left), top: Math.max(a.top, b.top), right: Math.min(a.right, b.right), bottom: Math.min(a.bottom, b.bottom),
});
const viewport = (): Box => ({ left: 0, top: 0, right: window.innerWidth, bottom: window.innerHeight });
const inside = (r: Box, c: Box) => r.right > c.left && r.left < c.right && r.bottom > c.top && r.top < c.bottom;

interface Char { range: Range; delay: number; face: number; level: number; done: boolean }
interface Group {
  el: HTMLElement; clippers: Element[]; chars: Char[]; start: number; end: number;
  /** Where the hint is now (kept while it has no box); shards follow it as the page moves. */
  x: number; y: number;
}

const clips = (cs: CSSStyleDeclaration) => cs.overflowX !== "visible" || cs.overflowY !== "visible";

function clippersOf(el: HTMLElement): Element[] {
  const out: Element[] = [];
  for (let n = el.parentElement; n && n !== document.documentElement; n = n.parentElement) if (clips(getComputedStyle(n))) out.push(n);
  return out;
}

function clipOf(els: Element[]): Box {
  return els.reduce<Box>((b, n) => meet(b, n.getBoundingClientRect()), viewport());
}

/** How the characters of one text node look: enough to redraw a glyph on a canvas. */
interface Face { font: string; color: string; size: number; transform: string }

/** One range per visible character of `el`; `faces` collects their looks (index in `Char.face`). */
function charsOf(el: HTMLElement, faces: Face[]): Char[] {
  const out: Char[] = [];
  const walk = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
  for (let n = walk.nextNode() as Text | null; n; n = walk.nextNode() as Text | null) {
    const parent = n.parentElement;
    if (!parent || !n.data.trim()) continue;
    const cs = getComputedStyle(parent);
    const f: Face = { font: `${cs.fontStyle} ${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`, color: cs.color, size: parseFloat(cs.fontSize) || 12, transform: cs.textTransform };
    let face = faces.findIndex((x) => x.font === f.font && x.color === f.color && x.transform === f.transform);
    if (face < 0) face = faces.push(f) - 1;
    let off = 0;
    for (const ch of n.data) {
      const at = off;
      off += ch.length;
      if (/\s/.test(ch)) continue;
      const range = new Range();
      range.setStart(n, at);
      range.setEnd(n, off);
      out.push({ range, delay: 0, face, level: -1, done: false });
    }
  }
  return out;
}

/** Keyframes that take the hint's box from its natural size to nothing, or null for inline hints. */
function foldFrames(el: HTMLElement): [Keyframe, Keyframe] | null {
  // A `grow` hint is a row's spacer: brief mode keeps its space, so only the text goes.
  if (el.classList.contains("grow")) return null;
  const cs = getComputedStyle(el);
  if (cs.display === "inline" || cs.display === "contents") return null;
  const open: Keyframe = {}, shut: Keyframe = {};
  const set = (prop: string, from: string, to = "0px") => { open[prop] = from; shut[prop] = to; };
  for (const p of ["height", "paddingTop", "paddingBottom", "borderTopWidth", "borderBottomWidth", "marginTop", "marginBottom"] as const) set(p, cs[p]);
  const parent = el.parentElement ? getComputedStyle(el.parentElement) : null;
  const flex = !!parent?.display.endsWith("flex");
  const row = flex && !parent?.flexDirection.startsWith("column");
  if (row) {
    // Side by side with its siblings: fold the width too, so they slide over.
    set("maxWidth", `${el.getBoundingClientRect().width}px`);
    for (const p of ["paddingLeft", "paddingRight", "marginLeft", "marginRight"] as const) set(p, cs[p]);
  }
  // Swallow the flex gap next to the hint as well, or the space would snap shut at the end.
  const gap = (flex && parseFloat((row ? parent?.columnGap : parent?.rowGap) ?? "")) || 0;
  if (gap && (el.previousElementSibling || el.nextElementSibling)) {
    const side = el.previousElementSibling ? (row ? "marginLeft" : "marginTop") : row ? "marginRight" : "marginBottom";
    shut[side] = `${parseFloat(cs[side]) - gap}px`;
  }
  return [open, shut];
}

/** Hints on screen and not covered (by a dialog, say), in page order. */
function targets(): HTMLElement[] {
  const out: HTMLElement[] = [];
  const vp = viewport();
  for (const el of document.querySelectorAll<HTMLElement>(".hint")) {
    if (out.length >= MAX_HINTS) break;
    // Nested hints ride along with the outer one; exit clones are about to go anyway.
    if (el.parentElement?.closest(".hint, .ap-ghost")) continue;
    const r = el.getClientRects()[0];
    if (!r || !inside(r, vp)) continue;
    const x = Math.min(Math.max(r.left + r.width / 2, 1), vp.right - 1);
    const y = Math.min(Math.max(r.top + r.height / 2, 1), vp.bottom - 1);
    const hit = document.elementFromPoint(x, y);
    if (hit && (el.contains(hit) || hit.contains(el))) out.push(el);
  }
  return out;
}

// ---------------------------------------------------------------- Shards

const PAD = 2;
/** Widest atlas the shards are cut into, and tallest. */
const ATLAS_W = 2048;
const ATLAS_H = 4096;

interface Shard {
  /** Cell in the atlas; the glyph box's viewport position; the piece's centre in that box. */
  ax: number; ay: number; x: number; y: number; px: number; py: number;
  /** Its hint, and where that was when the glyph broke. */
  g: Group; gx: number; gy: number;
  dx: number; dy: number; fall: number; spin: number; t0: number; life: number;
}

/**
 * The flying shards, painted on one canvas over the page (one layer and one copy per piece
 * per frame, however many there are). A breaking character's glyph is drawn once per piece,
 * clipped, into its own atlas cell. Until it breaks, a character is the page's real text.
 */
function shardLayer(faces: Face[], pieces: number, count: number) {
  const dpr = window.devicePixelRatio || 1;
  const size = Math.max(...faces.map((f) => f.size));
  const cell = Math.ceil((size * 1.7 + PAD * 2) * dpr);
  const cols = Math.floor(ATLAS_W / cell);
  const rows = Math.min(Math.ceil((count * pieces) / Math.max(cols, 1)), Math.floor(ATLAS_H / cell));
  if (!cols || !rows) return null;
  const atlas = document.createElement("canvas");
  atlas.width = cols * cell;
  atlas.height = rows * cell;
  const canvas = document.createElement("canvas");
  const w = window.innerWidth, h = window.innerHeight;
  canvas.width = Math.ceil(w * dpr);
  canvas.height = Math.ceil(h * dpr);
  const actx = atlas.getContext("2d"), ctx = canvas.getContext("2d");
  if (!actx || !ctx) return null;
  canvas.className = "ap-shards";
  canvas.setAttribute("aria-hidden", "true");
  Object.assign(canvas.style, { width: `${w}px`, height: `${h}px` });
  document.body.appendChild(canvas);
  // Baseline of each face inside a character's box (the box is the font's content area).
  const metrics = faces.map((f) => {
    actx.font = f.font;
    const m = actx.measureText("x");
    return { asc: m.fontBoundingBoxAscent, desc: m.fontBoundingBoxDescent };
  });
  const live: Shard[] = [];
  let next = 0;

  return {
    remove: () => canvas.remove(),
    /** Breaks the character `ch` of hint `g` (laid out at `r`) at time `t`. */
    crack(t: number, ch: string, r: DOMRect, fi: number, g: Group) {
      const f = faces[fi], m = metrics[fi];
      const bw = r.width + PAD * 2, bh = r.height + PAD * 2;
      if (bw * dpr > cell || bh * dpr > cell) return;
      const text = f.transform === "uppercase" ? ch.toUpperCase() : f.transform === "lowercase" ? ch.toLowerCase() : ch;
      const base = PAD + (r.height - m.asc - m.desc) / 2 + m.asc;
      for (const poly of shardPolys(bw, bh, pieces)) {
        if (next >= cols * rows) return;
        const ax = (next % cols) * cell, ay = Math.floor(next / cols) * cell;
        next++;
        actx.save();
        actx.setTransform(dpr, 0, 0, dpr, ax, ay);
        actx.beginPath();
        poly.forEach(([x, y], i) => (i ? actx.lineTo(x, y) : actx.moveTo(x, y)));
        actx.clip();
        actx.font = f.font;
        actx.fillStyle = f.color;
        actx.fillText(text, PAD, base);
        actx.restore();
        const px = poly.reduce((a, q) => a + q[0], 0) / poly.length, py = poly.reduce((a, q) => a + q[1], 0) / poly.length;
        // A small crack apart (away from the glyph's middle, a touch to the right), then
        // the pieces drop a little, turn and fade.
        const ox = px - bw / 2, oy = py - bh / 2, len = Math.hypot(ox, oy) || 1;
        const burst = 1.5 + Math.random() * 2.5;
        live.push({
          ax, ay, x: r.left - PAD, y: r.top - PAD, px, py, g, gx: g.x, gy: g.y,
          dx: (ox / len) * burst + 0.5 + Math.random() * 2, dy: (oy / len) * burst * 0.6,
          fall: 5 + Math.random() * 5, spin: ((Math.random() - 0.5) * Math.PI) / 3,
          t0: t, life: SHARD_MIN_MS + Math.random() * (SHARD_MAX_MS - SHARD_MIN_MS),
        });
      }
    },
    paint(t: number) {
      ctx.setTransform(1, 0, 0, 1, 0, 0);
      ctx.clearRect(0, 0, canvas.width, canvas.height);
      let n = 0;
      for (const s of live) {
        const p = Math.max(0, (t - s.t0) / s.life);
        if (p >= 1) continue;
        live[n++] = s;
        const e = 1 - (1 - p) ** 3;
        const a = s.spin * e, k = 1 - 0.15 * p;
        const cos = Math.cos(a) * k, sin = Math.sin(a) * k;
        ctx.globalAlpha = 1 - p * p;
        const x = s.x + s.g.x - s.gx + s.px + s.dx * e, y = s.y + s.g.y - s.gy + s.py + s.dy * e + s.fall * p * p;
        ctx.setTransform(cos, sin, -sin, cos, x * dpr, y * dpr);
        ctx.drawImage(atlas, s.ax, s.ay, cell, cell, -s.px * dpr, -s.py * dpr, cell, cell);
      }
      live.length = n;
      ctx.globalAlpha = 1;
    },
  };
}

// ---------------------------------------------------------------- Running

type Registry = Map<string, Highlight>;
const registry = (): Registry | null => (typeof CSS !== "undefined" && "highlights" in CSS ? (CSS.highlights as unknown as Registry) : null);

const reduce = typeof matchMedia === "function" ? matchMedia("(prefers-reduced-motion: reduce)") : null;
/** How the switch animates at the current motion level: character by character only at
 *  "Excessive", whole hints fading and folding at "Standard", just fading when reduced. */
function motionStyle(): "chars" | "block" | "fade" | null {
  const m = document.documentElement.dataset.motion;
  if (m === "off") return null;
  if (m === "reduced" || reduce?.matches) return "fade";
  if (m === "rich" && registry() && typeof Highlight === "function") return "chars";
  return "block";
}

/** Whole hints fade (and, unless `fadeOnly`, fold) in or out together. */
function blocks(next: Hints, fadeOnly: boolean) {
  const root = document.documentElement;
  const out = next === "brief";
  if (!out) root.dataset.hints = next;
  const els = targets();
  const frames = fadeOnly ? els.map(() => null) : els.map(foldFrames);
  const anims: Animation[] = [];
  let total = 0;
  els.forEach((el, i) => {
    const f = frames[i];
    const fade: Keyframe[] = [{ opacity: 1 }, { opacity: 0 }];
    // Going out the text fades first and the space closes behind it; coming in, the
    // space opens first and the text fades in as it settles.
    const fadeOpts: KeyframeAnimationOptions = out
      ? { duration: 200, easing: "ease-out", fill: "both" }
      : { duration: 260, delay: f ? 140 : 0, easing: "ease-out", fill: "both" };
    anims.push(el.animate(out ? fade : [...fade].reverse(), fadeOpts));
    total = Math.max(total, (fadeOpts.delay ?? 0) + (fadeOpts.duration as number));
    if (!f) return;
    el.classList.add("ap-folding");
    const delay = out ? 90 : 0;
    anims.push(el.animate(out ? f : [f[1], f[0]], { duration: FOLD_MS, delay, easing: EASE, fill: "both" }));
    total = Math.max(total, delay + FOLD_MS);
  });
  const end = () => {
    window.clearTimeout(timer);
    if (out) root.dataset.hints = next;
    anims.forEach((a) => a.cancel());
    els.forEach((el) => el.classList.remove("ap-folding"));
    if (finish === end) finish = null;
  };
  const timer = window.setTimeout(end, total + 20);
  finish = end;
}

let shown: Hints | null = null;
/** Jumps the running transition to its end. */
let finish: (() => void) | null = null;

/** Shows hints in `next` mode, animating the switch when motion allows. */
export function setHints(next: Hints): void {
  const root = document.documentElement;
  if (shown === next) return;
  const first = shown === null;
  shown = next;
  finish?.();
  const how = first ? null : motionStyle();
  if (!how) {
    root.dataset.hints = next;
    return;
  }
  if (how !== "chars") {
    blocks(next, how === "fade");
    return;
  }
  const reg = registry()!;
  const out = next === "brief";
  // Coming in, the hints have to be laid out before they can be measured.
  if (!out) root.dataset.hints = next;
  const els = targets();
  // Read everything before the first write (animations, highlights) changes the layout.
  const frames = els.map(foldFrames);
  const faces: Face[] = [];
  const groups: Group[] = els.map((el) => ({ el, clippers: clippersOf(el), chars: charsOf(el, faces), start: 0, end: 0, x: 0, y: 0 }));
  // One wave per hint, the hints cascading: coming in top down, going out bottom up and
  // each from its last character, so the text crumbles from its end.
  const base = out ? 0 : Math.round(FOLD_MS * 0.35);
  const order = out ? [...groups].reverse() : groups;
  let last = 0;
  order.forEach((g, i) => {
    const d = stagger(g.chars.length, out);
    g.start = base + i * HINT_LAG_MS;
    g.chars.forEach((c, k) => { c.delay = g.start + d[k]; });
    g.end = g.start + (d.length ? Math.max(...d) : 0);
    last = Math.max(last, g.end);
  });

  const names: string[] = [];
  const sheet = document.createElement("style");
  const levels: Highlight[][] = [];
  const gone = new Highlight();
  if (out) {
    reg.set(GONE, gone);
    names.push(GONE);
  } else {
    // A highlight per look and step; every character starts on step 0 (invisible).
    let css = "";
    faces.forEach((f, ci) => {
      const parsed = rgba(f.color) ?? [128, 128, 128, 1];
      levels[ci] = Array.from({ length: LEVELS }, (_, k) => {
        const name = `ap-hint-in-${ci}-${k}`;
        const h = new Highlight();
        reg.set(name, h);
        names.push(name);
        css += `::highlight(${name}) { ${levelStyle(k, parsed)} }\n`;
        return h;
      });
    });
    sheet.textContent = css;
    document.head.appendChild(sheet);
    for (const g of groups) for (const c of g.chars) { c.level = 0; levels[c.face][0].add(c.range); }
  }

  const total = groups.reduce((s, g) => s + g.chars.length, 0);
  const shards = out && total ? shardLayer(faces, total * 3 <= MAX_SHARDS ? 3 : 2, total) : null;

  const anims: Animation[] = [];
  const timers: number[] = [];
  let raf = 0;
  let foldDone = false;
  const endFold = () => {
    if (foldDone) return;
    foldDone = true;
    if (out) root.dataset.hints = next;
    anims.forEach((a) => a.cancel());
    els.forEach((el) => el.classList.remove("ap-folding"));
    for (const n of names) reg.delete(n);
    sheet.remove();
  };
  const end = () => {
    timers.forEach((t) => window.clearTimeout(t));
    cancelAnimationFrame(raf);
    window.removeEventListener("resize", end);
    endFold();
    shards?.remove();
    if (finish === end) finish = null;
  };
  finish = end;
  window.addEventListener("resize", end);

  // Going out, a hint's space starts folding once its wave is under way (its last lines,
  // which go first, are the first to be covered).
  let foldEnd = 0;
  els.forEach((el, i) => {
    const f = frames[i];
    if (!f) return;
    const g = groups[i];
    const delay = out ? Math.round(g.start + (g.end - g.start) * 0.45) : 0;
    el.classList.add("ap-folding");
    anims.push(el.animate(out ? f : [f[1], f[0]], { duration: FOLD_MS, delay, easing: EASE, fill: "both" }));
    foldEnd = Math.max(foldEnd, delay + FOLD_MS);
  });

  const t0 = performance.now();
  const frame = (now: number) => {
    const t = now - t0;
    if (out) {
      // Read every rect first, then write, so a frame lays out once.
      const due: { g: Group; c: Char; r: DOMRect | null }[] = [];
      for (const g of groups) {
        const b = g.el.isConnected ? g.el.getBoundingClientRect() : null;
        if (b && (b.width || b.height)) { g.x = b.left; g.y = b.top; }
        let clip: Box | null = null;
        for (const c of g.chars) {
          if (c.done || t < c.delay) continue;
          c.done = true;
          const r = c.range.getClientRects()[0] ?? null;
          clip ??= clipOf(g.clippers);
          due.push({ g, c, r: r && r.width > 0 && inside(r, clip) ? r : null });
        }
      }
      for (const { g, c, r } of due) {
        gone.add(c.range);
        if (r) shards?.crack(t, c.range.toString(), r, c.face, g);
      }
      shards?.paint(t);
    } else {
      for (const g of groups) {
        for (const c of g.chars) {
          if (c.done) continue;
          const k = Math.max(0, Math.floor(((t - c.delay) / IN_MS) * LEVELS));
          if (k === c.level) continue;
          levels[c.face][c.level].delete(c.range);
          if (k >= LEVELS) { c.done = true; continue; }
          levels[c.face][k].add(c.range);
          c.level = k;
        }
      }
    }
    if (t < last + (out ? SHARD_MAX_MS : IN_MS)) raf = requestAnimationFrame(frame);
  };
  frame(t0);

  // Timers, not requestAnimationFrame: a hidden window stops painting, and the page must
  // still end up in the new mode.
  if (out) {
    // The hints leave the layout once folded and crumbled; the shards fly on.
    timers.push(window.setTimeout(endFold, Math.max(foldEnd, last + 20)));
    timers.push(window.setTimeout(end, last + SHARD_MAX_MS + 40));
  } else {
    timers.push(window.setTimeout(end, Math.max(foldEnd, last + IN_MS) + 20));
  }
}
