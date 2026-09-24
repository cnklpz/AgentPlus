import { useEffect, useRef, useState } from "react";
import type { EnvInfo } from "../api";
import { Icon } from "./icons";
import { t } from "../i18n";

interface Props {
  envs: EnvInfo[];
  current: EnvInfo | undefined;
  switching: boolean;
  onOpen: () => void;
  onPick: (id: string) => void;
}

/** Top-bar picker for where configs are read and written: Windows or a WSL distro. */
export function EnvSwitch({ envs, current, switching, onOpen, onPick }: Props) {
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => { if (!root.current?.contains(e.target as Node)) setOpen(false); };
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") setOpen(false); };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const wsl = current?.id.startsWith("wsl:");
  return (
    <div className="dd env" ref={root}>
      <button
        className={`env-btn${open ? " open" : ""}${wsl ? " wsl" : ""}`}
        aria-haspopup="menu"
        aria-expanded={open}
        title={t("envSwitch.switchTitle")}
        onClick={() => { if (!open) onOpen(); setOpen((o) => !o); }}
      >
        {wsl ? <Icon.terminal /> : <Icon.monitor />}
        <span>{switching ? t("envSwitch.switching") : current?.label ?? t("envSwitch.localWindows")}</span>
        <Icon.chevron />
      </button>
      {open && (
        <div className="dd-menu env-menu" role="menu">
          <div className="env-menu-head tiny muted">{t("envSwitch.menuHead")}</div>
          {envs.map((e) => (
            <button key={e.id} role="menuitemradio" aria-checked={e.current} className={`dd-item env-item${e.current ? " sel" : ""}`}
              onClick={() => { setOpen(false); if (!e.current) onPick(e.id); }}>
              <span className="env-ico">{e.id.startsWith("wsl:") ? <Icon.terminal /> : <Icon.monitor />}</span>
              <span className="grow minw0">
                <span className="block ellipsis">{e.label}</span>
                <span className="block tiny muted ellipsis">{e.detail}</span>
              </span>
              {e.current && <Icon.check size={13} />}
            </button>
          ))}
          {envs.length <= 1 && <div className="tiny muted env-empty">{t("envSwitch.noWsl")}</div>}
        </div>
      )}
    </div>
  );
}
