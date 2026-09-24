import { useEffect, useRef, useState } from "react";
import { type AgentId, type AgentState, type ApiKind, type GatewayBreaker, type GatewayBreakerView, type GatewayRoute, type GatewayRouteView, type GatewayStatus, type TestResult, api } from "../api";
import { API_LABEL, type Group, type Station, apiFor, gatewayCapable, gatewayRouteId, plainRoute } from "../services";
import { ComboBox } from "./ComboBox";
import { Dropdown } from "./Dropdown";
import { AgentIcon, Icon } from "./icons";
import { Modal } from "./Modal";
import { ErrorBox, Seg, ToggleRow } from "./controls";
import { TestButton, TestResultView } from "./ProviderTest";
import { t, tn, tx } from "../i18n";
import { scrub } from "../privacy";
import { copyText, errText, type Flash, isHttpUrl, onActivateKey } from "../util";
import { joinList } from "../format";

interface Props {
  status: GatewayStatus | null;
  setStatus: (s: GatewayStatus) => void;
  agents: AgentState[];
  stations: Station[];
  /** Current gateway host first, then earlier ports. */
  gatewayHost: readonly string[];
  /**
   * Create (or reuse) the route for a group and make sure the gateway runs; with `replace`,
   * the agent entries using the group's address are switched to the forward.
   */
  onForward: (g: Group, replace: boolean) => Promise<GatewayRouteView>;
  /** Confirm and delete a forward (optionally restoring the addresses it replaced). */
  onDeleteRoute: (r: GatewayRouteView) => Promise<boolean>;
  /** Add a provider to an agent that points at this route (pending change). */
  onAddToAgent: (route: GatewayRouteView, agent: AgentId, api: ApiKind) => void;
  /** Agent entries pointing at the gateway without their agent's own key. */
  staleKeys: number;
  /** Queue those entries for rewriting with each agent's key (pending changes). */
  onUpdateKeys: () => void;
  flash: Flash;
}

const PROTOS: ApiKind[] = ["chat", "responses", "anthropic"];
const PATHS: Record<ApiKind, string> = { chat: "/chat/completions", responses: "/responses", anthropic: "/messages", gemini: "" };

/** Paused by the error breaker (or waiting for its trial request). */
export function tripped(r: GatewayRouteView): GatewayBreakerView | null {
  return r.breaker && r.breaker.state !== "closed" ? r.breaker : null;
}

function secs(n: number) {
  return n >= 60 ? t("gatewayPage.minSec", { m: Math.floor(n / 60), s: n % 60 }) : t("gatewayPage.sec", { n });
}

/** "15:41:52 起 · 42 秒后重试" */
function breakerWhen(b: GatewayBreakerView) {
  return b.state === "open"
    ? t("gatewayPage.breakerOpen", { at: b.at ?? "", wait: secs(b.remainingSecs) })
    : t("gatewayPage.breakerHalfOpen");
}

/** Local gateway, organised by provider: turn forwarding on for a provider, then plug it into any agent. */
export function GatewayPage({ status: s, setStatus, agents, stations, gatewayHost, onForward, onDeleteRoute, onAddToAgent, staleKeys, onUpdateKeys, flash }: Props) {
  const [port, setPort] = useState(String(s?.port ?? 18650));
  const [busy, setBusy] = useState<string | null>(null);
  const [open, setOpen] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const remove = async (id: string) => {
    const r = s?.routes.find((x) => x.id === id);
    if (r && (await onDeleteRoute(r)) && open === id) setOpen(null);
  };
  useEffect(() => { if (s) setPort(String(s.port)); }, [s?.port]);

  const copy = (text: string) => copyText(text, flash);
  const paused = s?.routes.filter((r) => tripped(r)) ?? [];
  const resetBreaker = async (id: string | null) => {
    try {
      setStatus(await api.gatewayResetBreaker(id));
      flash(t(id ? "gatewayPage.resumedOne" : "gatewayPage.resumedAll"));
    } catch (e) {
      flash(errText(e), true);
    }
  };
  const portNum = /^\d+$/.test(port.trim()) ? Number(port.trim()) : NaN;
  const portOk = portNum >= 1024 && portNum <= 65535;
  const power = async (on: boolean) => {
    // Starting or switching needs a usable port; stopping keeps whatever is saved.
    if (on && !portOk) return;
    setBusy("power");
    try {
      setStatus(await api.gatewaySet(on, portOk ? portNum : null));
      flash(t(on ? "common.gatewayStarted" : "common.gatewayStopped"));
    } catch (e) {
      flash(errText(e), true);
      api.gatewayStatus().then(setStatus).catch(() => undefined);
    } finally {
      setBusy(null);
    }
  };

  const groups = stations.filter((x) => !x.builtin && !gatewayHost.includes(x.host.replace("localhost", "127.0.0.1"))).flatMap((st) => st.groups.map((g) => ({ st, g })));
  const routeOf = (g: Group) => s?.routes.find((r) => r.library === g.lib?.id && r.upstreamApi === g.api) ?? null;
  const forwarded = s?.routes.length ?? 0;
  const portChanged = !!s && String(s.port) !== port.trim();
  const base = s ? `http://127.0.0.1:${s.port}` : "";

  const forward = async (g: Group, replace: boolean) => {
    const r = await onForward(g, replace);
    setOpen(r.id);
    setAdding(false);
    flash(t(replace && r.replaced?.length ? "gatewayPage.forwardAddedReplaced" : "gatewayPage.forwardAdded", { base: r.localBase }));
  };

  return (
    <main className="page">
      <div className="page-top">
        <div className="page-head">
          <span className="page-icon"><Icon.gateway size={20} /></span>
          <div className="page-title">
            <h1>{t("gatewayPage.title")}</h1>
            <span className="muted small">{t("gatewayPage.intro")}</span>
          </div>
        </div>
      </div>

      <div className="page-body">
        <div className="settings full">
          <section className={`gw-hero${s?.running ? " on" : s?.enabled ? " bad" : ""}`}>
            <div className="gw-hero-main">
              <span className="gw-hero-dot"><span className="gw-dot" /></span>
              <div className="grow minw0">
                <div className="gw-hero-title">{t(s?.running ? "gatewayPage.online" : s?.enabled ? "gatewayPage.notRunning" : "gatewayPage.off")}</div>
                {s?.running ? (
                  <div className="row gap6 minw0">
                    <span className="mono small ellipsis">{base}</span>
                    <button className="icon-btn sm" aria-label={t("common.copyUrl")} onClick={() => copy(base)}><Icon.copy size={12} /></button>
                    <span className="tiny muted">· {tn("gatewayPage.heroRequests", s.requests)} · {tn("gatewayPage.heroFailures", s.failures)}{s.active ? ` · ${tn("gatewayPage.heroActive", s.active)}` : ""}</span>
                  </div>
                ) : (
                  <div className="small muted">{scrub(s?.error) ?? t("gatewayPage.idleHint")}</div>
                )}
              </div>
              <label className="gw-port-field">
                <span className="tiny muted">{t("gatewayPage.port")}</span>
                <input className="input mono" value={port} onChange={(e) => setPort(e.target.value.replace(/\D/g, "").slice(0, 5))} aria-label={t("gatewayPage.port")}
                  aria-invalid={!portOk} title={portOk ? undefined : t("gatewayPage.portInvalid")} />
              </label>
              {s?.running ? (
                <>
                  {portChanged && <button className="btn primary" disabled={!!busy || !portOk} onClick={() => power(true)}>{t("gatewayPage.switchPort")}</button>}
                  <button className="btn" disabled={!!busy} onClick={() => power(false)}>{t("gatewayPage.stop")}</button>
                </>
              ) : (
                <button className="btn primary" disabled={!!busy || !s || !portOk} onClick={() => power(true)}><Icon.gateway size={13} />{t("gatewayPage.start")}</button>
              )}
            </div>
            {!portOk && <em className="field-err">{t("gatewayPage.portInvalid")}</em>}
            {s?.running && s.error && <ErrorBox text={s.error} />}
            {s?.running && paused.length > 0 && (
              <div className="gw-trip" role="alert">
                <div className="row between gap6">
                  <strong className="small">{tn("gatewayPage.pausedBanner", paused.length)}</strong>
                  {paused.length > 1 && <button className="btn xs" onClick={() => resetBreaker(null)}>{t("gatewayPage.resumeAll")}</button>}
                </div>
                {paused.map((r) => (
                  <div key={r.id} className="gw-trip-row">
                    <span className="small strong ellipsis">{r.name}</span>
                    <span className="small grow minw0 gw-trip-why">{scrub(r.breaker!.reason) ?? t("gatewayPage.upstreamError")}</span>
                    <span className="tiny muted nowrap">{breakerWhen(r.breaker!)}</span>
                    <button className="btn xs" onClick={() => resetBreaker(r.id)}>{t("gatewayPage.resumeNow")}</button>
                  </div>
                ))}
              </div>
            )}
            {staleKeys > 0 && (
              <div className="gw-stale">
                <Icon.key size={14} />
                <div className="grow minw0">
                  <strong className="small">{tn("gatewayPage.staleKeysBanner", staleKeys)}</strong>
                  <div className="tiny muted">{t("gatewayPage.staleKeysHint")}</div>
                </div>
                <button className="btn xs" onClick={onUpdateKeys}>{t("gatewayPage.updateKeys")}</button>
              </div>
            )}
            {forwarded === 0 && (
              <div className="gw-steps">
                <span className={s?.running ? "done" : ""}><b>1</b>{t("gatewayPage.step1")}</span>
                <Icon.arrow size={12} />
                <span><b>2</b>{t("gatewayPage.step2")}</span>
                <Icon.arrow size={12} />
                <span><b>3</b>{t("gatewayPage.step3")}</span>
              </div>
            )}
          </section>

          {s && s.routes.length > 0 && <Unified s={s} copy={copy} />}

          <section className="sgroup">
            <h2 className="row between">
              <span>{t("gatewayPage.routes")}</span>
              <button className="btn xs" onClick={() => setAdding(true)}><Icon.plus size={11} />{t("gatewayPage.addRoute")}</button>
            </h2>
            {s && s.routes.length === 0 && (
              <div className="gw-empty">
                <strong className="small">{t("gatewayPage.noRoutes")}</strong>
                <span className="muted small">{t("gatewayPage.noRoutesHint")}</span>
                <button className="btn primary small" onClick={() => setAdding(true)}><Icon.plus size={12} />{t("gatewayPage.addRoute")}</button>
              </div>
            )}
            {s?.routes.map((r) => {
              const g = groups.find(({ g }) => routeOf(g)?.id === r.id)?.g ?? null;
              const expanded = open === r.id;
              const trip = tripped(r);
              return (
                <div key={r.id} className={`gw-item fwd${expanded ? " open" : ""}`} data-url={r.localBase} data-ctx="route" data-route={r.id}>
                  <div className="gw-item-head" role="button" tabIndex={0} aria-expanded={expanded} onClick={() => setOpen(expanded ? null : r.id)}
                    onKeyDown={onActivateKey(() => setOpen(expanded ? null : r.id))}>
                    <span className={`api-chip api-${r.upstreamApi}`}>{API_LABEL[r.upstreamApi]}</span>
                    <span className="grow minw0">
                      <span className="block small strong ellipsis">{r.name}</span>
                      <span className={`block tiny ellipsis ${r.upstreamMissing || (trip && r.enabled) ? "warn-text" : "muted"}`}>
                        {r.upstreamMissing ? t("gatewayPage.upstreamMissing") : trip && r.enabled ? t("gatewayPage.tripped", { reason: trip.reason ?? t("gatewayPage.upstreamError") }) : `${r.localBase}  →  ${scrub(r.upstreamUrl)}`}
                      </span>
                    </span>
                    <span className="gw-weight" title={t("gatewayPage.weightTitle")}>{t("gatewayPage.weightChip", { w: r.weight })}</span>
                    {r.upstreamMissing ? <span className="chip-muted">{t("gatewayPage.chipBroken")}</span>
                      : !r.enabled ? <span className="chip-muted">{t("gatewayPage.chipPaused")}</span>
                      : trip ? <span className="chip-bad" title={breakerWhen(trip)}>{trip.state === "open" ? t("gatewayPage.chipTripped", { wait: secs(trip.remainingSecs) }) : t("gatewayPage.chipProbe")}</span>
                      : <span className="chip-ok">{t("gatewayPage.chipActive")}</span>}
                    <button className="icon-btn sm" aria-label={t("gatewayPage.copyGatewayUrl")} title={t("common.copyUrl")} onClick={(e) => { e.stopPropagation(); copy(r.localBase); }}><Icon.copy size={12} /></button>
                    <button className="icon-btn sm" aria-label={t("gatewayPage.deleteRoute")} title={t("gatewayPage.deleteRoute")} onClick={(e) => { e.stopPropagation(); remove(r.id); }}><Icon.trash size={12} /></button>
                    <span className="gw-chev"><Icon.chevron /></span>
                  </div>
                  {expanded && <RouteBody r={r} g={g} agents={agents} running={!!s?.running} threshold={s?.breaker.threshold ?? 3} setStatus={setStatus} copy={copy} flash={flash}
                    onAddToAgent={onAddToAgent} onDelete={() => remove(r.id)} onResetBreaker={() => resetBreaker(r.id)} />}
                </div>
              );
            })}
          </section>

          {s && s.routes.length > 0 && <BreakerSettings cfg={s.breaker} setStatus={setStatus} flash={flash} />}

          <section className="sgroup">
            <h2>{t("gatewayPage.requestLog")}</h2>
            {(!s || s.log.length === 0) && <div className="srow muted small">{t("gatewayPage.noRequests")}</div>}
            {s && s.log.length > 0 && (
              <div className="gw-log">
                {s.log.map((l, i) => (
                  <div key={i} className={`gw-logrow${l.status >= 400 || l.error ? " bad" : ""}`} title={scrub(l.error) ?? undefined}>
                    <span className="mono tiny muted">{l.at}</span>
                    <span className="mono small ellipsis">{l.route}</span>
                    <span className="tiny">
                      {l.inbound ? API_LABEL[l.inbound as ApiKind] : l.method}
                      {l.converted ? <> → {API_LABEL[l.upstream as ApiKind]}</> : l.upstream ? t("gatewayPage.passthroughSuffix") : ""}
                    </span>
                    <span className="mono tiny ellipsis">{l.model}</span>
                    <span className={`mono tiny ${l.status >= 400 ? "warn-text" : "good-ink"}`}>{l.status || "—"}</span>
                    <span className="mono tiny muted">{(l.ms / 1000).toFixed(2)}s{l.stream ? t("gatewayPage.streamSuffix") : ""}</span>
                  </div>
                ))}
              </div>
            )}
          </section>
        </div>
      </div>
      {adding && (
        <AddForward
          groups={groups.filter(({ g }) => !routeOf(g))}
          onForward={forward}
          onClose={() => setAdding(false)}
        />
      )}
    </main>
  );
}

/** The unified entry: one address for every forward, routed by model and split by weight. */
function Unified({ s, copy }: { s: GatewayStatus; copy: (t: string) => void }) {
  const live = s.routes.filter((r) => r.enabled && !r.upstreamMissing);
  const byModel = new Map<string, GatewayRouteView[]>();
  for (const r of live) for (const m of r.models) byModel.set(m, [...(byModel.get(m) ?? []), r]);
  const rows = [...byModel.entries()].sort((a, b) => b[1].length - a[1].length || a[0].localeCompare(b[0]));
  const unknown = live.filter((r) => r.models.length === 0);
  return (
    <section className="sgroup">
      <h2 className="row between"><span>{t("gatewayPage.unified")}</span><span className="tiny muted">{tn("common.modelCount", rows.length)} · {tn("gatewayPage.routeCount", live.length)}</span></h2>
      <div className="srow stacked">
        <div className="gw-base">
          <span className="mono small grow ellipsis">{s.unifiedBase}</span>
          <button className="icon-btn sm" aria-label={t("gatewayPage.copyUnified")} onClick={() => copy(s.unifiedBase)}><Icon.copy size={12} /></button>
        </div>
        <span className="tiny muted">
          {tx("gatewayPage.unifiedHint", { example: <span className="mono">{t("gatewayPage.unifiedExample")}</span> })}
        </span>
        {rows.length > 0 && (
          <div className="gw-split">
            {rows.slice(0, 30).map(([m, rs]) => {
              const total = rs.reduce((n, r) => n + Math.max(1, r.weight), 0);
              return (
                <div key={m} className="gw-split-row">
                  <span className="mono small ellipsis" title={m}>{m}</span>
                  <span className="gw-split-bar">
                    {rs.map((r) => {
                      const pct = Math.round((Math.max(1, r.weight) / total) * 100);
                      return <span key={r.id} className="gw-split-seg" style={{ flexGrow: Math.max(1, r.weight) }} title={t("gatewayPage.splitTitle", { name: r.name, weight: r.weight, pct })}>{rs.length > 1 ? `${r.name} ${pct}%` : r.name}</span>;
                    })}
                  </span>
                </div>
              );
            })}
            {rows.length > 30 && <span className="tiny muted">{tn("gatewayPage.moreModels", rows.length - 30)}</span>}
          </div>
        )}
        {unknown.length > 0 && (
          <span className="tiny muted">{t("gatewayPage.unknownModels", { names: joinList(unknown.map((r) => r.name)) })}</span>
        )}
      </div>
    </section>
  );
}

/** Add a forward: pick a provider group, or type a custom upstream (saved to the library). */
function AddForward({ groups, onForward, onClose }: {
  groups: { st: Station; g: Group }[];
  onForward: (g: Group, replace: boolean) => Promise<void>;
  onClose: () => void;
}) {
  const [mode, setMode] = useState<"pick" | "custom">(groups.length ? "pick" : "custom");
  const [pick, setPick] = useState(groups[0]?.g.key ?? "");
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [apiKind, setApiKind] = useState<ApiKind>("chat");
  const [key, setKey] = useState("");
  const [replace, setReplace] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  /** Custom upstream already saved to the library by an earlier attempt. */
  const savedId = useRef<string | null>(null);

  const urlOk = isHttpUrl(url);
  const can = mode === "pick" ? !!pick : name.trim() !== "" && urlOk;
  const save = async () => {
    setBusy(true);
    setErr(null);
    try {
      if (mode === "pick") {
        const g = groups.find((x) => x.g.key === pick)?.g;
        if (!g) throw new Error(t("gatewayPage.chooseProvider"));
        await onForward(g, replace);
      } else {
        // A retry after a failed forward updates the entry saved the first time instead of adding another.
        const e = await api.librarySave({ id: savedId.current, name: name.trim(), baseUrl: url.trim(), api: apiKind, apiKey: key.trim() || null, models: null, adoptFrom: null });
        savedId.current = e.id;
        await onForward({ key: `lib:${e.id}`, name: e.name, baseUrl: e.baseUrl, api: e.api, keyFp: e.keyFp, keyHint: e.keyHint, lib: e, uses: [] }, false);
      }
    } catch (e) {
      setErr(errText(e));
      setBusy(false);
    }
  };

  const foot = (
    <>
      <span className="muted tiny grow">{t("gatewayPage.autoStart")}</span>
      <button className="btn" onClick={onClose}>{t("common.cancel")}</button>
      <button className="btn primary" disabled={!can || busy} onClick={save}>{busy ? t("gatewayPage.adding") : t("common.add")}</button>
    </>
  );
  return (
    <Modal label={t("gatewayPage.addRoute")} title={t("gatewayPage.addRoute")} onClose={onClose} foot={foot}>
      <Seg value={mode} onChange={setMode} options={[
        { value: "pick", label: t("gatewayPage.pickExisting"), disabled: !groups.length },
        { value: "custom", label: t("gatewayPage.customUrl") },
      ]} />
      {mode === "pick" ? (
        <div className="field">
          <span className="field-label">{t("common.provider")}</span>
          <Dropdown value={pick} label={t("common.provider")} onChange={setPick}
            options={groups.map(({ st, g }) => ({ value: g.key, label: g.name, hint: `${st.name} · ${API_LABEL[g.api]} · ${g.baseUrl}` }))} />
          <em className="muted tiny">{t("gatewayPage.pickHint")}</em>
          {(() => {
            const users = replaceable(groups.find((x) => x.g.key === pick)?.g);
            return (
              <ToggleRow className="af-replace" on={replace} onChange={setReplace} disabled={users.length === 0} title={t("gatewayPage.replaceTitle")}
                hint={users.length === 0
                  ? t("gatewayPage.replaceNone")
                  : replace
                    ? t("gatewayPage.replaceOn", {
                        list: joinList(users.map((u) => t("gatewayPage.agentProvider", { agent: u.agent.name, provider: u.p!.name }))),
                      })
                    : tn("gatewayPage.replaceOff", users.length)} />
            );
          })()}
        </div>
      ) : (
        <>
          <div className="field">
            <label htmlFor="af-name">{t("common.name")}</label>
            <input id="af-name" className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("gatewayPage.namePlaceholder")} />
          </div>
          <div className="field">
            <label htmlFor="af-url">{t("gatewayPage.upstreamUrl")}</label>
            <input id="af-url" className="input mono sensitive" value={url} onChange={(e) => setUrl(e.target.value)} placeholder="https://api.example.com/v1" />
            {url && !urlOk && <em className="field-err">{t("common.urlInvalid")}</em>}
          </div>
          <div className="field">
            <span className="field-label">{t("gatewayPage.upstreamProto")}</span>
            <Seg value={apiKind} onChange={setApiKind} label={t("gatewayPage.upstreamProto")} options={PROTOS.map((p) => ({ value: p, label: API_LABEL[p] }))} />
            <em className="muted tiny">{t("gatewayPage.protoHint")}</em>
          </div>
          <div className="field">
            <label htmlFor="af-key">{t("common.apiKey")}</label>
            <input id="af-key" className="input mono" type="password" autoComplete="off" value={key} onChange={(e) => setKey(e.target.value)} placeholder="sk-..." />
            <em className="muted tiny">{t("gatewayPage.keyHint")}</em>
          </div>
        </>
      )}
      {err && <ErrorBox text={err} />}
    </Modal>
  );
}

/** Agent entries that use a group's address directly (what "replace" switches to the forward). */
function replaceable(g: Group | undefined) {
  return (g?.uses ?? []).filter((u) => u.p && u.p.editable && !u.p.isNew && !u.p.isDeleted && u.p.baseUrl);
}

function RouteBody({ r, g, agents, running, threshold, setStatus, copy, flash, onAddToAgent, onDelete, onResetBreaker }: {
  onDelete: () => void;
  onResetBreaker: () => void;
  r: GatewayRouteView; g: Group | null; agents: AgentState[]; running: boolean; threshold: number; setStatus: (s: GatewayStatus) => void;
  copy: (t: string) => void; flash: Flash; onAddToAgent: Props["onAddToAgent"];
}) {
  const [inbound, setInbound] = useState<ApiKind>(r.upstreamApi === "responses" ? "chat" : "responses");
  const models = [...new Set([...(g?.lib?.models ?? []), ...(g?.uses ?? []).flatMap((u) => u.p?.models.map((m) => m.id) ?? [])])];
  const [model, setModel] = useState(models[0] ?? "");
  const [testing, setTesting] = useState(false);
  const [res, setRes] = useState<TestResult | string | null>(null);
  const [map, setMap] = useState(r.modelMap.map(([a, b]) => `${a}=${b}`).join("\n"));
  const [advanced, setAdvanced] = useState(r.modelMap.length > 0);
  const [weight, setWeight] = useState(String(r.weight));

  // Agents already pointing at this route.
  const linked = new Set(agents.filter((a) => a.providers.some((p) => gatewayRouteId(p.baseUrl, r.localBase.replace(/^https?:\/\//, "").split("/")[0]) === r.id)).map((a) => a.id));

  const save = async (patch: Partial<GatewayRoute>, msg: string) => {
    try {
      setStatus(await api.gatewaySaveRoute({ ...plainRoute(r), ...patch }, r.id));
      flash(msg);
    } catch (e) {
      flash(errText(e), true);
    }
  };
  const test = async () => {
    setTesting(true);
    setRes(null);
    try {
      setRes(await api.gatewayTest(r.id, inbound, model.trim()));
    } catch (e) {
      setRes(errText(e));
    } finally {
      setTesting(false);
      // A passing test un-pauses a tripped forward.
      api.gatewayStatus().then(setStatus).catch(() => undefined);
    }
  };
  const canTest = running && r.enabled && !testing && !!model.trim();
  const b = r.breaker;

  return (
    <div className="gw-item-body">
      {b && (
        <div className={`gw-block gw-breaker ${b.state}`}>
          <span className="gw-label">{t("gatewayPage.breaker")}</span>
          {b.state === "closed" ? (
            <span className="small">{tx("gatewayPage.breakerCounting", { fails: b.fails, threshold, reason: <span className="gw-trip-why">{scrub(b.reason)}</span> })}</span>
          ) : (
            <>
              <span className="small">
                {tx("gatewayPage.pausedReason", {
                  paused: <strong>{t("gatewayPage.paused")}</strong>,
                  reason: <span className="gw-trip-why">{scrub(b.reason) ?? t("gatewayPage.upstreamError")}</span>,
                })}
              </span>
              <div className="row gap6">
                <span className="tiny muted grow">
                  {t("gatewayPage.pausedDetail", {
                    when: b.trips > 1 ? t("gatewayPage.breakerWhenTrips", { when: breakerWhen(b), n: b.trips }) : breakerWhen(b),
                  })}
                </span>
                <button className="btn small primary" onClick={onResetBreaker}>{t("gatewayPage.resumeNow")}</button>
              </div>
            </>
          )}
        </div>
      )}
      <div className="gw-block">
        <span className="gw-label">{t("gatewayPage.connectAgents")}</span>
        <div className="gw-actions">
          {agents.filter((a) => a.installed && !a.readonly && gatewayCapable(a.id)).map((a) => {
            const via: ApiKind = apiFor(a.id, r.upstreamApi);
            return linked.has(a.id) ? (
              <span key={a.id} className="gw-linked"><AgentIcon id={a.id} size={16} />{t("gatewayPage.linked", { name: a.name })}</span>
            ) : (
              <button key={a.id} className="btn small" disabled={!r.enabled} title={t("gatewayPage.addToAgentTitle", { agent: a.name, api: API_LABEL[via] })}
                onClick={() => onAddToAgent(r, a.id, via)}>
                <AgentIcon id={a.id} size={16} />{a.name}<span className="tiny muted">{API_LABEL[via]}</span>
              </button>
            );
          })}
        </div>
        <span className="tiny muted">{t("gatewayPage.connectHint")}</span>
      </div>

      <div className="gw-block">
        <span className="gw-label">{t("gatewayPage.otherClients")}</span>
        <div className="gw-paths">
          {PROTOS.map((p) => (
            <button key={p} className="gw-path" onClick={() => copy(r.localBase + PATHS[p])} title={t("gatewayPage.copyFullUrl")}>
              <span className={`api-chip api-${p}`}>{API_LABEL[p]}</span>
              <span className="mono tiny">{PATHS[p]}</span>
              <span className="tiny muted">{t(p === r.upstreamApi ? "gatewayPage.passthrough" : "gatewayPage.converted")}</span>
            </button>
          ))}
        </div>
      </div>

      <div className="gw-block">
        <span className="gw-label">{t("common.test")}</span>
        <div className="gw-test">
          <Seg value={inbound} onChange={setInbound} options={PROTOS.map((p) => ({ value: p, label: API_LABEL[p] }))} />
          <ComboBox value={model} options={models} onChange={setModel} onEnter={() => { if (canTest) test(); }} placeholder={t("gatewayPage.modelId")} label={t("gatewayPage.testModel")} disabled={testing} />
          <TestButton className="btn small primary" running={testing} disabled={!canTest} onClick={test} title={t(running ? "gatewayPage.testTitle" : "gatewayPage.startFirst")} />
        </div>
        {res !== null && <TestResultView result={res} extra={<span className="tiny">{API_LABEL[inbound]} → {API_LABEL[r.upstreamApi]}</span>} />}
      </div>

      <div className="gw-block">
        <span className="gw-label">{t("gatewayPage.weight")}</span>
        <div className="row gap6">
          <input className="input mono gw-weight-input" value={weight} onChange={(e) => setWeight(e.target.value.replace(/\D/g, "").slice(0, 4))} aria-label={t("gatewayPage.weight")} />
          {weight !== String(r.weight) && weight !== "" && <button className="btn small primary" onClick={() => save({ weight: Math.max(1, Number(weight)) }, t("gatewayPage.weightSaved"))}>{t("common.save")}</button>}
          <span className="tiny muted">{t("gatewayPage.weightHint")}</span>
        </div>
      </div>

      <div className="gw-block">
        <button className="link gw-adv" onClick={() => setAdvanced((v) => !v)}>{t(advanced ? "gatewayPage.hideAdvanced" : "gatewayPage.showAdvanced")}</button>
        {advanced && (
          <div className="stack6">
            <label className="field">
              <span className="small">{t("gatewayPage.modelMap")} <em className="muted tiny">{t("gatewayPage.modelMapHint")}</em></span>
              <textarea className="input mono gw-map" rows={3} value={map} onChange={(e) => setMap(e.target.value)} placeholder={"gpt-5.5=glm-5\n*=deepseek-v4-pro"} />
            </label>
            <div className="row gap6">
              <button className="btn small primary" onClick={() => save({
                modelMap: map.split("\n").map((l) => l.split("=").map((x) => x.trim())).filter((p) => p.length === 2 && p[0] && p[1]) as [string, string][],
              }, t("gatewayPage.modelMapSaved"))}>{t("gatewayPage.saveMap")}</button>
              <span className="grow" />
              <button className="btn small" onClick={() => save({ enabled: !r.enabled }, t(r.enabled ? "gatewayPage.routePaused" : "gatewayPage.routeResumed"))}>{t(r.enabled ? "common.pauseRoute" : "common.resumeRoute")}</button>
              <button className="btn small danger" onClick={onDelete}>{t("gatewayPage.deleteRoute")}</button>
            </div>
            <span className="tiny muted">{t("gatewayPage.stopHint")}</span>
          </div>
        )}
      </div>
    </div>
  );
}

/** Error breaker settings: when to pause a forward that keeps failing, and for how long. */
function BreakerSettings({ cfg, setStatus, flash }: { cfg: GatewayBreaker; setStatus: (s: GatewayStatus) => void; flash: Flash }) {
  const [threshold, setThreshold] = useState(String(cfg.threshold));
  const [cooldown, setCooldown] = useState(String(cfg.cooldownSecs));
  useEffect(() => { setThreshold(String(cfg.threshold)); setCooldown(String(cfg.cooldownSecs)); }, [cfg.threshold, cfg.cooldownSecs]);
  const dirty = threshold !== String(cfg.threshold) || cooldown !== String(cfg.cooldownSecs);
  const save = async (next: GatewayBreaker, msg: string) => {
    try {
      setStatus(await api.gatewaySetBreaker(next));
      flash(msg);
    } catch (e) {
      flash(errText(e), true);
    }
  };
  return (
    <section className="sgroup">
      <h2>{t("gatewayPage.breaker")}</h2>
      <div className="srow stacked">
        <ToggleRow on={cfg.enabled} title={t("gatewayPage.breakerToggle")} hint={t("gatewayPage.breakerDesc")} label={t("gatewayPage.breaker")}
          onChange={() => save({ ...cfg, enabled: !cfg.enabled }, t(cfg.enabled ? "gatewayPage.breakerOff" : "gatewayPage.breakerOn"))} />
        {cfg.enabled && (
          <div className="row gap6 gw-breaker-form">
            <span className="small row gap6 gw-breaker-form">
              {tx("gatewayPage.breakerForm", {
                threshold: <input className="input mono gw-weight-input" value={threshold} aria-label={t("gatewayPage.thresholdLabel")} onChange={(e) => setThreshold(e.target.value.replace(/\D/g, "").slice(0, 3))} />,
                cooldown: <input className="input mono gw-weight-input" value={cooldown} aria-label={t("gatewayPage.cooldownLabel")} onChange={(e) => setCooldown(e.target.value.replace(/\D/g, "").slice(0, 4))} />,
              })}
            </span>
            {dirty && threshold !== "" && cooldown !== "" && (
              <button className="btn small primary" onClick={() => save({ ...cfg, threshold: Math.max(1, Number(threshold)), cooldownSecs: Math.max(5, Number(cooldown)) }, t("gatewayPage.breakerSaved"))}>{t("common.save")}</button>
            )}
          </div>
        )}
      </div>
    </section>
  );
}
