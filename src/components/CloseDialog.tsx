import { useEffect, useRef, useState } from "react";
import type { CloseAction } from "../prefs";
import { t } from "../i18n";
import { Icon } from "./icons";
import { ConfirmFrame } from "./Modal";

export interface CloseChoice {
  action: Exclude<CloseAction, "ask">;
  /** Save it as the 关闭窗口时 preference and stop asking. */
  remember: boolean;
}

/** Asked when the window is closed while 设置 › 界面 › 关闭窗口时 is "每次询问". null = cancelled. */
export function CloseDialog({ onDone }: { onDone: (c: CloseChoice | null) => void }) {
  const [remember, setRemember] = useState(false);
  const trayRef = useRef<HTMLButtonElement>(null);
  useEffect(() => { trayRef.current?.focus(); }, []);
  const pick = (action: CloseChoice["action"]) => onDone({ action, remember });
  return (
    <ConfirmFrame
      title={t("closeDialog.title")}
      icon={<Icon.power size={16} />}
      message={t("closeDialog.message")}
      check={{ label: t("closeDialog.remember"), hint: t("closeDialog.rememberHint"), on: remember, onChange: setRemember }}
      onClose={() => onDone(null)}
      foot={<>
        <button className="btn" onClick={() => onDone(null)}>{t("common.cancel")}</button>
        <span className="grow" />
        <button className="btn" onClick={() => pick("quit")}>{t("common.quitApp")}</button>
        <button ref={trayRef} className="btn primary" onClick={() => pick("tray")}>{t("common.minimizeToTray")}</button>
      </>}
    />
  );
}
