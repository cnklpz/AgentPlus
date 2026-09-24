import { type ReactNode, useEffect, useLayoutEffect, useRef, useState } from "react";

export interface DropdownOption {
  value: string;
  label: string;
  hint?: string;
  icon?: ReactNode;
  /** Section heading, shown above the first option of each group. */
  group?: string;
}

interface Props {
  value: string;
  options: DropdownOption[];
  onChange: (v: string) => void;
  disabled?: boolean;
  label?: string;
  /** Tallest the menu may grow (default 280). */
  maxHeight?: number;
}

/**
 * A styled replacement for <select>: keyboard and outside-click aware. The menu is
 * fixed-positioned so scrolling containers (dialogs, side panels) never clip it, and it
 * opens upwards when there is no room below.
 */
export function Dropdown({ value, options, onChange, disabled, label, maxHeight = 280 }: Props) {
  const [open, setOpen] = useState(false);
  const [hi, setHi] = useState(0);
  const [pos, setPos] = useState<{ left: number; top: number; minWidth: number; maxHeight: number; up: boolean } | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const cur = options.find((o) => o.value === value);

  useEffect(() => {
    if (!open) return;
    setHi(Math.max(0, options.findIndex((o) => o.value === value)));
    const close = () => setOpen(false);
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (!root.current?.contains(t) && !menu.current?.contains(t)) close();
    };
    const onScroll = (e: Event) => { if (!menu.current?.contains(e.target as Node)) close(); };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", close);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", close);
      setPos(null);
    };
  }, [open]);

  // Place the menu under the button, or above it when the window has no room below.
  useLayoutEffect(() => {
    if (!open || !root.current) return;
    const r = root.current.getBoundingClientRect();
    const groups = new Set(options.map((o) => o.group).filter(Boolean)).size;
    const want = Math.min(maxHeight, options.length * 44 + groups * 28 + 12);
    const below = window.innerHeight - r.bottom - 14;
    const above = r.top - 14;
    // Open on the side with room; if neither fits, the roomier side, scrolling inside.
    const up = below < want && above > below;
    const h = Math.max(80, Math.min(want, up ? above : below));
    setPos({ left: r.left, top: up ? r.top - 6 - h : r.bottom + 6, minWidth: r.width, maxHeight: h, up });
  }, [open]);

  const pick = (v: string) => { onChange(v); setOpen(false); };

  return (
    <div className="dd" ref={root}>
      <button
        type="button"
        className={`dd-btn${open ? " open" : ""}`}
        disabled={disabled}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={label}
        onClick={() => setOpen((o) => !o)}
        onKeyDown={(e) => {
          if (!open && (e.key === "ArrowDown" || e.key === "Enter" || e.key === " ")) { e.preventDefault(); setOpen(true); return; }
          if (!open) return;
          if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); setOpen(false); }
          else if (e.key === "ArrowDown") { e.preventDefault(); setHi((h) => Math.min(h + 1, options.length - 1)); }
          else if (e.key === "ArrowUp") { e.preventDefault(); setHi((h) => Math.max(h - 1, 0)); }
          else if (e.key === "Enter") { e.preventDefault(); pick(options[hi].value); }
        }}
      >
        <span className="dd-cur minw0">{cur?.icon}<span className="ellipsis">{cur?.label ?? value}</span></span>
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>
      </button>
      {open && pos && (
        <div ref={menu} className={`dd-menu dd-float${pos.up ? " up" : ""}`} role="listbox"
          style={{ position: "fixed", left: pos.left, top: pos.top, minWidth: pos.minWidth, maxHeight: pos.maxHeight }}>
          {options.map((o, i) => [
            o.group && o.group !== options[i - 1]?.group && <div key={`g:${o.group}`} className="dd-group tiny muted">{o.group}</div>,
            <button
              key={o.value}
              type="button"
              role="option"
              aria-selected={o.value === value}
              className={`dd-item${i === hi ? " hi" : ""}${o.value === value ? " sel" : ""}`}
              onMouseEnter={() => setHi(i)}
              onClick={() => pick(o.value)}
            >
              {o.icon}
              <span className="grow minw0">
                <span className="block ellipsis">{o.label}</span>
                {o.hint && <span className="block tiny muted ellipsis">{o.hint}</span>}
              </span>
              {o.value === value && (
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M20 6 9 17l-5-5" /></svg>
              )}
            </button>,
          ])}
        </div>
      )}
    </div>
  );
}
