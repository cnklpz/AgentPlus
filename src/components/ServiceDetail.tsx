import { useState } from "react";
import type { AgentId, AgentState } from "../api";
import { type Service, type Use, USE_LABEL, writableAgents } from "../services";
import { AgentIcon, Icon } from "./icons";
import { Bars, type Latency, colorFor, initials } from "./ProviderCard";

interface Props {
  s: Service;
  agents: AgentState[];
  latency: Latency;
  onClose: () => void;
  onTest: () => void;
  onCopy: (text: string) => void;
  onEdit: () => void;
  onAddTo: (agent: AgentId) => void;
  onRemove: (u: Use) => void;
  onUndo: (u: Use) => void;
  onModels: (u: Use) => void;
  /** Remove from the chosen agent entries and (optionally) the library. */
  onDelete: (uses: Use[], fromLibrary: boolean) => void;
}

function removable(u: Use): string | null {
  if (!u.p || u.state === "removing" || u.state === "adding") return "—";
  if (!u.p.editable) return "内置供应商不能删除";
  if (u.state === "current") return "正在使用，先在 Codex 里切换到别的供应商";
  return null;
}

export function ServiceDetail(props: Props) {
  const { s, agents, latency } = props;
  const [deleting, setDeleting] = useState(false);
  const deletable = s.uses.filter((u) => removable(u) === null);
  const uid = (u: Use) => `${u.agent.id}:${u.p?.id}`;
  const [pick, setPick] = useState<Set<string>>(new Set(deletable.map(uid)));
  const [fromLib, setFromLib] = useState(true);

  const lat = latency === "pending" ? { t: "测速中…", l: 0 } : typeof latency === "number" ? { t: `${latency} ms`, l: latency < 200 ? 3 : latency < 400 ? 2 : 1 } : { t: latency ? String(latency) : "未测速", l: 0 };
  const targets = writableAgents(agents);

  return (
    <section className="pdetail sdetail" aria-label={`${s.name} 详情`}>
      <div className="pdetail-head">
        <span className="pavatar" style={{ background: s.builtin ? "#121722" : colorFor({ baseUrl: s.baseUrl, host: s.host, builtin: false, id: s.key } as never) }}>{initials(s.name)}</span>
        <span className="pcard-title">
          <span className="pcard-name"><span className="ellipsis">{s.name}</span></span>
          <span className="pcard-host mono ellipsis">{s.baseUrl ?? "账号登录"}</span>
        </span>
        <button className="icon-btn" aria-label="关闭详情" onClick={props.onClose}><Icon.close /></button>
      </div>

      {s.baseUrl && (
        <div className="pdetail-lat">
          <Bars level={lat.l} />
          <span className={`grow small ${lat.l >= 2 ? "mono good-ink" : ""}`}>{lat.t}</span>
          <button className="link" onClick={props.onTest}>重新测速</button>
        </div>
      )}

      {s.baseUrl && (
        <div className="kv">
          <div className="kv-row">
            <span className="muted small">地址</span>
            <span className="row gap6 minw0">
              <span className="mono small ellipsis grow">{s.baseUrl}</span>
              <button className="icon-btn sm" aria-label="复制地址" onClick={() => props.onCopy(s.baseUrl!)}><Icon.copy size={12} /></button>
            </span>
          </div>
          <div className="kv-row">
            <span className="muted small">密钥</span>
            <span className="small">{s.lib?.hasKey ? <span className="mono">{s.lib.keyHint}</span> : s.lib ? "未设置" : "由各 Agent 各自保存"}</span>
          </div>
          <div className="kv-row">
            <span className="muted small">接口</span>
            <span className="row gap6">{(s.apis.length ? s.apis : [s.api]).map((x) => <span key={x} className="api-chip">{x === "responses" ? "Responses" : x === "chat" ? "Chat" : "Anthropic"}</span>)}</span>
          </div>
        </div>
      )}

      {!s.builtin && !deleting && (
        <div className="grid2">
          <button className="btn" onClick={props.onEdit}><Icon.edit />编辑地址与密钥</button>
          <button className="btn danger" onClick={() => { setPick(new Set(deletable.map(uid))); setDeleting(true); }}><Icon.trash />删除</button>
        </div>
      )}

      {deleting && (
        <div className="confirm-box">
          <strong className="small">从哪里删除「{s.name}」？</strong>
          {s.uses.filter((u) => u.p).map((u) => {
            const why = removable(u);
            return (
              <label key={`${u.agent.id}-${u.p!.id}`} className={`pick${why ? " dim" : ""}`} title={why ?? undefined}>
                <input type="checkbox" disabled={!!why} checked={pick.has(uid(u))}
                  onChange={() => setPick((p) => { const n = new Set(p); n.has(uid(u)) ? n.delete(uid(u)) : n.add(uid(u)); return n; })} />
                <AgentIcon id={u.agent.id} size={16} />
                <span className="small">{u.agent.name} · {u.p!.name}</span>
                {why && why !== "—" && <span className="tiny muted">（{why}）</span>}
              </label>
            );
          })}
          {s.lib && (
            <label className="pick">
              <input type="checkbox" checked={fromLib} onChange={(e) => setFromLib(e.target.checked)} />
              <Icon.key size={14} />
              <span className="small">从供应商库移除（地址和密钥）</span>
            </label>
          )}
          <div className="row gap6">
            <span className="tiny muted grow">Agent 里的删除会先加入待写入</span>
            <button className="btn small" onClick={() => setDeleting(false)}>取消</button>
            <button className="btn small danger" disabled={pick.size === 0 && !(s.lib && fromLib)}
              onClick={() => { props.onDelete(deletable.filter((u) => pick.has(uid(u))), !!s.lib && fromLib); setDeleting(false); }}>删除</button>
          </div>
        </div>
      )}

      <div className="stack6">
        <div className="row between">
          <strong className="small">在各 Agent 中</strong>
          <span className="tiny muted">模型在各 Agent 里单独设置</span>
        </div>
        <div className="uses">
          {s.uses.map((u) => (
            <div key={u.importKey ?? `${u.agent.id}-${u.p?.id}`} className={`use ${u.state}`}>
              <AgentIcon id={u.agent.id} size={22} />
              <span className="grow minw0">
                <span className="block small strong ellipsis">{u.agent.name} · {u.p?.name ?? s.name}</span>
                <span className="block tiny muted ellipsis">
                  <span className={`ustate ${u.state}`}>{USE_LABEL[u.state]}</span>
                  {u.p && ` · ${u.agent.catalog ? `模型目录 ${u.models} 个` : `${u.models} 个模型`}`}
                </span>
              </span>
              {(u.state === "adding" || u.state === "removing" || u.state === "new")
                ? <button className="btn xs" onClick={() => props.onUndo(u)}>撤销</button>
                : <>
                    {u.p && <button className="btn xs" onClick={() => props.onModels(u)}>模型</button>}
                    {u.p && removable(u) === null && <button className="icon-btn sm" aria-label={`从 ${u.agent.name} 移除`} title={`从 ${u.agent.name} 移除`} onClick={() => props.onRemove(u)}><Icon.trash size={12} /></button>}
                  </>}
            </div>
          ))}
          {!s.builtin && targets.filter((a) => !s.uses.some((u) => u.agent.id === a.id && u.state !== "removing")).map((a) => (
            <div key={a.id} className="use none">
              <AgentIcon id={a.id} size={22} />
              <span className="grow minw0">
                <span className="block small strong ellipsis">{a.name}</span>
                <span className="block tiny muted ellipsis">{a.id === "codex" ? "未接入 · 将以 Responses 接口添加" : "未接入"}</span>
              </span>
              <button className="btn xs restore" onClick={() => props.onAddTo(a.id)}><Icon.plus size={11} />添加</button>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}
