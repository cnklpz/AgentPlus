import type { AgentId, AgentState } from "../api";
import { type Draft, type ViewProvider, currentProvider, isEnabled, isVisible, viewModels } from "../draft";
import { AgentIcon, Icon } from "./icons";
import { Bars, type Latency, colorFor, initials, latencyView, serviceKey } from "./ProviderCard";

interface Props {
  st: AgentState;
  p: ViewProvider;
  draft: Draft;
  agents: AgentState[];
  latency: Latency;
  onClose: () => void;
  onTest: () => void;
  onAction: () => void;
  onModels: () => void;
  onCopy: (text: string) => void;
  onEdit: () => void;
  onDelete: () => void;
  onUndo: () => void;
}

export function ProviderDetail({ st, p, draft, agents, latency, onClose, onTest, onAction, onModels, onCopy, onEdit, onDelete, onUndo }: Props) {
  const enabled = p.isNew || isEnabled(p, draft);
  const off = st.mode === "multi" && !enabled;
  const isCurrent = st.mode === "single" && currentProvider(st, draft) === p.id;
  const lat = latencyView(p, off, latency);
  const visible = p.isNew ? p.models.length : viewModels(p.id, p.models, draft).filter((m) => !m.isDeleted && isVisible(p.id, m, draft)).length;

  // The same service configured in other agents (matched by host:port).
  const key = serviceKey(p);
  const elsewhere: { id: AgentId; name: string; provider: string }[] = key
    ? agents.flatMap((a) =>
        a.providers
          .filter((q) => !(a.id === st.id && q.id === p.id) && serviceKey(q) === key)
          .map((q) => ({ id: a.id, name: a.name, provider: q.name })),
      )
    : [];

  const status = p.isDeleted ? "将删除（应用后生效）" : p.isNew ? "新增（应用后生效）" : !p.compatible ? "不兼容" : off ? "已停用" : isCurrent ? "当前使用" : p.builtin ? "内置" : st.mode === "multi" ? "已启用" : "可切换";

  return (
    <section className="pdetail">
      <div className="pdetail-head">
        <span className="pavatar" style={{ background: colorFor(p) }}>{initials(p.name)}</span>
        <div className="grow minw0">
          <div className="strong ellipsis">{p.name}</div>
          <div className="muted tiny">{status}</div>
        </div>
        <button className="icon-btn" aria-label="关闭详情" onClick={onClose}>
          <svg width="12" height="12" viewBox="0 0 12 12" stroke="currentColor" strokeWidth="1.4" aria-hidden="true"><path d="M1 1l10 10M11 1 1 11" /></svg>
        </button>
      </div>

      {!p.isNew && (
        <div className="pdetail-lat">
          <Bars level={lat.level} />
          <span className={`lat${lat.live ? (lat.level >= 2 ? " good" : " slow") : ""}`}>{lat.text}</span>
          {p.baseUrl && p.compatible && !off && <button className="btn small" onClick={onTest}><Icon.pulse size={12} />重新测速</button>}
        </div>
      )}

      <div className="kv">
        <div className="kv-row">
          <span className="muted small">地址</span>
          <span className="row gap6 minw0">
            <span className="mono small wrap grow">{p.baseUrl ?? p.host}</span>
            {p.baseUrl && <button className="link tiny" onClick={() => onCopy(p.baseUrl!)}>复制</button>}
          </span>
        </div>
        <div className="kv-row"><span className="muted small">接口</span><span className="small">{p.apis.join(" · ")}</span></div>
        {p.isNew && <div className="kv-row"><span className="muted small">密钥</span><span className="small">{p.hasKey ? "已填写" : "未填写"}</span></div>}
        {p.details.map((d) => (
          <div key={d.k} className="kv-row">
            <span className="muted small">{d.k}</span>
            <span className={d.mono ? "mono small wrap" : "small wrap"}>{d.v}</span>
          </div>
        ))}
        <div className="kv-row">
          <span className="muted small">模型</span>
          <span className="row gap6">
            <span className="small">{visible}/{p.models.length} 可见</span>
            {p.models.length > 0 && !p.isNew && <button className="link tiny" onClick={onModels}>查看模型列表</button>}
          </span>
        </div>
      </div>

      {elsewhere.length > 0 && (
        <div className="pdetail-also">
          <span className="muted tiny">同一服务也配置在</span>
          {elsewhere.map((e, i) => (
            <span key={i} className="also-chip"><AgentIcon id={e.id} size={16} />{e.name} · {e.provider}</span>
          ))}
        </div>
      )}

      {p.isDeleted || p.isNew ? (
        <div className="grid2">
          {p.isNew && <button className="btn full" onClick={onEdit}>编辑</button>}
          <button className="btn full" onClick={onUndo}>{p.isNew ? "撤销添加" : "撤销删除"}</button>
        </div>
      ) : (
        <>
          {p.compatible && !(st.mode === "multi" && p.builtin) && (
            st.mode === "single" ? (
              <button className="btn full" disabled={isCurrent || st.readonly} onClick={onAction}>{isCurrent ? "正在使用" : "设为当前供应商"}</button>
            ) : (
              <button className="btn full" disabled={st.readonly} onClick={onAction}>{enabled ? "停用这个供应商" : "启用这个供应商"}</button>
            )
          )}
          {p.editable && (
            <div className="grid2">
              <button className="btn full" disabled={st.readonly} onClick={onEdit}>编辑</button>
              <button className="btn full danger" disabled={st.readonly || isCurrent} title={isCurrent ? "正在使用，先切换到其他供应商" : undefined} onClick={onDelete}>删除</button>
            </div>
          )}
        </>
      )}
    </section>
  );
}
