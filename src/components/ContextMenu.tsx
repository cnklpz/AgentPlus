import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useLatest, useListNav, usePopover } from "../hooks";

export type MenuItem =
  | { label: string; icon?: ReactNode; hint?: string; danger?: boolean; disabled?: boolean; action: () => void }
  | "sep";

interface Props {
  /** Builds the menu for the element that was right-clicked. */
  build: (target: Element, e: MouseEvent) => MenuItem[];
}

/** Replaces the webview's default right-click menu with the app's own. */
export function ContextMenu({ build }: Props) {
  const [menu, setMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  const buildRef = useLatest(build);
  const items = menu?.items ?? [];
  // Arrow keys skip separators and disabled items, and wrap around.
  const nav = useListNav(items.length, { wrap: true, enabled: (i) => items[i] !== "sep" && !(items[i] as Exclude<MenuItem, "sep">).disabled, initial: -1 });

  useEffect(() => {
    const onMenu = (e: MouseEvent) => {
      e.preventDefault();
      const t = e.target instanceof Element ? e.target : document.body;
      const items = buildRef.current(t, e);
      // Drop leading / trailing / doubled separators.
      const clean = items.filter((it, i, a) => it !== "sep" || (i > 0 && i < a.length - 1 && a[i - 1] !== "sep"));
      setMenu(clean.length ? { x: e.clientX, y: e.clientY, items: clean } : null);
      setPos(null);
      nav.setHi(-1);
    };
    document.addEventListener("contextmenu", onMenu);
    return () => document.removeEventListener("contextmenu", onMenu);
  }, []);

  // Keep the menu inside the window.
  useLayoutEffect(() => {
    if (!menu || !ref.current) return;
    const r = ref.current.getBoundingClientRect();
    setPos({
      left: Math.max(6, Math.min(menu.x, window.innerWidth - r.width - 6)),
      top: Math.max(6, menu.y + r.height > window.innerHeight - 6 ? menu.y - r.height : menu.y),
    });
  }, [menu]);

  const close = () => setMenu(null);
  // The menu sits above everything: Esc closes it alone, not the dialog or panel underneath.
  usePopover(!!menu, close, [ref], { scroll: true, resize: true, blur: true });
  // Focus stays where it was (the menu takes no focus): listen on the document.
  const onKey = useLatest((e: KeyboardEvent) => {
    if (nav.onKey(e)) return;
    const it = items[nav.hi];
    // Disabled items can be highlighted by hovering (WebView2 sends them mouse events), not run.
    if (e.key === "Enter" && it && it !== "sep" && !it.disabled) {
      e.preventDefault();
      close();
      it.action();
    }
  });
  useEffect(() => {
    if (!menu) return;
    const f = (e: KeyboardEvent) => onKey.current(e);
    document.addEventListener("keydown", f);
    return () => document.removeEventListener("keydown", f);
  }, [!!menu]);

  if (!menu) return null;
  return (
    <div ref={ref} className="ctx-menu" role="menu" style={{ left: pos?.left ?? menu.x, top: pos?.top ?? menu.y, visibility: pos ? "visible" : "hidden" }}
      onContextMenu={(e) => e.preventDefault()}>
      {menu.items.map((it, i) =>
        it === "sep" ? <div key={i} className="ctx-sep" role="separator" /> : (
          <button key={i} role="menuitem" className={`ctx-item${i === nav.hi ? " hi" : ""}${it.danger ? " danger" : ""}`} disabled={it.disabled}
            onMouseEnter={() => { if (!it.disabled) nav.setHi(i); }} onMouseDown={(e) => e.preventDefault()} onClick={() => { close(); it.action(); }}>
            <span className="ctx-icon">{it.icon}</span>
            <span className="grow">{it.label}</span>
            {it.hint && <span className="ctx-hint">{it.hint}</span>}
          </button>
        ),
      )}
    </div>
  );
}

// ---------------------------------------------------------------- editing helpers for inputs

type Editable = HTMLInputElement | HTMLTextAreaElement;

export function editableOf(t: Element): Editable | null {
  const el = t.closest("input, textarea");
  if (!el) return null;
  if (el instanceof HTMLInputElement && !["text", "search", "url", "password", "email", "number", ""].includes(el.type)) return null;
  return el as Editable;
}

/** Replaces the selection in a React-controlled input and lets React see the change. */
export function insertText(el: Editable, text: string) {
  el.focus();
  const start = el.selectionStart ?? el.value.length;
  const end = el.selectionEnd ?? el.value.length;
  const next = el.value.slice(0, start) + text + el.value.slice(end);
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(proto, "value")?.set?.call(el, next);
  el.dispatchEvent(new Event("input", { bubbles: true }));
  const caret = start + text.length;
  requestAnimationFrame(() => el.setSelectionRange(caret, caret));
}

export function selectedIn(el: Editable): string {
  return el.value.slice(el.selectionStart ?? 0, el.selectionEnd ?? 0);
}
