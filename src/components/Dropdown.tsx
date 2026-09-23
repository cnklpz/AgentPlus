import { useEffect, useRef, useState } from "react";

export interface DropdownOption {
  value: string;
  label: string;
  hint?: string;
}

interface Props {
  value: string;
  options: DropdownOption[];
  onChange: (v: string) => void;
  disabled?: boolean;
  label?: string;
}

/** A styled replacement for <select>: keyboard and outside-click aware. */
export function Dropdown({ value, options, onChange, disabled, label }: Props) {
  const [open, setOpen] = useState(false);
  const [hi, setHi] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const cur = options.find((o) => o.value === value);

  useEffect(() => {
    if (!open) return;
    setHi(Math.max(0, options.findIndex((o) => o.value === value)));
    const onDown = (e: MouseEvent) => { if (!root.current?.contains(e.target as Node)) setOpen(false); };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
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
          if (e.key === "Escape") { e.preventDefault(); setOpen(false); }
          else if (e.key === "ArrowDown") { e.preventDefault(); setHi((h) => Math.min(h + 1, options.length - 1)); }
          else if (e.key === "ArrowUp") { e.preventDefault(); setHi((h) => Math.max(h - 1, 0)); }
          else if (e.key === "Enter") { e.preventDefault(); pick(options[hi].value); }
        }}
      >
        <span className="ellipsis">{cur?.label ?? value}</span>
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>
      </button>
      {open && (
        <div className="dd-menu" role="listbox">
          {options.map((o, i) => (
            <button
              key={o.value}
              type="button"
              role="option"
              aria-selected={o.value === value}
              className={`dd-item${i === hi ? " hi" : ""}${o.value === value ? " sel" : ""}`}
              onMouseEnter={() => setHi(i)}
              onClick={() => pick(o.value)}
            >
              <span className="grow minw0">
                <span className="block ellipsis">{o.label}</span>
                {o.hint && <span className="block tiny muted ellipsis">{o.hint}</span>}
              </span>
              {o.value === value && (
                <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M20 6 9 17l-5-5" /></svg>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
