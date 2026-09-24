import { type ReactNode, useEffect, useRef, useState } from "react";
import { Icon } from "./icons";
import { ConfirmFrame } from "./Modal";
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
  const [cur, setCur] = useState<{ p: Pending; n: number } | null>(null);
  const pending = useRef<Pending | null>(null);

  useEffect(() => {
    let n = 0;
    show = (p) => {
      // A second question replaces the first: the first one counts as dismissed.
      pending.current?.resolve(false, false);
      pending.current = p;
      setCur({ p, n: ++n });
    };
    return () => { show = null; };
  }, []);

  const done = (p: Pending, ok: boolean, checked: boolean) => {
    if (pending.current !== p) return;
    pending.current = null;
    p.resolve(ok, checked);
    setCur(null);
  };

  return cur && <ConfirmDialog key={cur.n} p={cur.p} onDone={(ok, checked) => done(cur.p, ok, checked)} />;
}

function ConfirmDialog({ p, onDone }: { p: Pending; onDone: (ok: boolean, checked: boolean) => void }) {
  const [checked, setChecked] = useState(p.check?.value ?? false);
  const okRef = useRef<HTMLButtonElement>(null);
  const cancelRef = useRef<HTMLButtonElement>(null);
  // Destructive: start on Cancel, so a stray Enter does not delete anything.
  useEffect(() => { (p.danger ? cancelRef : okRef).current?.focus(); }, []);

  return (
    <ConfirmFrame
      title={p.title}
      icon={p.danger ? <Icon.trash size={16} /> : <Icon.check size={16} />}
      danger={p.danger}
      message={p.message}
      check={p.check && { label: p.check.label, hint: p.check.hint, on: checked, onChange: setChecked }}
      onClose={() => onDone(false, checked)}
      foot={<>
        <span className="grow" />
        <button ref={cancelRef} className="btn" onClick={() => onDone(false, checked)}>{t("common.cancel")}</button>
        <button ref={okRef} className={`btn ${p.danger ? "danger-solid" : "primary"}`} onClick={() => onDone(true, checked)}>{p.confirmText ?? (p.danger ? t("common.delete") : t("common.confirm"))}</button>
      </>}
    />
  );
}
