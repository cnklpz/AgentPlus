import { useEffect, useRef, useState } from "react";
import { type AgentState, type ApiKind, type ProviderInput, api } from "../api";
import type { ViewProvider } from "../draft";
import { Icon } from "./icons";

interface Props {
  st: AgentState;
  /** Provider being edited; null = add new. */
  editing: ViewProvider | null;
  onSave: (input: ProviderInput, draftKey?: string) => void;
  onClose: () => void;
}

const API_OPTIONS: { v: ApiKind; label: string; hint: string }[] = [
  { v: "responses", label: "Responses", hint: "OpenAI Responses 接口（/v1/responses）" },
  { v: "chat", label: "Chat", hint: "OpenAI 兼容 Chat Completions（/v1/chat/completions）" },
  { v: "anthropic", label: "Anthropic", hint: "Anthropic Messages 接口（/v1/messages）" },
];

export function ProviderDialog({ st, editing, onSave, onClose }: Props) {
  const isNew = !editing || !!editing.isNew;
  const onlyResponses = st.id === "codex";
  const [name, setName] = useState(editing?.name ?? "");
  const [baseUrl, setBaseUrl] = useState(editing?.baseUrl ?? "");
  const [kind, setKind] = useState<ApiKind>(onlyResponses ? "responses" : editing?.api ?? "chat");
  const [key, setKey] = useState("");
  const [models, setModels] = useState<string[]>(editing?.isNew ? editing.models.map((m) => m.id) : []);
  const [fetched, setFetched] = useState<string[] | null>(null);
  const [manual, setManual] = useState("");
  const [fetching, setFetching] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const first = useRef<HTMLInputElement>(null);

  useEffect(() => {
    first.current?.focus();
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);

  const urlOk = /^https?:\/\/\S+$/.test(baseUrl.trim());
  const canSave = name.trim() !== "" && urlOk;

  const fetchList = async () => {
    setErr(null);
    setFetching(true);
    try {
      // Editing an applied provider without a new key: let the backend use the stored key.
      const list = !isNew && !key.trim() && editing
        ? await api.fetchModels(st.id, editing.id)
        : await api.fetchModelsUrl(baseUrl.trim(), key.trim() || null, kind);
      setFetched(list);
      if (isNew && models.length === 0) setModels(list.slice(0, 20));
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

  const save = () => {
    if (!canSave) return;
    onSave(
      {
        id: isNew ? null : editing!.id,
        name: name.trim(),
        baseUrl: baseUrl.trim(),
        api: kind,
        apiKey: key.trim() || null,
        models: isNew ? models : [],
      },
      editing?.draftKey,
    );
  };

  const all = [...new Set([...(fetched ?? []), ...models])];

  return (
    <div className="modal-bg" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="modal" role="dialog" aria-modal="true" aria-label={isNew ? "添加供应商" : "编辑供应商"}>
        <div className="modal-head">
          <h2>{isNew ? `添加供应商到 ${st.name}` : `编辑「${editing!.name}」`}</h2>
          <button className="icon-btn" aria-label="关闭" onClick={onClose}>
            <svg width="12" height="12" viewBox="0 0 12 12" stroke="currentColor" strokeWidth="1.4" aria-hidden="true"><path d="M1 1l10 10M11 1 1 11" /></svg>
          </button>
        </div>

        <div className="modal-body">
          <label className="field">
            <span>名称</span>
            <input ref={first} className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder="例如：中转 A" />
          </label>
          <label className="field">
            <span>地址（Base URL）</span>
            <input className="input mono" value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="https://api.example.com/v1" />
            {baseUrl && !urlOk && <em className="field-err">需要以 http:// 或 https:// 开头</em>}
          </label>
          <div className="field">
            <span>接口类型</span>
            <div className="seg">
              {API_OPTIONS.map((o) => (
                <button key={o.v} type="button" className={kind === o.v ? "on" : ""} disabled={onlyResponses && o.v !== "responses"}
                  title={onlyResponses && o.v !== "responses" ? "Codex 只支持 Responses 接口" : o.hint} onClick={() => setKind(o.v)}>{o.label}</button>
              ))}
            </div>
          </div>
          <label className="field">
            <span>API Key</span>
            <input className="input mono" type="password" autoComplete="off" value={key} onChange={(e) => setKey(e.target.value)}
              placeholder={!isNew && editing?.hasKey ? "已设置，留空表示不修改" : "sk-..."} />
            <em className="muted tiny">
              {st.id === "codex" ? "写入 ~/.codex/.env，config.toml 里只保存变量名。" : st.id === "zcode" ? "按 ZCode 的格式明文保存在 provider_config.json。" : "按 MiMo 的格式明文保存在 mimocode.jsonc。"}
            </em>
          </label>

          {isNew && (
            <div className="field">
              <div className="row between">
                <span>模型</span>
                <button type="button" className="btn small" disabled={!urlOk || fetching} onClick={fetchList}>
                  <Icon.refresh size={12} />{fetching ? "拉取中…" : "从地址拉取模型"}
                </button>
              </div>
              {err && <div className="err">{err}</div>}
              <div className="pick-list">
                {all.length === 0 && <div className="muted small">还没有模型。拉取或手动添加，也可以之后在「模型列表」里加。</div>}
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
          )}
          {!isNew && (
            <div className="field">
              <div className="row between">
                <span>检查连接</span>
                <button type="button" className="btn small" disabled={!urlOk || fetching} onClick={fetchList}>{fetching ? "检查中…" : "拉取模型列表试试"}</button>
              </div>
              {err && <div className="err">{err}</div>}
              {fetched && <div className="muted small">连接正常，这个地址提供 {fetched.length} 个模型。在「模型列表」里可以添加。</div>}
            </div>
          )}
        </div>

        <div className="modal-foot">
          <span className="muted tiny grow">保存后加入「待写入的改动」，点「应用」才会写入配置文件。</span>
          <button className="btn" onClick={onClose}>取消</button>
          <button className="btn primary" disabled={!canSave} onClick={save}>{isNew ? "添加" : "保存"}</button>
        </div>
      </div>
    </div>
  );
}
