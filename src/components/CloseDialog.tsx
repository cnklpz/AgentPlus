import { useEffect, useRef, useState } from "react";
import type { CloseAction } from "../prefs";
import { t } from "../i18n";
import { useEscape } from "../hooks";
import { Icon } from "./icons";

export interface CloseChoice {
  action: Exclude<CloseAction, "ask">;
  /** Save it as the 关闭窗口时 preference and stop asking. */
  remember: boolean;
}

/** Asked when the window is closed while 设置 › 界面 › 关闭窗口时 is "每次询问". null = cancelled. */
export function CloseDialog({ onDone }: { onDone: (c: CloseChoice | null) => void }) {
  const [remember, setRemember] = useState(false);
  const trayRef = useRef<HTMLButtonElement>(null);
  useEscape(() => onDone(null));
  useEffect(() => { trayRef.current?.focus(); }, []);
  const pick = (action: CloseChoice["action"]) => onDone({ action, remember });
  return (
    <div className="modal-bg confirm-bg" onMouseDown={(e) => { if (e.target === e.currentTarget) onDone(null); }}>
      <div className="modal confirm" role="alertdialog" aria-modal="true" aria-label={t("closeDialog.title")}>
        <div className="confirm-body">
          <span className="confirm-icon"><Icon.power size={16} /></span>
          <div className="grow minw0">
            <div className="confirm-title">{t("closeDialog.title")}</div>
            <div className="confirm-msg">{t("closeDialog.message")}</div>
            <div className={`gw-toggle confirm-check${remember ? " on" : ""}`}>
              <div className="grow minw0">
                <div className="small strong">{t("closeDialog.remember")}</div>
                <div className="tiny muted">{t("closeDialog.rememberHint")}</div>
              </div>
              <button type="button" className={`switch${remember ? " on" : ""}`} role="switch" aria-checked={remember} aria-label={t("closeDialog.remember")}
                onClick={() => setRemember((v) => !v)}><span /></button>
            </div>
          </div>
        </div>
        <div className="modal-foot">
          <button className="btn" onClick={() => onDone(null)}>{t("common.cancel")}</button>
          <span className="grow" />
          <button className="btn" onClick={() => pick("quit")}>{t("common.quitApp")}</button>
          <button ref={trayRef} className="btn primary" onClick={() => pick("tray")}>{t("common.minimizeToTray")}</button>
        </div>
      </div>
    </div>
  );
}
