// Small form controls shared by the pages and dialogs. They render the same markup the
// stylesheets (and motion.ts) expect: .switch, .gw-toggle, .seg, .srow, .err.
import { type ReactNode, useEffect, useReducer, useRef } from "react";
import { useLang } from "../i18n";
import { scrub } from "../privacy";

/**
 * Text that changes wording in place (a setting's description following its value): the new
 * text eases in while the old one fades out over it. Only plain text animates, and not on a
 * language switch, where everything changes at once.
 */
export function Swap({ children }: { children: ReactNode }) {
  const lang = useLang();
  const text = typeof children === "string" || typeof children === "number" ? String(children) : null;
  const [, redraw] = useReducer((n: number) => n + 1, 0);
  const seen = useRef<{ text: string | null; lang: string; node: ReactNode; n: number; old: ReactNode }>();
  seen.current ??= { text, lang, node: children, n: 0, old: null };
  const s = seen.current;
  if (text !== s.text) {
    const animate = text !== null && s.text !== null && lang === s.lang && document.documentElement.dataset.motion !== "off";
    if (animate) {
      s.old = s.node;
      s.n++;
    }
    s.text = text;
  }
  s.lang = lang;
  s.node = children;
  useEffect(() => {
    if (!s.n) return;
    const timer = window.setTimeout(() => { s.old = null; redraw(); }, 320);
    return () => window.clearTimeout(timer);
  }, [s.n]);
  return (
    <span className="swap">
      <span key={s.n} className={s.n ? "swap-in" : undefined}>{children}</span>
      {s.old != null && <span className="swap-out" aria-hidden="true">{s.old}</span>}
    </span>
  );
}

/** On/off switch. `fast`: the orange "fast mode" colour when on. */
export function Switch({ on, onChange, label, disabled, fast }: {
  on: boolean; onChange: (next: boolean) => void; label: string; disabled?: boolean; fast?: boolean;
}) {
  return (
    <button type="button" className={`switch${on ? " on" : ""}${fast ? " fast" : ""}`} role="switch" aria-checked={on} aria-label={label}
      disabled={disabled} onClick={() => onChange(!on)}>
      <span />
    </button>
  );
}

/**
 * A bordered option card (.gw-toggle): icon, title, hint and a switch. Without `onChange`
 * it is a static notice (no switch), highlighted when `on`. `label` names the switch when
 * the title is not plain text (or should read differently).
 */
/** A switch with a title and a hint. The hint explains the option, so brief hints mode hides
 *  it, unless `keepHint` (it says something current) or there is no switch (a static notice). */
export function ToggleRow({ icon, title, hint, keepHint, on, onChange, disabled, className, label }: {
  icon?: ReactNode; title: ReactNode; hint?: ReactNode; keepHint?: boolean; on: boolean; onChange?: (next: boolean) => void;
  disabled?: boolean; className?: string; label?: string;
}) {
  return (
    <div className={`gw-toggle${className ? ` ${className}` : ""}${on ? " on" : ""}`}>
      {icon}
      <div className="grow minw0">
        <div className="small strong">{title}</div>
        {hint && <div className={`tiny muted${onChange && !keepHint ? " hint" : ""}`}><Swap>{hint}</Swap></div>}
      </div>
      {onChange && <Switch on={on} onChange={onChange} disabled={disabled} label={label ?? (typeof title === "string" ? title : "")} />}
    </div>
  );
}

export interface SegOption<T> {
  value: T;
  label: ReactNode;
  title?: string;
  disabled?: boolean;
  /** Language of the label (language names are written in their own language). */
  lang?: string;
}

/** Segmented control: one of a few options. */
export function Seg<T extends string | number>({ value, options, onChange, label, className }: {
  value: T; options: SegOption<T>[]; onChange: (v: T) => void; label?: string; className?: string;
}) {
  return (
    <div className={`seg${className ? ` ${className}` : ""}`} role="radiogroup" aria-label={label}>
      {options.map((o) => (
        <button key={String(o.value)} type="button" role="radio" aria-checked={value === o.value} className={value === o.value ? "on" : ""}
          title={o.title} disabled={o.disabled} lang={o.lang} onClick={() => onChange(o.value)}>{o.label}</button>
      ))}
    </div>
  );
}

/** `Seg` with several options on at once; the last one on can't be turned off. */
export function SegMulti<T extends string | number>({ value, options, onChange, label, className }: {
  value: T[]; options: SegOption<T>[]; onChange: (v: T[]) => void; label?: string; className?: string;
}) {
  return (
    <div className={`seg${className ? ` ${className}` : ""}`} role="group" aria-label={label}>
      {options.map((o) => {
        const on = value.includes(o.value);
        return (
          <button key={String(o.value)} type="button" aria-pressed={on} className={on ? "on" : ""}
            title={o.title} disabled={o.disabled} lang={o.lang}
            onClick={() => {
              if (!on) onChange([...value, o.value]);
              else if (value.length > 1) onChange(value.filter((v) => v !== o.value));
            }}>{o.label}</button>
        );
      })}
    </div>
  );
}

/**
 * One row of a settings list (.srow): title and description on the left, the control
 * (children) on the right. `lead` goes before the text (a checkbox, a status dot); `note`
 * under the description. `as="label"` makes the whole row toggle a checkbox inside it.
 */
/** A setting: its name, a description (an explanation: hidden in brief hints mode unless
 *  `keepDesc`, for a description that says something current such as a path or a count),
 *  and its control. */
export function SettingRow({ label, desc, descClassName, keepDesc, note, lead, children, className, id, as: Tag = "div" }: {
  label: ReactNode; desc?: ReactNode; descClassName?: string; keepDesc?: boolean; note?: ReactNode; lead?: ReactNode; children?: ReactNode;
  className?: string; id?: string; as?: "div" | "label";
}) {
  return (
    <Tag className={`srow${className ? ` ${className}` : ""}`} id={id}>
      {lead}
      <div className="grow minw0">
        <div className="slabel">{label}</div>
        {desc != null && <div className={`muted small${keepDesc ? "" : " hint"}${descClassName ? ` ${descClassName}` : ""}`}><Swap>{desc}</Swap></div>}
        {note}
      </div>
      {children}
    </Tag>
  );
}

/** An error message box. The text may carry addresses, keys or paths: masked in privacy mode. */
export function ErrorBox({ text, className, alert }: { text: string; className?: string; alert?: boolean }) {
  return <div className={`err${className ? ` ${className}` : ""}`} role={alert ? "alert" : undefined}>{scrub(text)}</div>;
}
