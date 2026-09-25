import { useEffect, useRef, useState } from "react";
import { useFloatingMenu, useListNav, usePopover } from "../hooks";
import { t } from "../i18n";
import { Icon } from "./icons";

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
  const root = useRef<HTMLDivElement>(null);
  const input = useRef<HTMLInputElement>(null);

  // Show everything until the user types; then filter by what they typed.
  const listFor = (text: string, filtered: boolean) => {
    const q = text.trim().toLowerCase();
    return filtered && q ? options.filter((o) => o.toLowerCase().includes(q)) : options;
  };
  const list = listFor(value, typed);
  const nav = useListNav(list.length);

  useEffect(() => { if (open) nav.setHi(Math.max(0, list.indexOf(value))); }, [open]);
  // Only open while there is something to list: an invisible "open" menu would still take Esc
  // (and outside clicks) away from the page or dialog underneath.
  const shown = open && list.length > 0;
  usePopover(shown, () => setOpen(false), [root], { scroll: true, resize: true });
  // Sized for the whole list: typing only narrows it.
  const float = useFloatingMenu(root, nav.list, open, options.length * 34 + 12, { matchWidth: true });

  const pick = (v: string) => {
    onChange(v);
    setTyped(false);
    setOpen(false);
    input.current?.focus();
  };

  return (
    <div className={`dd combo${shown ? " open" : ""}${disabled ? " disabled" : ""}`} ref={root}>
      <input
        ref={input}
        className="combo-input mono"
        value={value}
        disabled={disabled}
        placeholder={placeholder}
        aria-label={label}
        role="combobox"
        aria-expanded={shown}
        aria-autocomplete="list"
        onFocus={() => setTyped(false)}
        onChange={(e) => { onChange(e.target.value); setTyped(true); setOpen(listFor(e.target.value, true).length > 0); nav.setHi(0); }}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" && !open) { e.preventDefault(); setOpen(list.length > 0); }
          else if (nav.onKey(e)) return;
          else if (e.key === "Enter") {
            e.preventDefault();
            if (open && list[nav.hi]) pick(list[nav.hi]);
            else onEnter?.();
          }
        }}
      />
      {options.length > 0 && (
        <button type="button" className="combo-toggle" tabIndex={-1} disabled={disabled} aria-label={t("comboBox.expand")}
          onClick={() => { setTyped(false); setOpen((o) => !o); input.current?.focus(); }}>
          <Icon.chevron />
        </button>
      )}
      {shown && float && (
        <div ref={nav.list} className={`dd-menu combo-menu${float.up ? " up" : ""}`} role="listbox" style={float.style}>
          {list.map((o, i) => (
            <button key={o} type="button" role="option" aria-selected={o === value}
              className={`dd-item${i === nav.hi ? " hi" : ""}${o === value ? " sel" : ""}`}
              onMouseEnter={() => nav.setHi(i)} onMouseDown={(e) => e.preventDefault()} onClick={() => pick(o)}>
              <span className="grow minw0 ellipsis">{o}</span>
              {o === value && <Icon.check size={13} sw={2.6} />}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
