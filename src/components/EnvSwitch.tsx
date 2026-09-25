import { useRef, useState } from "react";
import { usePopover } from "../hooks";
import type { EnvInfo } from "../api";
import { EnvIcon, Icon } from "./icons";
import { t } from "../i18n";
import { scrub } from "../privacy";
import { localEnvLabel } from "../platform";

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

  // Esc closes the menu alone, not the settings page or detail panel underneath.
  usePopover(open, () => setOpen(false), [root]);

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
        <EnvIcon id={current?.id ?? ""} />
        <span>{switching ? t("envSwitch.switching") : current?.label ?? localEnvLabel()}</span>
        <Icon.chevron />
      </button>
      {open && (
        <div className="dd-menu env-menu" role="menu">
          <div className="env-menu-head tiny muted">{t("envSwitch.menuHead")}</div>
          {envs.map((e) => (
            <button key={e.id} role="menuitemradio" aria-checked={e.current} className={`dd-item env-item${e.current ? " sel" : ""}`}
              onClick={() => { setOpen(false); if (!e.current) onPick(e.id); }}>
              <span className="env-ico"><EnvIcon id={e.id} /></span>
              <span className="grow minw0">
                <span className="block ellipsis">{e.label}</span>
                <span className="block tiny muted ellipsis">{scrub(e.detail)}</span>
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
