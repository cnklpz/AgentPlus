import { type ReactNode, useEffect, useState } from "react";
import { type AgentId, type AgentState, type DiffGroup, api } from "../api";
import { type Draft, opsToWrite } from "../draft";
import type { Service } from "../services";
import { AgentIcon, Icon } from "./icons";

interface Props {
  agents: AgentState[];
  drafts: Record<string, Draft>;
  services: Service[];
  detail: ReactNode | null;
  busy: boolean;
  onDiscard: (agent: AgentId | null) => void;
  onApplyAll: () => void;
}

/** Hub page right column: service details on top, every agent's pending changes below. */
export function HubAside({ agents, drafts, services, detail, busy, onDiscard, onApplyAll }: Props) {
  const withOps = agents.filter((a) => Object.keys(drafts[a.id] ?? {}).length > 0);
  const total = withOps.reduce((n, a) => n + Object.keys(drafts[a.id]).length, 0);
  const [diffs, setDiffs] = useState<Record<string, DiffGroup[] | string>>({});

  useEffect(() => {
    let alive = true;
    for (const a of withOps) {
      api.preview(a.id, opsToWrite(a, drafts[a.id]))
        .then((d) => alive && setDiffs((m) => ({ ...m, [a.id]: d })))
        .catch((e) => alive && setDiffs((m) => ({ ...m, [a.id]: String(e) })));
    }
    return () => { alive = false; };
  }, [drafts, agents]);

  const api_ = services.filter((s) => !s.builtin);
  const inLib = api_.filter((s) => s.lib).length;

  return (
    <aside className="aside" aria-label="供应商详情与改动">
      {detail ?? (
        <section className="aside-cur">
          <div className="row between">
            <h2>供应商库</h2>
            <span className="muted tiny">保存在 ~/.agentplus</span>
          </div>
          <div className="hub-stats">
            <div><b>{api_.length}</b><span>个供应商</span></div>
            <div><b>{inLib}</b><span>已收录密钥</span></div>
            <div><b>{agents.filter((a) => a.installed && !a.readonly).length}</b><span>个 Agent 可用</span></div>
          </div>
          <span className="muted tiny">点一张卡片查看它在各 Agent 里的情况，可以一键添加、同步地址和密钥或移除。</span>
        </section>
      )}

      <section className="aside-diff">
        <div className="row between">
          <h2>待写入的改动</h2>
          <span className={`count${total ? " warn" : ""}`}>{total ? `${withOps.length} 个 Agent · ${total} 项` : "无"}</span>
        </div>
        {withOps.map((a) => {
          const d = diffs[a.id];
          return (
            <div key={a.id} className="hub-agent">
              <div className="row gap6">
                <AgentIcon id={a.id} size={18} />
                <strong className="small grow">{a.name}</strong>
                <span className="tiny muted">{Object.keys(drafts[a.id]).length} 项</span>
                <button className="link" onClick={() => onDiscard(a.id)}>放弃</button>
              </div>
              {typeof d === "string" && <div className="err">{d}</div>}
              {Array.isArray(d) && d.map((g) => (
                <div key={g.file} className="dgroup">
                  <div className="dfile mono ellipsis">{g.file}</div>
                  {g.lines.map((l, i) => <div key={i} className={`dline mono ${l.add ? "add" : "del"}`}>{l.text}</div>)}
                </div>
              ))}
            </div>
          );
        })}
        {total === 0 && (
          <div className="dempty">
            <Icon.check size={20} color="#16A34A" />
            <strong>配置已是最新</strong>
            <span className="muted small">添加、同步或移除供应商后，这里按 Agent 列出将写入的内容</span>
          </div>
        )}
      </section>

      <div className="aside-foot">
        <div className="grid2">
          <button className="btn full" disabled={!total || busy} onClick={() => onDiscard(null)}>全部放弃</button>
          <button className="btn primary full" disabled={!total || busy} onClick={onApplyAll}>{busy ? "写入中…" : withOps.length > 1 ? `应用到 ${withOps.length} 个 Agent` : "应用"}</button>
        </div>
        <span className="muted tiny center">写入前自动备份原文件</span>
      </div>
    </aside>
  );
}
