// Small form controls shared by the pages and dialogs. They render the same markup the
// stylesheets (and motion.ts) expect: .switch, .gw-toggle, .seg, .srow, .err.
import type { ReactNode } from "react";
import { scrub } from "../privacy";

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
export function ToggleRow({ icon, title, hint, on, onChange, disabled, className, label }: {
  icon?: ReactNode; title: ReactNode; hint?: ReactNode; on: boolean; onChange?: (next: boolean) => void;
  disabled?: boolean; className?: string; label?: string;
}) {
  return (
    <div className={`gw-toggle${className ? ` ${className}` : ""}${on ? " on" : ""}`}>
      {icon}
      <div className="grow minw0">
        <div className="small strong">{title}</div>
        {hint && <div className="tiny muted">{hint}</div>}
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
export function SettingRow({ label, desc, descClassName, note, lead, children, className, id, as: Tag = "div" }: {
  label: ReactNode; desc?: ReactNode; descClassName?: string; note?: ReactNode; lead?: ReactNode; children?: ReactNode;
  className?: string; id?: string; as?: "div" | "label";
}) {
  return (
    <Tag className={`srow${className ? ` ${className}` : ""}`} id={id}>
      {lead}
      <div className="grow minw0">
        <div className="slabel">{label}</div>
        {desc != null && <div className={`muted small${descClassName ? ` ${descClassName}` : ""}`}>{desc}</div>}
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
