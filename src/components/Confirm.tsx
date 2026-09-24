import { type ReactNode, useEffect, useRef, useState } from "react";
import { Icon } from "./icons";
import { t } from "../i18n";

export interface ConfirmOptions {
  title: string;
  message?: ReactNode;
  confirmText?: string;
  danger?: boolean;
  /** An on/off option shown in the dialog; its value is returned by askCheck(). */
  check?: { label: string; hint?: ReactNode; value: boolean };
}

type Pending = ConfirmOptions & { resolve: (ok: boolean, checked: boolean) => void };
let show: ((p: Pending) => void) | null = null;

/** Asks the user to confirm; resolves false when dismissed. Needs <ConfirmHost/> mounted. */
export function ask(opts: ConfirmOptions): Promise<boolean> {
  return new Promise((resolve) => {
    if (!show) return resolve(window.confirm(opts.title));
    show({ ...opts, resolve });
  });
}

/** Like ask() with an option switch: resolves the switch's value, or null when dismissed. */
export function askCheck(opts: ConfirmOptions & { check: NonNullable<ConfirmOptions["check"]> }): Promise<boolean | null> {
  return new Promise((resolve) => {
    if (!show) return resolve(window.confirm(opts.title) ? opts.check.value : null);
    show({ ...opts, resolve: (ok, checked) => resolve(ok ? checked : null) });
  });
}

/** Renders the confirm dialog requested through ask(). Mount once, near the app root. */
export function ConfirmHost() {
  const [cur, setCur] = useState<Pending | null>(null);
  const [checked, setChecked] = useState(false);
  const okRef = useRef<HTMLButtonElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    show = (p) => {
      setChecked(p.check?.value ?? false);
      setCur(p);
    };
    return () => { show = null; };
  }, []);

  useEffect(() => {
    if (!cur) return;
    // Destructive: start on Cancel, so a stray Enter does not delete anything.
    (cur.danger ? cancelRef : okRef).current?.focus();
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") { e.preventDefault(); e.stopPropagation(); done(false); }
    };
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
  }, [cur]);

  const done = (ok: boolean) => {
    cur?.resolve(ok, checked);
    setCur(null);
  };

  if (!cur) return null;
  return (
    <div className="modal-bg confirm-bg" onMouseDown={(e) => { if (e.target === e.currentTarget) done(false); }}>
      <div className="modal confirm" role="alertdialog" aria-modal="true" aria-label={cur.title}>
        <div className="confirm-body">
          <span className={`confirm-icon${cur.danger ? " danger" : ""}`}>{cur.danger ? <Icon.trash size={16} /> : <Icon.check size={16} />}</span>
          <div className="grow minw0">
            <div className="confirm-title">{cur.title}</div>
            {cur.message && <div className="confirm-msg">{cur.message}</div>}
            {cur.check && (
              <div className={`gw-toggle confirm-check${checked ? " on" : ""}`}>
                <div className="grow minw0">
                  <div className="small strong">{cur.check.label}</div>
                  {cur.check.hint && <div className="tiny muted">{cur.check.hint}</div>}
                </div>
                <button type="button" className={`switch${checked ? " on" : ""}`} role="switch" aria-checked={checked} aria-label={cur.check.label}
                  onClick={() => setChecked((v) => !v)}><span /></button>
              </div>
            )}
          </div>
        </div>
        <div className="modal-foot">
          <span className="grow" />
          <button ref={cancelRef} className="btn" onClick={() => done(false)}>{t("common.cancel")}</button>
          <button ref={okRef} className={`btn ${cur.danger ? "danger-solid" : "primary"}`} onClick={() => done(true)}>{cur.confirmText ?? (cur.danger ? t("common.delete") : t("common.confirm"))}</button>
        </div>
      </div>
    </div>
  );
}
