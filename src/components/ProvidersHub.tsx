import { useMemo, useState } from "react";
import type { AgentState } from "../api";
import { type Service, USE_LABEL, writableAgents } from "../services";
import { AgentIcon, Icon } from "./icons";
import { Bars, type Latency, colorFor, initials } from "./ProviderCard";

interface Props {
  agents: AgentState[];
  services: Service[];
  latency: Record<string, Latency>;
  selected: string | null;
  onSelect: (key: string | null) => void;
  onAdd: () => void;
  onTestAll: () => void;
  onTestOne: (url: string) => void;
  envLabel: string;
}

type Filter = "all" | "used" | "idle";

function latencyText(l: Latency): { text: string; level: number } {
  if (l === "pending") return { text: "测速中…", level: 0 };
  if (typeof l === "number") return { text: `${l} ms`, level: l < 200 ? 3 : l < 400 ? 2 : 1 };
  return { text: l ? "不可达" : "未测速", level: 0 };
}

/** Every provider in one place: address and key live here, model lists live in each agent. */
export function ProvidersHub({ agents, services, latency, selected, onSelect, onAdd, onTestAll, onTestOne, envLabel }: Props) {
  const [q, setQ] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const shown = writableAgents(agents);
  const slots = agents.filter((a) => a.installed);

  const api = services.filter((s) => !s.builtin);
  const accounts = services.filter((s) => s.builtin);
  const used = (s: Service) => s.uses.some((u) => u.state !== "removing");

  const list = useMemo(() => {
    const t = q.trim().toLowerCase();
    return api.filter((s) => {
      if (filter === "used" && !used(s)) return false;
      if (filter === "idle" && used(s)) return false;
      return !t || s.name.toLowerCase().includes(t) || (s.baseUrl ?? "").toLowerCase().includes(t)
        || s.uses.some((u) => u.p?.name.toLowerCase().includes(t));
    });
  }, [api, q, filter]);

  const counts = { all: api.length, used: api.filter(used).length, idle: api.filter((s) => !used(s)).length };

  return (
    <main className="page">
      <div className="page-top">
        <div className="page-head">
          <div className="page-title">
            <h1>供应商</h1>
            <span className="muted small">
              所有 Agent 共用一份供应商：地址和密钥在这里统一维护，每个 Agent 用哪些模型在它自己的「模型列表」里设置。当前环境：{envLabel}
            </span>
          </div>
          <button className="btn" onClick={onTestAll}><Icon.pulse />测试延迟</button>
          <button className="btn primary" onClick={onAdd}><Icon.plus />添加供应商</button>
        </div>
        <div className="toolbar">
          <div className="seg" role="tablist" aria-label="筛选">
            {([["all", "全部"], ["used", "已接入"], ["idle", "未使用"]] as [Filter, string][]).map(([id, label]) => (
              <button key={id} role="tab" aria-selected={filter === id} className={filter === id ? "on" : ""} onClick={() => setFilter(id)}>
                {label}<b>{counts[id]}</b>
              </button>
            ))}
          </div>
          <label className="search-box">
            <Icon.search size={13} />
            <input value={q} onChange={(e) => setQ(e.target.value)} placeholder="按名称、地址搜索" />
          </label>
        </div>
      </div>

      <div className="page-body">
        <div className="hgrid">
          {list.map((s, i) => {
            const lat = s.baseUrl ? latencyText(latency[s.baseUrl]) : { text: "", level: 0 };
            return (
              <section
                key={s.key}
                className={`hcard${selected === s.key ? " selected" : ""}`}
                style={{ ["--i" as string]: Math.min(i, 12) }}
                tabIndex={0}
                role="button"
                aria-pressed={selected === s.key}
                onClick={() => onSelect(selected === s.key ? null : s.key)}
                onKeyDown={(e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); onSelect(selected === s.key ? null : s.key); } }}
              >
                <div className="hcard-head">
                  <span className="pavatar" style={{ background: colorFor({ baseUrl: s.baseUrl, host: s.host, builtin: false, id: s.key } as never) }}>{initials(s.name)}</span>
                  <span className="pcard-title">
                    <span className="pcard-name"><span className="ellipsis">{s.name}</span></span>
                    <span className="pcard-host mono ellipsis">{s.baseUrl}</span>
                  </span>
                  {s.baseUrl && (
                    <button className={`hlat lat-btn${lat.level >= 2 ? " good" : lat.level === 1 ? " slow" : ""}`} title="点一下重新测速"
                      disabled={latency[s.baseUrl] === "pending"}
                      onClick={(e) => { e.stopPropagation(); onTestOne(s.baseUrl!); }} onKeyDown={(e) => e.stopPropagation()}>
                      <Bars level={lat.level} />{lat.text}<span className="lat-re" aria-hidden="true">↻</span>
                    </button>
                  )}
                </div>
                <div className="hslots">
                  {slots.map((a) => {
                    const mine = s.uses.filter((u) => u.agent.id === a.id);
                    const live = mine.filter((u) => u.state !== "removing");
                    const st = live.find((u) => u.state === "current") ?? live.find((u) => u.state === "adding" || u.state === "new") ?? live[0] ?? mine[0];
                    const cls = !st ? "none" : st.state;
                    const label = !st ? "未接入"
                      : st.state === "adding" || st.state === "new" || st.state === "removing" ? USE_LABEL[st.state]
                      : live.length > 1 ? `${live.length} 处` : USE_LABEL[st.state];
                    return (
                      <span key={a.id} className={`hslot ${cls}`} title={mine.map((u) => `${a.name} · ${u.p?.name ?? s.name}（${USE_LABEL[u.state]}）`).join("\n") || `${a.name}：未接入`}>
                        <AgentIcon id={a.id} size={18} />
                        <span className="ellipsis">{label}</span>
                      </span>
                    );
                  })}
                </div>
                <div className="hcard-foot">
                  {s.apis.map((x) => <span key={x} className="api-chip">{x === "responses" ? "Responses" : x === "chat" ? "Chat" : "Anthropic"}</span>)}
                  <span className="grow" />
                  {s.lib ? <span className="hmeta" title="已保存在供应商库，可添加到任意 Agent"><Icon.key size={12} />已收录</span>
                    : <span className="hmeta faint" title="只存在于 Agent 配置里，编辑后会收录进供应商库">未收录</span>}
                </div>
              </section>
            );
          })}
          {filter !== "used" && !q && (
            <button className="hcard-add" onClick={onAdd} style={{ ["--i" as string]: Math.min(list.length, 12) }}>
              <Icon.plus size={16} />
              <strong>添加供应商</strong>
              <span className="tiny muted">填一次地址和密钥，按需添加到 {shown.map((a) => a.name).join(" / ") || "各 Agent"}</span>
            </button>
          )}
        </div>
        {list.length === 0 && (q || filter !== "all") && <div className="empty">没有匹配的供应商</div>}

        {accounts.length > 0 && filter === "all" && !q && (
          <>
            <div className="section-label">账号登录 · 各 Agent 内置</div>
            <div className="acct-row">
              {accounts.map((s) => (
                <button key={s.key} className={`acct${selected === s.key ? " selected" : ""}`} onClick={() => onSelect(selected === s.key ? null : s.key)}>
                  <AgentIcon id={s.uses[0].agent.id} size={22} />
                  <span className="grow minw0">
                    <span className="block small strong ellipsis">{s.name}</span>
                    <span className="block tiny muted ellipsis">{s.uses[0].agent.name} · {USE_LABEL[s.uses[0].state]}</span>
                  </span>
                </button>
              ))}
            </div>
          </>
        )}
      </div>
    </main>
  );
}
