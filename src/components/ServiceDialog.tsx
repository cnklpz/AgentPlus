import { useEffect, useRef, useState } from "react";
import { type AgentId, type AgentState, type ApiKind, api } from "../api";
import { type Service, type Use, writableAgents } from "../services";
import { AgentIcon, Icon } from "./icons";

export interface ServiceSave {
  name: string;
  baseUrl: string;
  api: ApiKind;
  apiKey: string | null;
  models: string[];
  /** Existing agent entries to update with the new address / key. */
  sync: Use[];
  /** Agents to add this provider to. */
  addTo: AgentId[];
}

interface Props {
  agents: AgentState[];
  /** null = add a new provider. */
  service: Service | null;
  onSave: (v: ServiceSave) => Promise<void>;
  onClose: () => void;
}

const API_OPTIONS: { v: ApiKind; label: string; hint: string }[] = [
  { v: "responses", label: "Responses", hint: "OpenAI Responses 接口（/v1/responses），Codex 只支持这种" },
  { v: "chat", label: "Chat", hint: "OpenAI 兼容 Chat Completions（/v1/chat/completions）" },
  { v: "anthropic", label: "Anthropic", hint: "Anthropic Messages 接口（/v1/messages）" },
];

export function ServiceDialog({ agents, service, onSave, onClose }: Props) {
  const isNew = !service;
  const [name, setName] = useState(service?.name ?? "");
  const [baseUrl, setBaseUrl] = useState(service?.baseUrl ?? "");
  const [kind, setKind] = useState<ApiKind>(service?.api ?? "responses");
  const [key, setKey] = useState("");
  const [models, setModels] = useState<string[]>(service?.lib?.models ?? []);
  const [fetched, setFetched] = useState<string[] | null>(null);
  const [manual, setManual] = useState("");
  const [fetching, setFetching] = useState(false);
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const first = useRef<HTMLInputElement>(null);

  const editable = (service?.uses ?? []).filter((u) => u.p && u.p.editable && !u.p.isNew && u.state !== "removing");
  const uid = (u: Use) => `${u.agent.id}:${u.p!.id}`;
  const [sync, setSync] = useState<Set<string>>(new Set(editable.map(uid)));
  const free = writableAgents(agents).filter((a) => !(service?.uses ?? []).some((u) => u.agent.id === a.id && u.state !== "removing"));
  const [addTo, setAddTo] = useState<Set<AgentId>>(new Set());

  useEffect(() => {
    first.current?.focus();
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  const url = baseUrl.trim().replace(/\/+$/, "");
  const urlOk = /^https?:\/\/\S+$/.test(url);
  const changedAddr = !!service && url !== (service.baseUrl ?? "").replace(/\/+$/, "");
  const changedKey = key.trim() !== "";
  const canSave = name.trim() !== "" && urlOk && !saving;

  const fetchList = async () => {
    setErr(null);
    setFetching(true);
    try {
      const src = editable[0];
      const list = !key.trim() && src && !changedAddr
        ? await api.fetchModels(src.agent.id, src.p!.id)
        : await api.fetchModelsUrl(url, key.trim() || null, kind);
      setFetched(list);
      if (models.length === 0) setModels(list.slice(0, 20));
    } catch (e) {
      setErr(`拉取失败：${e}`);
    } finally {
      setFetching(false);
    }
  };

  const toggle = (m: string) => setModels((l) => (l.includes(m) ? l.filter((x) => x !== m) : [...l, m]));
  const addManual = () => {
    const ids = manual.split(/[\s,]+/).map((s) => s.trim()).filter(Boolean);
    setModels((l) => [...l, ...ids.filter((i) => !l.includes(i))]);
    setManual("");
  };

  const save = async () => {
    if (!canSave) return;
    setSaving(true);
    setErr(null);
    try {
      await onSave({
        name: name.trim(), baseUrl: url, api: kind, apiKey: key.trim() || null, models,
        sync: changedAddr || changedKey ? editable.filter((u) => sync.has(uid(u))) : [],
        addTo: [...addTo],
      });
    } catch (e) {
      setErr(String(e));
      setSaving(false);
    }
  };

  const all = [...new Set([...(fetched ?? []), ...models])];

  return (
    <div className="modal-bg" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="modal wide" role="dialog" aria-modal="true" aria-label={isNew ? "添加供应商" : "编辑供应商"}>
        <div className="modal-head">
          <h2>{isNew ? "添加供应商" : `编辑「${service!.name}」`}</h2>
          <button className="icon-btn" aria-label="关闭" onClick={onClose}><Icon.close /></button>
        </div>

        <div className="modal-body">
          <div className="form2">
            <label className="field">
              <span>名称</span>
              <input ref={first} className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="例如：中转 A" />
            </label>
            <label className="field">
              <span>地址（Base URL）</span>
              <input className="input mono" value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="https://api.example.com/v1" />
              {baseUrl && !urlOk && <em className="field-err">需要以 http:// 或 https:// 开头</em>}
            </label>
          </div>
          <div className="form2">
            <div className="field">
              <span>默认接口类型</span>
              <div className="seg">
                {API_OPTIONS.map((o) => (
                  <button key={o.v} type="button" className={kind === o.v ? "on" : ""} title={o.hint} onClick={() => setKind(o.v)}>{o.label}</button>
                ))}
              </div>
              <em className="muted tiny">添加到 Codex 时总是用 Responses。</em>
            </div>
            <label className="field">
              <span>API Key</span>
              <input className="input mono" type="password" autoComplete="off" value={key} onChange={(e) => setKey(e.target.value)}
                placeholder={service?.lib?.hasKey || editable.some((u) => u.p!.hasKey) ? "已设置，留空表示不修改" : "sk-..."} />
              <em className="muted tiny">保存在本机 ~/.agentplus，写入各 Agent 时按它们自己的格式保存。</em>
            </label>
          </div>

          <div className="field">
            <div className="row between">
              <span>常用模型 <em className="muted tiny">（添加到 ZCode / MiMo 时作为初始模型列表）</em></span>
              <button type="button" className="btn small" disabled={!urlOk || fetching} onClick={fetchList}>
                <Icon.refresh size={12} />{fetching ? "拉取中…" : "从地址拉取"}
              </button>
            </div>
            <div className="pick-list wide">
              {all.length === 0 && <div className="muted small">还没有模型，可以拉取或手动添加，之后也能在各 Agent 的「模型列表」里改。</div>}
              {all.map((m) => (
                <label key={m} className="pick">
                  <input type="checkbox" checked={models.includes(m)} onChange={() => toggle(m)} />
                  <span className="mono small">{m}</span>
                </label>
              ))}
            </div>
            <div className="row gap6">
              <input className="input mono grow" value={manual} onChange={(e) => setManual(e.target.value)}
                onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); addManual(); } }} placeholder="手动添加模型 ID，多个用空格分隔" />
              <button type="button" className="btn" disabled={!manual.trim()} onClick={addManual}>添加</button>
            </div>
          </div>

          {editable.length > 0 && (
            <div className="field">
              <span>同步到已接入的 Agent {!(changedAddr || changedKey) && <em className="muted tiny">（地址或密钥有改动时才需要）</em>}</span>
              <div className="agent-picks">
                {editable.map((u) => (
                  <label key={uid(u)} className={`apick${sync.has(uid(u)) && (changedAddr || changedKey) ? " on" : ""}${!(changedAddr || changedKey) ? " dim" : ""}`}>
                    <input type="checkbox" disabled={!(changedAddr || changedKey)} checked={sync.has(uid(u))}
                      onChange={() => setSync((p) => { const n = new Set(p); n.has(uid(u)) ? n.delete(uid(u)) : n.add(uid(u)); return n; })} />
                    <AgentIcon id={u.agent.id} size={18} />
                    <span className="small ellipsis">{u.agent.name} · {u.p!.name}</span>
                  </label>
                ))}
              </div>
            </div>
          )}

          {free.length > 0 && (
            <div className="field">
              <span>添加到</span>
              <div className="agent-picks">
                {free.map((a) => (
                  <label key={a.id} className={`apick${addTo.has(a.id) ? " on" : ""}`}>
                    <input type="checkbox" checked={addTo.has(a.id)}
                      onChange={() => setAddTo((p) => { const n = new Set(p); n.has(a.id) ? n.delete(a.id) : n.add(a.id); return n; })} />
                    <AgentIcon id={a.id} size={18} />
                    <span className="small">{a.name}</span>
                    {a.id === "codex" && kind !== "responses" && <span className="tiny muted">· Responses</span>}
                  </label>
                ))}
              </div>
              {isNew && <em className="muted tiny">都不勾也可以，只保存到供应商库，之后随时添加。</em>}
            </div>
          )}

          {err && <div className="err">{err}</div>}
        </div>

        <div className="modal-foot">
          <span className="muted tiny grow">供应商库立即保存；写入 Agent 的部分会先进入「待写入的改动」。</span>
          <button className="btn" onClick={onClose}>取消</button>
          <button className="btn primary" disabled={!canSave} onClick={save}>{saving ? "保存中…" : isNew ? "添加" : "保存"}</button>
        </div>
      </div>
    </div>
  );
}
