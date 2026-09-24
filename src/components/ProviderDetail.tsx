import type { AgentId, AgentState, ApiKind, GatewayRouteView } from "../api";
import { type Draft, type ViewProvider, currentProvider, isEnabled, isVisible, viewModels } from "../draft";
import { AgentIcon, Icon } from "./icons";
import { Avatar, Bars, type Latency, colorFor, latencyTone, latencyView, serviceKey } from "./ProviderCard";
import { ProviderTest } from "./ProviderTest";
import { API_LABEL } from "../services";
import { t } from "../i18n";
import { scrub, scrubHost } from "../privacy";

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
  /** Gateway state of this provider: undefined = direct; null = points at a route that no longer exists. */
  gatewayRoute: GatewayRouteView | null | undefined;
}

export function ProviderDetail({ st, p, draft, agents, latency, onClose, onTest, onAction, onModels, onCopy, onEdit, onDelete, onUndo, gatewayRoute }: Props) {
  const enabled = p.isNew || isEnabled(p, draft);
  const off = st.mode === "multi" && !enabled;
  const isCurrent = st.mode === "single" && currentProvider(st, draft) === p.id;
  const switching = st.currentProvider !== p.id;
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

  const status = p.isDeleted ? t("providerDetail.statusDeleting")
    : p.isNew ? t("providerDetail.statusNew")
    : !p.compatible ? t("providerDetail.statusIncompatible")
    : off ? t("common.disabled")
    : isCurrent ? t(switching ? "providerDetail.statusSwitching" : "providerDetail.statusCurrent")
    : p.builtin ? t("providerDetail.statusBuiltin")
    : st.mode === "multi" ? t("common.enabled")
    : t("providerDetail.statusSwitchable");

  return (
    <section className="pdetail">
      <div className="pdetail-head">
        <Avatar name={p.name} color={colorFor(p)} />
        <div className="grow minw0">
          <div className="strong ellipsis">{p.name}</div>
          <div className="muted tiny">{status}</div>
        </div>
        <button className="icon-btn" aria-label={t("providerDetail.closeDetails")} onClick={onClose}>
          <Icon.close />
        </button>
      </div>

      {!p.isNew && (
        <div className="pdetail-lat">
          <Bars level={lat.level} />
          <span className={`lat${lat.live ? ` ${latencyTone(lat.level)}` : ""}`}>{lat.text}</span>
          {p.baseUrl && p.compatible && !off && <button className="btn small" onClick={onTest}><Icon.pulse size={12} />{t("providerDetail.retest")}</button>}
        </div>
      )}

      {p.baseUrl && p.compatible && (() => {
        // Codex: one shared catalog; others: this provider's own list (visible first).
        const list = st.catalog ? st.catalog.filter((m) => m.visible).map((m) => m.id) : [...p.models].sort((a, b) => Number(b.visible) - Number(a.visible)).map((m) => m.id);
        const configured = st.current.find((r) => r.k === "model")?.v ?? null;
        return (
          <ProviderTest
            // A fresh tester per provider: a result still on its way for the last one is dropped.
            key={`${st.id}\n${p.id}`}
            source={p.isNew ? null : { agent: st.id, provider: p.id }}
            models={list}
            defaultModel={configured}
            disabled={p.isNew ? t("providerDetail.testAfterApply") : null}
          />
        );
      })()}

      <div className="kv">
        <div className="kv-row">
          <span className="muted small">{t("common.baseUrl")}</span>
          <span className="row gap6 minw0">
            <span className="mono small ellipsis grow minw0" title={scrub(p.baseUrl) ?? scrubHost(p.host)}>{scrub(p.baseUrl) ?? scrubHost(p.host)}</span>
            {p.baseUrl && <button className="icon-btn sm" aria-label={t("providerDetail.copyUrl")} title={t("providerDetail.copyUrl")} onClick={() => onCopy(p.baseUrl!)}><Icon.copy size={12} /></button>}
          </span>
        </div>
        <div className="kv-row"><span className="muted small">{t("providerDetail.api")}</span><span className="small">{p.apis.join(" · ")}</span></div>
        {gatewayRoute !== undefined && (
          <div className="kv-row">
            <span className="muted small">{t("providerDetail.connection")}</span>
            <span className="small wrap">
              {gatewayRoute
                ? <>{t("providerDetail.viaGateway", { from: API_LABEL[p.api as ApiKind], to: API_LABEL[gatewayRoute.upstreamApi] })}<br /><span className="mono tiny muted">{scrub(gatewayRoute.upstreamUrl)}</span></>
                : <span className="warn-text">{t("providerDetail.gatewayMissing")}</span>}
            </span>
          </div>
        )}
        {p.isNew && <div className="kv-row"><span className="muted small">{t("common.apiKey")}</span><span className="small">{p.hasKey ? t("providerDetail.keyFilled") : t("providerDetail.keyEmpty")}</span></div>}
        {p.details.map((d) => (
          <div key={d.k} className="kv-row">
            <span className="muted small">{d.k}</span>
            <span className={d.mono ? "mono small wrap" : "small wrap"}>{scrub(d.v)}</span>
          </div>
        ))}
        <div className="kv-row">
          <span className="muted small">{t("common.models")}</span>
          <span className="row gap6">
            <span className="small">{t("providerDetail.visibleOf", { visible, total: p.models.length })}</span>
            {p.models.length > 0 && !p.isNew && <button className="link tiny" onClick={onModels}>{t("providerDetail.viewModels")}</button>}
          </span>
        </div>
      </div>

      {elsewhere.length > 0 && (
        <div className="pdetail-also">
          <span className="muted tiny">{t("providerDetail.alsoIn")}</span>
          {elsewhere.map((e, i) => (
            <span key={i} className="also-chip"><AgentIcon id={e.id} size={16} />{e.name} · {e.provider}</span>
          ))}
        </div>
      )}

      {p.isDeleted || p.isNew ? (
        <div className="grid2">
          {p.isNew && <button className="btn full" onClick={onEdit}>{t("common.edit")}</button>}
          <button className="btn full" onClick={onUndo}>{p.isNew ? t("providerDetail.undoAdd") : t("providerDetail.undoDelete")}</button>
        </div>
      ) : (
        <>
          {p.compatible && !(st.mode === "multi" && p.builtin) && (
            st.mode === "single" ? (
              <button className="btn full" disabled={isCurrent || st.readonly} onClick={onAction}>{!isCurrent ? t("providerDetail.setCurrent") : switching ? t("providerDetail.switchOnApply") : t("providerDetail.inUse")}</button>
            ) : (
              <button className="btn full" disabled={st.readonly} onClick={onAction}>{enabled ? t("providerDetail.disableThis") : t("providerDetail.enableThis")}</button>
            )
          )}
          {p.editable && (
            <div className="grid2">
              <button className="btn full" disabled={st.readonly} onClick={onEdit}>{t("common.edit")}</button>
              <button className="btn full danger" disabled={st.readonly || isCurrent} title={isCurrent ? t("providerDetail.deleteInUse") : undefined} onClick={onDelete}>{t("common.delete")}</button>
            </div>
          )}
        </>
      )}
    </section>
  );
}
