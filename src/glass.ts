// Liquid glass, after Shu Ding's liquid-glass (https://github.com/shuding/liquid-glass, MIT):
// the backdrop is refracted through an SVG displacement map, then sharpened and brightened
// a little. The slab magnifies slightly, so what passes under it moves at another pace than
// what is around it, and its rim bends light hard, like the rounded edge of a glass block;
// red, green and blue bend by slightly different amounts, fringing edges with colour. The
// look is on [data-glass] in styles-motion.css. Chromium only: WebKit can't use an SVG
// filter as a backdrop-filter, so on macOS the styles fall back to a frosted blur.

const NS = "http://www.w3.org/2000/svg";
const FILTER_ID = "ap-glass";
/** How much more green and blue bend than red. */
const DISPERSION = [1, 1.07, 1.14];

function smoothStep(a: number, b: number, t: number): number {
  const x = Math.max(0, Math.min(1, (t - a) / (b - a)));
  return x * x * (3 - 2 * x);
}

/** Signed distance to a rounded rectangle centred on the origin (negative inside). */
function roundedRectSDF(x: number, y: number, hw: number, hh: number, r: number): number {
  const qx = Math.abs(x) - hw + r;
  const qy = Math.abs(y) - hh + r;
  return Math.min(Math.max(qx, qy), 0) + Math.hypot(Math.max(qx, 0), Math.max(qy, 0)) - r;
}

/**
 * The displacement map for a w x h slab with corner radius r, as RGBA pixels: R and G say
 * how far each pixel reaches for the backdrop it shows (0.5 = its own spot), in units of
 * `scale` px. The whole slab reaches a little toward its centre (a mild magnifier); within
 * the bezel, a band along the rim, it also reaches inward, hardest at the very edge.
 */
export function glassMap(w: number, h: number, r: number): { data: Uint8ClampedArray<ArrayBuffer>; scale: number } {
  const bezel = Math.min(16, Math.min(w, h) * 0.3);
  const depth = bezel * 0.9;
  const zoom = 0.07;
  const sdf = (x: number, y: number) => roundedRectSDF(x - w / 2, y - h / 2, w / 2, h / 2, r);
  const off = new Float32Array(w * h * 2);
  let max = 0;
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      const [px, py] = [x + 0.5, y + 0.5];
      const d = sdf(px, py);
      // Outside the rounded corners: clipped away anyway.
      if (d > 0) continue;
      let [ox, oy] = [(w / 2 - px) * zoom, (h / 2 - py) * zoom];
      // 1 at the edge, 0 past the bezel: the slope of a rounded rim.
      const k = 1 - smoothStep(0, 1, -d / bezel);
      if (k > 0) {
        // The outward normal, from the distance field's slope.
        const nx = sdf(px + 1, py) - sdf(px - 1, py);
        const ny = sdf(px, py + 1) - sdf(px, py - 1);
        const s = (-depth * k * k) / (Math.hypot(nx, ny) || 1);
        ox += nx * s;
        oy += ny * s;
      }
      const i = (y * w + x) * 2;
      off[i] = ox;
      off[i + 1] = oy;
      max = Math.max(max, Math.abs(ox), Math.abs(oy));
    }
  }
  const scale = max * 2 || 1;
  const data = new Uint8ClampedArray(w * h * 4);
  for (let p = 0; p < w * h; p++) {
    data[p * 4] = Math.round((off[p * 2] / scale + 0.5) * 255);
    data[p * 4 + 1] = Math.round((off[p * 2 + 1] / scale + 0.5) * 255);
    data[p * 4 + 3] = 255;
  }
  return { data, scale };
}

/** Rendered maps by size: a drag measures the same row every time. */
const maps = new Map<string, { url: string; scale: number }>();
let filter: { root: SVGSVGElement; filter: SVGFilterElement; image: SVGFEImageElement; disps: SVGFEDisplacementMapElement[] } | null = null;

function ensureFilter() {
  if (filter?.root.isConnected) return filter;
  const el = <K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string>) => {
    const n = document.createElementNS(NS, tag);
    for (const [k, v] of Object.entries(attrs)) n.setAttribute(k, v);
    return n;
  };
  const root = el("svg", { width: "0", height: "0", "aria-hidden": "true", style: "position: absolute; pointer-events: none" });
  const f = el("filter", { id: FILTER_ID, filterUnits: "userSpaceOnUse", "color-interpolation-filters": "sRGB", x: "0", y: "0" });
  const image = el("feImage", { result: "map", preserveAspectRatio: "none" });
  f.appendChild(image);
  // One displacement per colour channel, each keeping only its channel, added back together.
  const disps = ["r", "g", "b"].map((c, i) => {
    const disp = el("feDisplacementMap", { in: "SourceGraphic", in2: "map", xChannelSelector: "R", yChannelSelector: "G", result: `${c}0` });
    const keep = ["1 0 0 0 0", "0 1 0 0 0", "0 0 1 0 0"].map((row, j) => (j === i ? row : "0 0 0 0 0")).join("  ");
    f.append(disp, el("feColorMatrix", { in: `${c}0`, type: "matrix", values: `${keep}  0 0 0 1 0`, result: c }));
    return disp;
  });
  f.append(el("feBlend", { in: "r", in2: "g", mode: "screen", result: "rg" }), el("feBlend", { in: "rg", in2: "b", mode: "screen" }));
  root.appendChild(el("defs", {})).appendChild(f);
  document.body.appendChild(root);
  filter = { root, filter: f, image, disps };
  return filter;
}

/** The glass melting away, if any: a new slab finishes it at once. */
let melting: (() => void) | null = null;

/**
 * A backdrop filter list eased toward doing nothing: at k = 1 as given, at 0 blur(0) and
 * the colour functions at 1. The url() stays: SVG filters can't be interpolated, so its
 * displacement is scaled down separately.
 */
export function easeFilter(list: string, k: number): string {
  return list.replace(/([a-z-]+)\(([\d.]+)(px)?\)/g, (all, fn: string, v: string, px?: string) => {
    if (fn === "url") return all;
    const n = parseFloat(v);
    return fn === "blur" ? `blur(${(n * k).toFixed(3)}px)` : `${fn}(${(1 + (n - 1) * k).toFixed(3)})${px ?? ""}`;
  });
}

/**
 * Turns `el` into a slab of glass the shape of its border box. Returns the undo, which
 * melts the glass back into `el`'s own look over `ms` (0: at once); call it once `el` is
 * styled as it should end up, with transitions off.
 */
export function glass(el: HTMLElement): (ms?: number) => void {
  melting?.();
  const [w, h] = [el.offsetWidth, el.offsetHeight];
  if (!w || !h) return () => {};
  const r = Math.min(parseFloat(getComputedStyle(el).borderTopLeftRadius) || 0, w / 2, h / 2);
  const key = `${w}x${h}x${r}`;
  let map = maps.get(key);
  if (!map) {
    const { data, scale } = glassMap(w, h, r);
    const canvas = document.createElement("canvas");
    [canvas.width, canvas.height] = [w, h];
    const ctx = canvas.getContext("2d");
    if (!ctx) return () => {};
    ctx.putImageData(new ImageData(data, w, h), 0, 0);
    map = { url: canvas.toDataURL(), scale };
    maps.set(key, map);
  }
  const f = ensureFilter();
  for (const n of [f.filter, f.image]) {
    n.setAttribute("width", String(w));
    n.setAttribute("height", String(h));
  }
  f.image.setAttribute("href", map.url);
  const bend = (k: number) => f.disps.forEach((d, i) => d.setAttribute("scale", (map.scale * DISPERSION[i] * k).toFixed(2)));
  bend(1);
  el.dataset.glass = "";
  return (ms = 0) => {
    if (!ms) {
      delete el.dataset.glass;
      return;
    }
    // Glass and final look, both read with transitions off; the backdrop filter is held
    // inline and eased by hand, the rest is a plain animation between the two.
    const cs = getComputedStyle(el);
    const look = () => ({ backgroundColor: cs.backgroundColor, borderColor: cs.borderColor, boxShadow: cs.boxShadow });
    const filter = cs.backdropFilter;
    const from = look();
    el.style.backdropFilter = filter;
    delete el.dataset.glass;
    const anim = el.animate([from, look()], { duration: ms, easing: "cubic-bezier(.2, .7, .3, 1)" });
    const t0 = performance.now();
    let frame = 0;
    const done = () => {
      cancelAnimationFrame(frame);
      anim.cancel();
      el.style.removeProperty("backdrop-filter");
      if (melting === done) melting = null;
    };
    const tick = (now: number) => {
      const t = Math.min(1, (now - t0) / ms);
      if (t >= 1) return done();
      const k = (1 - t) ** 2;
      bend(k);
      el.style.backdropFilter = easeFilter(filter, k);
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    melting = done;
    // A hidden window stops animation frames; the glass must still go.
    window.setTimeout(() => { if (melting === done) done(); }, ms + 100);
  };
}
