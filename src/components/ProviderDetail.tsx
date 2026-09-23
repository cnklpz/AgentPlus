import type { AgentId, AgentState, Provider } from "../api";
import { type Draft, currentProvider, isEnabled, isVisible } from "../draft";
import { AgentIcon, Icon } from "./icons";
import { Bars, type Latency, colorFor, initials, latencyView, serviceKey } from "./ProviderCard";

interface Props {
  st: AgentState;
  p: Provider;
  draft: Draft;
  agents: AgentState[];
  latency: Latency;
  onClose: () => void;
  onTest: () => void;
  onAction: () => void;
  onModels: () => void;
  onCopy: (text: string) => void;
}

export function ProviderDetail({ st, p, draft, agents, latency, onClose, onTest, onAction, onModels, onCopy }: Props) {
  const enabled = isEnabled(p, draft);
  const off = st.mode === "multi" && !enabled;
  const isCurrent = st.mode === "single" && currentProvider(st, draft) === p.id;
  const lat = latencyView(p, off, latency);
  const visible = p.models.filter((m) => isVisible(p.id, m, draft)).length;

  // The same service configured in other agents (matched by host:port).
  const key = serviceKey(p);
  const elsewhere: { id: AgentId; name: string; provider: string }[] = key
    ? agents.flatMap((a) =>
        a.providers
          .filter((q) => !(a.id === st.id && q.id === p.id) && serviceKey(q) === key)
          .map((q) => ({ id: a.id, name: a.name, provider: q.name })),
      )
    : [];

  return (
    <section className="pdetail">
      <div className="pdetail-head">
        <span className="pavatar" style={{ background: colorFor(p) }}>{initials(p.name)}</span>
        <div className="grow minw0">
          <div className="strong ellipsis">{p.name}</div>
          <div className="muted tiny">
            {!p.compatible ? "不兼容" : off ? "已停用" : isCurrent ? "当前使用" : p.builtin ? "内置" : st.mode === "multi" ? "已启用" : "可切换"}
          </div>
        </div>
        <button className="icon-btn" aria-label="关闭详情" onClick={onClose}>
          <svg width="12" height="12" viewBox="0 0 12 12" stroke="currentColor" strokeWidth="1.4" aria-hidden="true"><path d="M1 1l10 10M11 1 1 11" /></svg>
        </button>
      </div>

      <div className="pdetail-lat">
        <Bars level={lat.level} />
        <span className={`lat${lat.live ? (lat.level >= 2 ? " good" : " slow") : ""}`}>{lat.text}</span>
        {p.baseUrl && p.compatible && !off && <button className="btn small" onClick={onTest}><Icon.pulse size={12} />重新测速</button>}
      </div>

      <div className="kv">
        <div className="kv-row">
          <span className="muted small">地址</span>
          <span className="row gap6 minw0">
            <span className="mono small wrap grow">{p.baseUrl ?? p.host}</span>
            {p.baseUrl && <button className="link tiny" onClick={() => onCopy(p.baseUrl!)}>复制</button>}
          </span>
        </div>
        <div className="kv-row"><span className="muted small">接口</span><span className="small">{p.apis.join(" · ")}</span></div>
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
            {p.models.length > 0 && <button className="link tiny" onClick={onModels}>查看模型列表</button>}
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

      {p.compatible && !(st.mode === "multi" && p.builtin) && (
        st.mode === "single" ? (
          <button className="btn full" disabled={isCurrent || st.readonly} onClick={onAction}>{isCurrent ? "正在使用" : "设为当前供应商"}</button>
        ) : (
          <button className="btn full" disabled={st.readonly} onClick={onAction}>{enabled ? "停用这个供应商" : "启用这个供应商"}</button>
        )
      )}
    </section>
  );
}
