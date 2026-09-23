import { useEffect, useState } from "react";
import { type AgentDetect, type AgentId, type EnvInfo, api } from "../api";
import type { Motion, Prefs } from "../prefs";
import { AgentIcon, Icon } from "./icons";

export type SettingsTab = "general" | "agents";

interface Props {
  tab: SettingsTab;
  setTab: (t: SettingsTab) => void;
  prefs: Prefs;
  setPrefs: (p: Prefs) => void;
  envs: EnvInfo[];
  switching: boolean;
  onEnv: (id: string) => void;
  onHistory: () => void;
  /** Detection or a folder changed: reload agents. */
  onAgentsChanged: () => void;
  flash: (text: string, error?: boolean) => void;
  /** Back to where the user came from. */
  onClose: () => void;
}

const MOTION: { v: Motion; label: string; hint: string }[] = [
  { v: "full", label: "标准", hint: "页面、卡片和弹窗带轻微的过渡" },
  { v: "reduced", label: "减少", hint: "只保留淡入淡出，不做位移" },
  { v: "off", label: "关闭", hint: "不使用任何动画" },
];

/** AgentPlus's own settings (top-right gear): general options and agent detection. */
export function SettingsPage(props: Props) {
  const { tab, setTab } = props;
  const env = props.envs.find((e) => e.current);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target instanceof Element ? e.target : null;
      if (e.key === "Escape" && !t?.closest("input, .modal-bg")) props.onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [props.onClose]);
  return (
    <main className="page settings-page">
      <div className="page-top">
        <div className="page-head">
          <span className="page-icon"><Icon.gear size={20} /></span>
          <div className="page-title">
            <h1>AgentPlus 设置</h1>
            <span className="muted small">AgentPlus 自己的选项；各 Agent 的配置在它们自己的页面里</span>
          </div>
          <button className="icon-btn" aria-label="关闭设置" title="关闭（Esc）" onClick={props.onClose}><Icon.close /></button>
        </div>
        <div className="tabs" role="tablist">
          {([["general", "通用"], ["agents", "Agent 识别"]] as [SettingsTab, string][]).map(([id, label]) => (
            <button key={id} role="tab" aria-selected={tab === id} className={`tab${tab === id ? " on" : ""}`} onClick={() => setTab(id)}>{label}</button>
          ))}
        </div>
      </div>
      <div className="page-body" key={tab}>
        {tab === "general" ? <General {...props} /> : <Detection envLabel={env?.label ?? "本机 · Windows"} onChanged={props.onAgentsChanged} flash={props.flash} />}
      </div>
    </main>
  );
}

function General({ prefs, setPrefs, envs, switching, onEnv, onHistory, flash }: Props) {
  return (
    <div className="settings">
      <section className="sgroup">
        <h2>管理的环境</h2>
        <div className="srow stacked">
          <div className="env-cards row-cards">
            {envs.map((e) => (
              <button key={e.id} className={`env-card${e.current ? " on" : ""}`} disabled={switching} onClick={() => !e.current && onEnv(e.id)}>
                <span className="env-ico">{e.id.startsWith("wsl:") ? <Icon.terminal /> : <Icon.monitor />}</span>
                <span className="grow minw0">
                  <span className="block small strong">{e.label}</span>
                  <span className="block tiny muted ellipsis">{e.detail}</span>
                </span>
                {e.current && <Icon.check size={14} color="var(--accent)" />}
              </button>
            ))}
          </div>
          <span className="muted tiny">供应商库在各环境间共用：在 Windows 添加的供应商，切到 WSL 后可以直接加到 Codex CLI。</span>
        </div>
      </section>

      <section className="sgroup">
        <h2>界面</h2>
        <div className="srow">
          <div className="grow minw0">
            <div className="slabel">动画效果</div>
            <div className="muted small">{MOTION.find((m) => m.v === prefs.motion)?.hint}；系统开启「减少动画」时自动减弱</div>
          </div>
          <div className="seg">
            {MOTION.map((m) => (
              <button key={m.v} className={prefs.motion === m.v ? "on" : ""} title={m.hint} onClick={() => setPrefs({ ...prefs, motion: m.v })}>{m.label}</button>
            ))}
          </div>
        </div>
        <div className="srow">
          <div className="grow minw0">
            <div className="slabel">打开页面时自动测速</div>
            <div className="muted small">对供应商地址发一次请求，测量响应时间</div>
          </div>
          <button className={`switch${prefs.autoLatency ? " on" : ""}`} role="switch" aria-checked={prefs.autoLatency} aria-label="打开页面时自动测速"
            onClick={() => setPrefs({ ...prefs, autoLatency: !prefs.autoLatency })}><span /></button>
        </div>
      </section>

      <section className="sgroup">
        <h2>数据与备份</h2>
        <div className="srow">
          <div className="grow minw0">
            <div className="slabel">AgentPlus 数据目录</div>
            <div className="muted small mono">~/.agentplus · 供应商库、各 Agent 的开关、写入前的备份</div>
          </div>
          <button className="btn" onClick={() => api.openDataDir().catch((e) => flash(String(e), true))}><Icon.folder />打开</button>
          <button className="btn" onClick={onHistory}><Icon.history size={14} />备份与回滚</button>
        </div>
      </section>

      <section className="sgroup">
        <h2>关于</h2>
        <div className="srow">
          <div className="grow minw0">
            <div className="slabel">AgentPlus 0.1.0</div>
            <div className="muted small">管理 Codex、ZCode、MiMo Desktop 的供应商和模型列表</div>
          </div>
        </div>
      </section>
    </div>
  );
}

function Detection({ envLabel, onChanged, flash }: { envLabel: string; onChanged: () => void; flash: (t: string, e?: boolean) => void }) {
  const [list, setList] = useState<AgentDetect[] | null>(null);
  const [edit, setEdit] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState<string | null>(null);

  const load = () => api.detectAgents().then(setList).catch((e) => flash(String(e), true));
  useEffect(() => { load(); }, [envLabel]);

  const save = async (id: AgentId, path: string | null) => {
    setSaving(id);
    try {
      await api.setAgentDir(id, path);
      setEdit((m) => { const n = { ...m }; delete n[id]; return n; });
      await load();
      onChanged();
      flash(path ? "已使用这个目录" : "已恢复默认目录");
    } catch (e) {
      flash(String(e), true);
    } finally {
      setSaving(null);
    }
  };

  return (
    <div className="settings">
      <div className="row between">
        <span className="muted small">在「{envLabel}」里找到的 Agent。没识别到的不会出现在左侧；装在别处的可以手动指定配置目录。</span>
        <button className="btn small" onClick={() => { setList(null); load(); }}><Icon.refresh size={12} />重新检测</button>
      </div>
      {!list && <div className="empty">正在检测…</div>}
      {list?.map((d) => {
        const draft = edit[d.id];
        return (
          <section key={d.id} className={`sgroup detect${d.enabled ? "" : " off"}`}>
            <div className="detect-head">
              <AgentIcon id={d.id} size={36} />
              <div className="grow minw0">
                <div className="slabel">{d.name}{d.version && <span className="tiny muted mono">{d.version}</span>}</div>
                <div className="muted small">
                  {d.appFound ? (d.running ? "已安装 · 运行中" : "已安装") : "没有找到程序"}
                  {" · "}{d.configFound ? "找到配置" : "没有配置文件"}
                </div>
              </div>
              <span className={d.enabled ? "chip-ok" : "chip-muted"}>{d.enabled ? "已识别" : "未识别"}</span>
            </div>
            {d.note && <div className="detect-note tiny">{d.note}</div>}
            <div className="srow stacked">
              <div className="row gap6">
                <span className="small strong">配置目录</span>
                <span className={`ptag ${d.customDir ? "tag-new" : "tag-soft"}`}>{d.customDir ? "手动指定" : "默认"}</span>
                <span className="grow" />
                <button className="link" onClick={() => api.openPath(d.configDir).catch((e) => flash(String(e), true))}>打开</button>
              </div>
              {draft === undefined ? (
                <div className="row gap6">
                  <span className="mono small grow ellipsis dir-line" title={d.configDir}>{d.configDir}</span>
                  <button className="btn small" onClick={() => setEdit((m) => ({ ...m, [d.id]: d.customDir ?? d.configDir }))}><Icon.edit size={12} />更改</button>
                  {d.customDir && <button className="btn small" disabled={saving === d.id} onClick={() => save(d.id, null)}>恢复默认</button>}
                </div>
              ) : (
                <div className="row gap6">
                  <input className="input mono grow" autoFocus value={draft} placeholder={d.defaultDir}
                    onChange={(e) => setEdit((m) => ({ ...m, [d.id]: e.target.value }))}
                    onKeyDown={(e) => { if (e.key === "Enter") save(d.id, draft); if (e.key === "Escape") setEdit((m) => { const n = { ...m }; delete n[d.id]; return n; }); }} />
                  <button className="btn small" onClick={() => setEdit((m) => { const n = { ...m }; delete n[d.id]; return n; })}>取消</button>
                  <button className="btn small primary" disabled={!draft.trim() || saving === d.id} onClick={() => save(d.id, draft)}>{saving === d.id ? "检查中…" : "使用"}</button>
                </div>
              )}
              {draft !== undefined && <span className="tiny muted">可以填 Windows 路径、\\wsl.localhost\… 路径，在 WSL 环境里也可以直接填 /home/… 。默认：{d.defaultDir}</span>}
            </div>
          </section>
        );
      })}
      {list && (
        <section className="sgroup detect off">
          <div className="detect-head">
            <span className="mono-tile" style={{ width: 36, height: 36 }}>CC</span>
            <div className="grow minw0">
              <div className="slabel">Claude Code</div>
              <div className="muted small">计划在后续版本支持</div>
            </div>
            <span className="chip-muted">即将支持</span>
          </div>
        </section>
      )}
    </div>
  );
}
