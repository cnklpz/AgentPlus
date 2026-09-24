import { useState } from "react";
import type { AgentId, AgentState } from "../api";
import { API_LABEL, ONLY_API, type GatewayHosts, type Group, type Station, type Use, USE_LABEL, cannotAdd, gatewayCapable, gatewayRouteId, importSource, writableAgents } from "../services";
import { AgentIcon, Icon } from "./icons";
import { Bars, type Latency, initials } from "./ProviderCard";
import { latencyText, stationColor } from "./ProvidersHub";
import { ProviderTest } from "./ProviderTest";
import { t, tn } from "../i18n";
import { scrub, scrubHost } from "../privacy";

interface Props {
  s: Station;
  agents: AgentState[];
  latency: Record<string, Latency>;
  onClose: () => void;
  onTest: (url: string) => void;
  onCopy: (text: string) => void;
  onAddGroup: () => void;
  onEditGroup: (g: Group) => void;
  onAddTo: (g: Group, agent: AgentId) => void;
  onRemove: (u: Use) => void;
  onUndo: (u: Use) => void;
  onModels: (u: Use) => void;
  /** Remove the chosen agent entries of a group and (optionally) its library entry. */
  onDeleteGroup: (g: Group, uses: Use[], fromLibrary: boolean) => void;
  /** Add the group to an agent through the local gateway (any protocol). */
  onViaGateway: (g: Group, agent: AgentId) => void;
  /** "127.0.0.1:<port>" of the gateway, to avoid routing the gateway through itself. */
  gatewayHost: GatewayHosts;
}

function removable(u: Use): string | null {
  if (!u.p || u.state === "removing" || u.state === "adding") return "—";
  if (!u.p.editable) return t("serviceDetail.builtinNoDelete");
  if (u.state === "current") return t("serviceDetail.inUse");
  return null;
}

/** Right-column details of a station: each group with its address, key, test and agent entries. */
export function ServiceDetail(props: Props) {
  const { s, latency } = props;
  const [open, setOpen] = useState<Set<string>>(() => new Set(s.groups.length <= 2 ? s.groups.map((g) => g.key) : [s.groups[0]?.key]));
  const toggle = (k: string) => setOpen((o) => { const n = new Set(o); n.has(k) ? n.delete(k) : n.add(k); return n; });
  const lat = s.baseUrl ? latencyText(latency[s.baseUrl]) : null;

  return (
    <section className="pdetail sdetail" aria-label={t("serviceDetail.detailsAria", { name: s.name })}>
      <div className="pdetail-head">
        <span className="pavatar" style={{ background: stationColor(s) }}>{initials(s.name)}</span>
        <span className="pcard-title">
          <span className="pcard-name"><span className="ellipsis">{scrubHost(s.name)}</span></span>
          <span className="pcard-host mono ellipsis">{s.builtin ? t("serviceDetail.accountLogin") : tn("serviceDetail.hostGroups", s.groups.length, { host: s.host })}</span>
        </span>
        <button className="icon-btn" aria-label={t("serviceDetail.closeDetails")} onClick={props.onClose}><Icon.close /></button>
      </div>

      {lat && (
        <div className="pdetail-lat">
          <Bars level={lat.level} />
          <span className={`grow small ${lat.level >= 2 ? "mono good-ink" : ""}`}>{lat.text}</span>
          <button className="link" onClick={() => props.onTest(s.baseUrl!)}>{t("serviceDetail.retest")}</button>
        </div>
      )}

      <div className="stack6">
        <div className="row between">
          <strong className="small">{t("serviceDetail.groups")}</strong>
          {!s.builtin && <button className="btn xs" onClick={props.onAddGroup}><Icon.plus size={11} />{t("serviceDetail.addGroup")}</button>}
        </div>
        {s.groups.map((g) => (
          <GroupPanel key={g.key} g={g} open={open.has(g.key) || s.groups.length === 1} onToggle={() => toggle(g.key)} builtin={s.builtin} {...props} />
        ))}
      </div>
    </section>
  );
}

function GroupPanel({ g, open, onToggle, builtin, agents, ...props }: Props & { g: Group; open: boolean; onToggle: () => void; builtin: boolean }) {
  const [deleting, setDeleting] = useState(false);
  const deletable = g.uses.filter((u) => removable(u) === null);
  const uid = (u: Use) => `${u.agent.id}:${u.p?.id}`;
  const [pick, setPick] = useState<Set<string>>(new Set(deletable.map(uid)));
  const [fromLib, setFromLib] = useState(true);
  const free = writableAgents(agents).filter((a) => !g.uses.some((u) => u.agent.id === a.id && u.state !== "removing"));

  const src = g.lib ? { agent: "library", provider: g.lib.id } : (() => {
    const u = g.uses.find((x) => x.p && !x.p.isNew && x.p.baseUrl);
    return u ? { agent: u.agent.id as string, provider: u.p!.id } : null;
  })();
  const models = [...new Set([
    ...(g.lib?.models ?? []),
    ...g.uses.flatMap((u) => (u.agent.catalog ? u.agent.catalog.filter((m) => m.visible) : u.p?.models.filter((m) => m.visible) ?? []).map((m) => m.id)),
  ])];

  return (
    <div className={`gpanel${open ? " open" : ""}`}>
      <button className="gpanel-head" onClick={onToggle} aria-expanded={open}>
        <span className={`api-chip api-${g.api}`}>{API_LABEL[g.api]}</span>
        <span className="grow minw0 ellipsis small strong">{g.name}</span>
        <span className="tiny muted">{tn("serviceDetail.uses", g.uses.filter((u) => u.state !== "removing").length)}</span>
        <Icon.chevron />
      </button>
      {open && (
        <div className="gpanel-body">
          {!builtin && (
            <div className="kv">
              <div className="kv-row">
                <span className="muted small">{t("common.baseUrl")}</span>
                <span className="row gap6 minw0">
                  <span className="mono small ellipsis grow" title={scrub(g.baseUrl)}>{scrub(g.baseUrl)}</span>
                  <button className="icon-btn sm" aria-label={t("serviceDetail.copyUrl")} onClick={() => props.onCopy(g.baseUrl)}><Icon.copy size={12} /></button>
                </span>
              </div>
              <div className="kv-row">
                <span className="muted small">{t("common.apiKey")}</span>
                <span className="small">{g.keyHint ? <span className="mono">{scrub(g.keyHint)}</span> : t("serviceDetail.notSet")}{g.lib ? ` · ${t("serviceDetail.inLibrary")}` : ""}</span>
              </div>
            </div>
          )}

          {!builtin && <ProviderTest source={src} models={models} />}

          {!builtin && !deleting && (
            <div className="grid2">
              <button className="btn small" onClick={() => props.onEditGroup(g)}><Icon.edit size={12} />{t("serviceDetail.editGroup")}</button>
              <button className="btn small danger" onClick={() => { setPick(new Set(deletable.map(uid))); setDeleting(true); }}><Icon.trash size={12} />{t("serviceDetail.deleteGroup")}</button>
            </div>
          )}

          {deleting && (
            <div className="confirm-box">
              <strong className="small">{t("serviceDetail.deleteFrom", { name: g.name })}</strong>
              {g.uses.filter((u) => u.p).map((u) => {
                const why = removable(u);
                return (
                  <label key={uid(u)} className={`pick${why ? " dim" : ""}`} title={why ?? undefined}>
                    <input type="checkbox" disabled={!!why} checked={pick.has(uid(u))}
                      onChange={() => setPick((p) => { const n = new Set(p); n.has(uid(u)) ? n.delete(uid(u)) : n.add(uid(u)); return n; })} />
                    <AgentIcon id={u.agent.id} size={16} />
                    <span className="small">{u.agent.name} · {u.p!.name}</span>
                    {why && why !== "—" && <span className="tiny muted">{t("serviceDetail.reason", { why })}</span>}
                  </label>
                );
              })}
              {g.lib && (
                <label className="pick">
                  <input type="checkbox" checked={fromLib} onChange={(e) => setFromLib(e.target.checked)} />
                  <Icon.key size={14} />
                  <span className="small">{t("serviceDetail.removeFromLibrary")}</span>
                </label>
              )}
              <div className="row gap6">
                <span className="tiny muted grow">{t("serviceDetail.deleteQueued")}</span>
                <button className="btn small" onClick={() => setDeleting(false)}>{t("common.cancel")}</button>
                <button className="btn small danger" disabled={pick.size === 0 && !(g.lib && fromLib)}
                  onClick={() => { props.onDeleteGroup(g, deletable.filter((u) => pick.has(uid(u))), !!g.lib && fromLib); setDeleting(false); }}>{t("common.delete")}</button>
              </div>
            </div>
          )}

          <div className="uses">
            {g.uses.map((u) => (
              <div key={u.importKey ?? uid(u)} className={`use ${u.state}`}>
                <AgentIcon id={u.agent.id} size={22} />
                <span className="grow minw0">
                  <span className="block small strong ellipsis">{u.agent.name} · {u.p?.name ?? g.name}</span>
                  <span className="block tiny muted ellipsis">
                    <span className={`ustate ${u.state}`}>{USE_LABEL[u.state]}</span>
                    {u.p && ` · ${u.agent.catalog ? tn("serviceDetail.catalogModels", u.models) : tn("serviceDetail.nModels", u.models)}`}
                  </span>
                </span>
                {(u.state === "adding" || u.state === "removing" || u.state === "new")
                  ? <button className="btn xs" onClick={() => props.onUndo(u)}>{t("serviceDetail.undo")}</button>
                  : <>
                      {u.p && <button className="btn xs" onClick={() => props.onModels(u)}>{t("common.models")}</button>}
                      {u.p && removable(u) === null && <button className="icon-btn sm" aria-label={t("serviceDetail.removeFrom", { agent: u.agent.name })} title={t("serviceDetail.removeFrom", { agent: u.agent.name })} onClick={() => props.onRemove(u)}><Icon.trash size={12} /></button>}
                    </>}
              </div>
            ))}
            {!builtin && free.map((a) => {
              const why = cannotAdd(g, a.id);
              // Protocol mismatch is exactly what the gateway fixes.
              const gw = !gatewayRouteId(g.baseUrl, props.gatewayHost) && !!importSource(g) && gatewayCapable(a.id) && g.api !== "gemini";
              const only = ONLY_API[a.id];
              const protoOnly = !!why && !!only && only !== "gemini" && g.api !== only && g.api !== "gemini";
              return (
                <div key={a.id} className="use none" title={why ?? undefined}>
                  <AgentIcon id={a.id} size={22} />
                  <span className="grow minw0">
                    <span className="block small strong ellipsis">{a.name}</span>
                    <span className="block tiny muted ellipsis">{protoOnly ? t("serviceDetail.needsProto", { need: API_LABEL[only!], api: API_LABEL[g.api] }) : why ?? t("serviceDetail.notAdded")}</span>
                  </span>
                  {protoOnly && gw ? (
                    <button className="btn xs restore" onClick={() => props.onViaGateway(g, a.id)} title={t("serviceDetail.addViaGatewayTitle")}>
                      <Icon.gateway size={11} />{t("serviceDetail.addViaGateway")}
                    </button>
                  ) : (
                    <>
                      {gw && !why && (
                        <button className="btn xs ghost-btn" onClick={() => props.onViaGateway(g, a.id)} title={t("serviceDetail.viaGatewayTitle")}>
                          <Icon.gateway size={11} />{t("serviceDetail.viaGateway")}
                        </button>
                      )}
                      <button className="btn xs restore" disabled={!!why} onClick={() => props.onAddTo(g, a.id)}><Icon.plus size={11} />{t("common.add")}</button>
                    </>
                  )}
                </div>
              );
            })}
          </div>
        </div>
      )}
    </div>
  );
}
