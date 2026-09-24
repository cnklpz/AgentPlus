import { type ReactNode, useEffect, useRef, useState } from "react";
import { useFloatingMenu, useListNav, usePopover } from "../hooks";
import { scrub } from "../privacy";
import { Icon } from "./icons";

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
  const root = useRef<HTMLDivElement>(null);
  const nav = useListNav(options.length);
  const cur = options.find((o) => o.value === value);

  // Start on the chosen option.
  useEffect(() => { if (open) nav.setHi(Math.max(0, options.findIndex((o) => o.value === value))); }, [open]);
  usePopover(open, () => setOpen(false), [root], { scroll: true, resize: true });
  const groups = new Set(options.map((o) => o.group).filter(Boolean)).size;
  const float = useFloatingMenu(root, nav.list, open, options.length * 44 + groups * 28 + 12, { maxHeight });

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
          if (!open || nav.onKey(e)) return;
          if (e.key === "Enter") { e.preventDefault(); const o = options[nav.hi]; if (o) pick(o.value); else setOpen(false); }
        }}
      >
        <span className="dd-cur minw0">{cur?.icon}<span className="ellipsis">{cur?.label ?? value}</span></span>
        <Icon.chevron />
      </button>
      {open && float && (
        <div ref={nav.list} className={`dd-menu dd-float${float.up ? " up" : ""}`} role="listbox" style={float.style}>
          {options.map((o, i) => [
            o.group && o.group !== options[i - 1]?.group && <div key={`g:${o.group}`} className="dd-group tiny muted">{o.group}</div>,
            <button
              key={o.value}
              type="button"
              role="option"
              aria-selected={o.value === value}
              className={`dd-item${i === nav.hi ? " hi" : ""}${o.value === value ? " sel" : ""}`}
              onMouseEnter={() => nav.setHi(i)}
              onClick={() => pick(o.value)}
            >
              {o.icon}
              <span className="grow minw0">
                <span className="block ellipsis">{o.label}</span>
                {o.hint && <span className="block tiny muted ellipsis">{scrub(o.hint)}</span>}
              </span>
              {o.value === value && <Icon.check size={13} sw={2.6} />}
            </button>,
          ])}
        </div>
      )}
    </div>
  );
}
