import type { AgentId, AgentState } from "../api";
import { type Draft, currentProvider, isEnabled, visibleCount } from "../draft";
import { AgentIcon, Icon } from "./icons";

interface Props {
  agents: AgentState[];
  drafts: Record<string, Draft>;
  selected: AgentId;
  onSelect: (id: AgentId) => void;
}

function subline(a: AgentState, d: Draft): string {
  if (!a.installed) return "未检测到安装";
  const n = visibleCount(a, d);
  if (a.mode === "single") return `供应商 ${currentProvider(a, d) ?? "-"} · ${n} 个模型`;
  const on = a.providers.filter((p) => isEnabled(p, d)).length;
  return `${on} 个供应商 · ${n} 个模型`;
}

export function Sidebar({ agents, drafts, selected, onSelect }: Props) {
  const detected = agents.filter((a) => a.installed).length;
  return (
    <nav className="sidebar" aria-label="导航">
      <div className="side-label">AGENT</div>
      {agents.map((a) => {
        const d = drafts[a.id] ?? {};
        const fast = a.id === "codex" && a.settings.find((s) => s.key === "fast_inject")?.value === true;
        const dirty = Object.keys(d).length > 0;
        return (
          <button key={a.id} className={`agent-row${a.id === selected ? " active" : ""}`} onClick={() => onSelect(a.id)}>
            <AgentIcon id={a.id} size={32} />
            <span className="agent-row-text">
              <span className="agent-row-name">
                {a.name}
                {fast && <span className="badge-fast">FAST</span>}
              </span>
              <span className="agent-row-sub">{subline(a, d)}</span>
            </span>
            {dirty && <span className="dot-dirty" title="有未应用的改动" />}
          </button>
        );
      })}

      <div className="side-label spaced">即将支持</div>
      <button className="agent-row disabled" disabled title="计划在后续版本支持">
        <span className="mono-tile">CC</span>
        <span className="agent-row-text">
          <span className="agent-row-name muted">Claude Code</span>
          <span className="agent-row-sub muted">计划中 · 暂不可配置</span>
        </span>
      </button>

      <div className="side-label spaced">资源</div>
      <button className="side-link" disabled title="即将推出"><Icon.layers />服务商与模型</button>
      <button className="side-link" disabled title="即将推出"><Icon.history />历史与回滚</button>
      <button className="side-link" disabled title="即将推出"><Icon.cloud />多设备同步</button>

      <div className="side-foot">
        <strong>已检测 {detected} 个 Agent</strong>
        <span>备份保存在 ~/.agentplus/backups</span>
      </div>
    </nav>
  );
}
