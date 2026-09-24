import type { KeyboardEventHandler, ReactNode } from "react";
import { useEscape } from "../hooks";
import { t } from "../i18n";
import { Icon } from "./icons";
import { ToggleRow } from "./controls";

interface ModalProps {
  /** Accessible name of the dialog. */
  label: string;
  /** Heading, shown with a close button; left out, the dialog has no head. */
  title?: ReactNode;
  onClose: () => void;
  /** Something is being written: Esc, the backdrop and the close button do nothing. */
  busy?: boolean;
  wide?: boolean;
  /** Extra classes on `.modal` / on the `.modal-bg` backdrop. */
  className?: string;
  bgClassName?: string;
  role?: "dialog" | "alertdialog";
  /** A press on the backdrop closes the dialog (default). */
  backdropClose?: boolean;
  /** Content of `.modal-foot`. */
  foot?: ReactNode;
  /** Children go straight into `.modal` instead of a `.modal-body`. */
  bare?: boolean;
  onKeyDown?: KeyboardEventHandler<HTMLDivElement>;
  children: ReactNode;
}

/**
 * A dialog over a dimmed backdrop (`.modal-bg` > `.modal`, the classes the styles and the
 * exit animation in motion.ts expect). Esc closes it — the topmost dialog only.
 */
export function Modal({ label, title, onClose, busy, wide, className, bgClassName, role = "dialog", backdropClose = true, foot, bare, onKeyDown, children }: ModalProps) {
  const close = () => { if (!busy) onClose(); };
  useEscape(close);
  return (
    <div className={`modal-bg${bgClassName ? ` ${bgClassName}` : ""}`} onMouseDown={(e) => { if (backdropClose && e.target === e.currentTarget) close(); }}>
      <div className={`modal${wide ? " wide" : ""}${className ? ` ${className}` : ""}`} role={role} aria-modal="true" aria-label={label} onKeyDown={onKeyDown}>
        {title !== undefined && (
          <div className="modal-head">
            <h2>{title}</h2>
            <button className="icon-btn" aria-label={t("common.close")} disabled={busy} onClick={close}><Icon.close /></button>
          </div>
        )}
        {bare ? children : <div className="modal-body">{children}</div>}
        {foot && <div className="modal-foot">{foot}</div>}
      </div>
    </div>
  );
}

/** The small question dialog (confirm, close window): icon, title, message, an optional switch, buttons. */
export function ConfirmFrame({ title, icon, danger, message, check, foot, onClose }: {
  title: string;
  icon: ReactNode;
  danger?: boolean;
  message?: ReactNode;
  check?: { label: string; hint?: ReactNode; on: boolean; onChange: (v: boolean) => void };
  foot: ReactNode;
  onClose: () => void;
}) {
  return (
    <Modal label={title} onClose={onClose} role="alertdialog" className="confirm" bgClassName="confirm-bg" bare foot={foot}>
      <div className="confirm-body">
        <span className={`confirm-icon${danger ? " danger" : ""}`}>{icon}</span>
        <div className="grow minw0">
          <div className="confirm-title">{title}</div>
          {message && <div className="confirm-msg">{message}</div>}
          {check && <ToggleRow className="confirm-check" title={check.label} hint={check.hint} on={check.on} onChange={check.onChange} />}
        </div>
      </div>
    </Modal>
  );
}
