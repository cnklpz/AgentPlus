import { useEffect, useRef, useState } from "react";
import type { EnvInfo } from "../api";
import { Icon } from "./icons";

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
        title="切换要管理的环境"
        onClick={() => { if (!open) onOpen(); setOpen((o) => !o); }}
      >
        {wsl ? <Icon.terminal /> : <Icon.monitor />}
        <span>{switching ? "切换中…" : current?.label ?? "本机 · Windows"}</span>
        <Icon.chevron />
      </button>
      {open && (
        <div className="dd-menu env-menu" role="menu">
          <div className="env-menu-head tiny muted">管理哪里的配置</div>
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
          {envs.length <= 1 && <div className="tiny muted env-empty">没有检测到 WSL 发行版</div>}
        </div>
      )}
    </div>
  );
}
