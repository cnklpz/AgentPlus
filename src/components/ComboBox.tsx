import { useEffect, useRef, useState } from "react";
import { t } from "../i18n";

interface Props {
  value: string;
  options: string[];
  onChange: (v: string) => void;
  /** Enter with the menu closed. */
  onEnter?: () => void;
  placeholder?: string;
  disabled?: boolean;
  label?: string;
}

/** Text input with a styled suggestion menu (same look as Dropdown); free text is allowed. */
export function ComboBox({ value, options, onChange, onEnter, placeholder, disabled, label }: Props) {
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState(false);
  const [hi, setHi] = useState(0);
  // The menu floats above everything (fixed), so scrolling panels never clip it.
  const [pos, setPos] = useState<{ top: number; left: number; width: number } | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);

  // Show everything until the user types; then filter by what they typed.
  const q = value.trim().toLowerCase();
  const list = typed && q ? options.filter((o) => o.toLowerCase().includes(q)) : options;

  useEffect(() => {
    if (!open) return;
    setHi(Math.max(0, list.indexOf(value)));
    const r = root.current!.getBoundingClientRect();
    setPos({ top: r.bottom + 6, left: r.left, width: r.width });
    const onDown = (e: MouseEvent) => { if (!root.current?.contains(e.target as Node)) setOpen(false); };
    const onScroll = (e: Event) => { if (!(e.target as Element)?.closest?.(".combo-menu")) setOpen(false); };
    const onResize = () => setOpen(false);
    document.addEventListener("mousedown", onDown);
    document.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onResize);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onResize);
      setPos(null);
    };
  }, [open]);

  const pick = (v: string) => {
    onChange(v);
    setTyped(false);
    setOpen(false);
    input.current?.focus();
  };

  return (
    <div className={`dd combo${open ? " open" : ""}${disabled ? " disabled" : ""}`} ref={root}>
      <input
        ref={input}
        className="combo-input mono"
        value={value}
        disabled={disabled}
        placeholder={placeholder}
        aria-label={label}
        role="combobox"
        aria-expanded={open}
        aria-autocomplete="list"
        onFocus={() => setTyped(false)}
        onChange={(e) => { onChange(e.target.value); setTyped(true); setOpen(true); setHi(0); }}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown") { e.preventDefault(); if (!open) setOpen(true); else setHi((h) => Math.min(h + 1, list.length - 1)); }
          else if (e.key === "ArrowUp") { e.preventDefault(); setHi((h) => Math.max(h - 1, 0)); }
          else if (e.key === "Escape" && open) { e.preventDefault(); e.stopPropagation(); setOpen(false); }
          else if (e.key === "Enter") {
            e.preventDefault();
            if (open && list[hi]) pick(list[hi]);
            else onEnter?.();
          }
        }}
      />
      {options.length > 0 && (
        <button type="button" className="combo-toggle" tabIndex={-1} disabled={disabled} aria-label={t("comboBox.expand")}
          onClick={() => { setTyped(false); setOpen((o) => !o); input.current?.focus(); }}>
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>
        </button>
      )}
      {open && pos && list.length > 0 && (
        <div className="dd-menu combo-menu" role="listbox" style={{ position: "fixed", top: pos.top, left: pos.left, width: pos.width }}>
          {list.map((o, i) => (
            <button key={o} type="button" role="option" aria-selected={o === value}
              className={`dd-item${i === hi ? " hi" : ""}${o === value ? " sel" : ""}`}
              onMouseEnter={() => setHi(i)} onMouseDown={(e) => e.preventDefault()} onClick={() => pick(o)}>
              <span className="grow minw0 ellipsis">{o}</span>
              {o === value && (
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M20 6 9 17l-5-5" /></svg>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
