import type { AgentId, AgentState } from "../api";
import { type Draft, currentProvider, isEnabled, visibleCount } from "../draft";
import { AgentIcon, Icon } from "./icons";

export type Page = "providers" | "history" | "sync" | "settings";

interface Props {
  agents: AgentState[];
  drafts: Record<string, Draft>;
  /** Selected agent when an agent page is open, else null. */
  selected: AgentId | null;
  page: Page | null;
  onSelect: (id: AgentId) => void;
  onPage: (p: Page) => void;
}

function subline(a: AgentState, d: Draft): string {
  if (!a.installed) return "未检测到安装";
  const n = visibleCount(a, d);
  if (a.mode === "single") return `供应商 ${currentProvider(a, d) ?? "-"} · ${n} 个模型`;
  const on = a.providers.filter((p) => isEnabled(p, d)).length;
  return `${on} 个供应商 · ${n} 个模型`;
}

export function Sidebar({ agents, drafts, selected, page, onSelect, onPage }: Props) {
  const detected = agents.filter((a) => a.installed).length;
  const pending = Object.values(drafts).reduce((n, d) => n + Object.keys(d).length, 0);
  const links: [Page, string, JSX.Element][] = [
    ["providers", "供应商", <Icon.layers key="l" />],
    ["history", "历史与回滚", <Icon.history key="h" />],
    ["sync", "多设备同步", <Icon.cloud key="c" />],
  ];
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
      {links.map(([id, label, icon]) => (
        <button key={id} className={`side-link${page === id ? " active" : ""}`} onClick={() => onPage(id)}>{icon}{label}</button>
      ))}

      <div className="side-foot">
        <strong>已检测 {detected} 个 Agent</strong>
        <span>{pending ? `${pending} 项改动未应用` : "备份保存在 ~/.agentplus/backups"}</span>
      </div>
    </nav>
  );
}
