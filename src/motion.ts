// The "Extra" motion level: effects that need pointer or DOM state, which CSS alone
// can't do. Installed once at startup; every handler is a no-op unless
// <html data-motion="rich"> and the system isn't asking for reduced motion.
// Styles for the classes used here live in styles-motion.css.

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

function rubberBand(e: WheelEvent) {
  if (e.ctrlKey || Math.abs(e.deltaY) < Math.abs(e.deltaX) || e.deltaY === 0) return;
  const down = e.deltaY > 0;
  // The browser chains a wheel to the next ancestor that can still scroll; only when
  // none can does the innermost scroller at its edge stretch.
  let edge: HTMLElement | null = null;
  for (let n = e.target as HTMLElement | null; n && n !== document.body; n = n.parentElement) {
    if (!overflowing(n)) continue;
    const atEdge = down ? n.scrollTop + n.clientHeight >= n.scrollHeight - 1 : n.scrollTop <= 0;
    if (!atEdge) return;
    edge ??= n;
  }
  if (!edge) return;
  if (obHost !== edge) {
    settle(true);
    obHost = edge;
    obPull = 0;
  }
  const px = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
  obPull += -px * 0.5;
  const max = 56;
  const off = Math.sign(obPull) * max * (1 - Math.exp(-Math.abs(obPull) / (max * 1.6)));
  window.clearTimeout(obClear);
  edge.classList.add("ap-ob", "ap-pulling");
  edge.style.setProperty("--ob", `${off.toFixed(1)}px`);
  window.clearTimeout(obRelease);
  obRelease = window.setTimeout(() => settle(false), 110);
}

function settle(now: boolean) {
  const host = obHost;
  if (!host) return;
  window.clearTimeout(obRelease);
  host.classList.remove("ap-pulling");
  host.style.setProperty("--ob", "0px");
  obPull = 0;
  const clear = () => {
    host.classList.remove("ap-ob");
    host.style.removeProperty("--ob");
    if (obHost === host) obHost = null;
  };
  if (now) clear();
  else obClear = window.setTimeout(clear, 650);
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
