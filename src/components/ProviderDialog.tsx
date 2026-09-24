import { useEffect, useMemo, useRef, useState } from "react";
import { useEscape } from "../hooks";
import { type AgentState, type ApiKind, type GatewayRouteView, type GatewayStatus, type ProviderInput, api, isProjectId } from "../api";
import { type Draft, type ViewProvider, isVisible, keys, viewModels } from "../draft";
import { API_LABEL, GATEWAY_KEY, ONLY_API, gatewayCapable, gatewayPoolBase, gatewayPoolIds } from "../services";
import { Dropdown } from "./Dropdown";
import { Icon } from "./icons";
import { TemplatePicker } from "./TemplatePicker";
import type { Template } from "../templates";
import { type TKey, t, tn } from "../i18n";
import { scrub } from "../privacy";

/** What the dialog asks the app to do; every part is optional. */
export interface ProviderSave {
  /** Provider edits (null = nothing changed). */
  input: ProviderInput | null;
  draftKey?: string;
  /** Codex / Claude Code: this provider's own model list. */
  codexModels?: string[] | null;
  /** Claude Code: model per role (default / opus / sonnet / haiku / subagent). */
  roles?: Record<string, string> | null;
  /** ZCode / MiMo: visibility per model id, plus models to add. */
  models?: { visible: Record<string, boolean>; added: string[] } | null;
  /** Switch how the agent reaches the provider. */
  connect?: "direct" | "gateway" | null;
  /** New provider on the gateway's unified entry: make sure the gateway runs. */
  unified?: boolean;
  /**
   * Template the agent can't reach directly (its only protocol isn't offered): save this to
   * the library, forward it through the gateway, and point the new provider there.
   */
  viaForward?: { name: string; baseUrl: string; api: ApiKind; apiKey: string; models: string[]; officialAuth?: boolean } | null;
}

interface Props {
  st: AgentState;
  draft: Draft;
  /** Provider being edited; null = add new. */
  editing: ViewProvider | null;
  /** Gateway state of the edited provider: undefined = direct; null = its route is gone. */
  gatewayRoute: GatewayRouteView | null | undefined;
  /** Resolves when handled (the dialog is closed by then, or stays open after a reported error). */
  onSave: (s: ProviderSave) => Promise<void>;
  onClose: () => void;
  gateway: GatewayStatus | null;
  /** Turns the gateway on if needed and returns its status. */
  ensureGateway: () => Promise<GatewayStatus>;
}

const API_OPTIONS: { v: ApiKind; label: string; hint: TKey }[] = [
  { v: "responses", label: "Responses", hint: "providerDialog.apiResponsesHint" },
  { v: "chat", label: "Chat", hint: "providerDialog.apiChatHint" },
  { v: "anthropic", label: "Anthropic", hint: "providerDialog.apiAnthropicHint" },
];

const same = (a: string[], b: string[]) => a.length === b.length && a.every((x) => b.includes(x));

/** Claude Code roles. The label is also the tag the backend puts on the model that fills
 * each (the backend renders it in the same UI language). */
const ROLES: { role: string; label: TKey; hint: TKey }[] = [
  { role: "default", label: "providerDialog.roleDefault", hint: "providerDialog.roleDefaultHint" },
  { role: "opus", label: "providerDialog.roleOpus", hint: "providerDialog.roleOpusHint" },
  { role: "sonnet", label: "providerDialog.roleSonnet", hint: "providerDialog.roleSonnetHint" },
  { role: "haiku", label: "providerDialog.roleHaiku", hint: "providerDialog.roleHaikuHint" },
  { role: "subagent", label: "providerDialog.roleSubagent", hint: "providerDialog.roleSubagentHint" },
];
const HERMES_ROLES: typeof ROLES = [{ role: "default", label: "providerDialog.roleDefault", hint: "providerDialog.hermesDefaultHint" }];

export function ProviderDialog({ st, draft, editing, gatewayRoute, onSave, onClose, gateway, ensureGateway }: Props) {
  const isNew = !editing || !!editing.isNew;
  const codex = st.id === "codex";
  const claude = st.id === "claude";
  /** Codex and Claude Code keep one plain list per provider (not per-model entries). */
  const listMode = codex || claude;
  const only = ONLY_API[st.id];
  /** Claude: the entry found in settings.json that AgentPlus does not manage yet. */
  const unmanaged = claude && editing?.id === "settings-env";
  /** Model roles this agent lets you assign (Claude: several; Hermes: the default model). */
  const roleList = claude ? ROLES : st.id === "hermes" ? HERMES_ROLES : [];
  const [name, setName] = useState(editing?.name ?? "");
  const [baseUrl, setBaseUrl] = useState(editing?.baseUrl ?? "");
  const [kind, setKind] = useState<ApiKind>(only ?? editing?.api ?? "chat");
  const [key, setKey] = useState("");
  /** Codex: keep the ChatGPT sign-in while requests go to this provider. */
  const [officialAuth, setOfficialAuth] = useState(editing?.officialAuth ?? false);
  const viaGateway = gatewayRoute !== undefined;
  const [connect, setConnect] = useState<"direct" | "gateway">(viaGateway ? "gateway" : "direct");
  /** An existing provider that already points at the gateway's unified entry (or a combination of forwards). */
  const onUnified = !isNew && gatewayPoolIds(editing?.baseUrl) !== null;
  /** New provider: use the gateway's unified entry instead of an address. */
  const [unifiedNew, setUnifiedNew] = useState(false);
  /** Forwards the unified provider may use; empty = all of them. */
  const [pool, setPool] = useState<string[]>(() => (isNew ? [] : gatewayPoolIds(editing?.baseUrl) ?? []));
  const poolBase = gatewayPoolBase(gateway?.port ?? 18650, pool);
  const [err, setErr] = useState<string | null>(null);
  const first = useRef<HTMLInputElement>(null);
  const [tpl, setTpl] = useState<Template | null>(null);
  /** Template without the agent's only protocol: reached through a gateway forward. */
  const tplForward = !!tpl && !!only && !tpl.endpoints[only];

  // ---- model list (per agent)
  const isCurrent = codex && !!editing && st.currentProvider === editing.id;
  const pmOp = editing ? draft[keys.providerModels(editing.id)] : undefined;
  const codexOriginal = editing && !isNew ? editing.models.filter((m) => m.visible).map((m) => m.id) : [];
  const codexStart = pmOp && pmOp.op === "set_provider_models" ? pmOp.models : codexOriginal;
  const perModels = editing && !isNew && !listMode ? viewModels(editing.id, editing.models, draft).filter((m) => !m.isDeleted) : [];
  const [checked, setChecked] = useState<string[]>(() =>
    isNew ? editing?.models.map((m) => m.id) ?? [] : listMode ? codexStart : perModels.filter((m) => isVisible(editing!.id, m, draft)).map((m) => m.id),
  );
  const [fetched, setFetched] = useState<string[]>([]);
  const [manual, setManual] = useState("");
  const [fetching, setFetching] = useState(false);
  const [filter, setFilter] = useState("");
  const rolesOp = editing ? draft[keys.roles(editing.id)] : undefined;
  const rolesOriginal: Record<string, string> = {};
  for (const m of editing?.models ?? []) for (const r of roleList) if (m.tags.some((g) => g.id === `role:${r.role}`)) rolesOriginal[r.role] = m.id;
  const [roles, setRoles] = useState<Record<string, string>>(rolesOp && rolesOp.op === "set_model_roles" ? rolesOp.roles : rolesOriginal);

  const candidates = useMemo(() => {
    const base = isNew ? [] : codex ? [...(st.catalog ?? []).map((m) => m.id), ...codexStart] : claude ? [...(editing?.models ?? []).map((m) => m.id), ...codexStart] : perModels.map((m) => m.id);
    return [...new Set([...base, ...checked, ...fetched])];
  }, [fetched, checked.length]);
  const shown = candidates.filter((m) => !filter.trim() || m.toLowerCase().includes(filter.trim().toLowerCase()));

  // Focus the first field once, when the dialog opens (not on every parent re-render).
  useEffect(() => { first.current?.focus(); }, []);
  useEscape(onClose);

  const urlOk = /^https?:\/\/\S+$/.test(baseUrl.trim());
  const gw = connect === "gateway";
  const [saving, setSaving] = useState(false);
  const canSave = !saving && name.trim() !== "" && (gw || unifiedNew || urlOk) && (!tpl || unifiedNew || key.trim() !== "");

  const fetchList = async () => {
    setErr(null);
    setFetching(true);
    try {
      let list: string[];
      if (unifiedNew || onUnified) {
        // Every model the chosen forwards (or all of them) offer.
        const g = await ensureGateway();
        list = await api.fetchModelsUrl(gatewayPoolBase(g.port, pool), null, "chat");
      } else if (!isNew && gw && gatewayRoute) {
        // Forwarded through the gateway, but the list still comes from the original endpoint.
        list = await api.fetchModelsLib(gatewayRoute.library);
      } else if (!isNew && !key.trim() && editing && baseUrl.trim() === (editing.baseUrl ?? "")) {
        list = await api.fetchModels(st.id, editing.id);
      } else {
        list = await api.fetchModelsUrl(baseUrl.trim(), key.trim() || null, tplForward ? tpl!.api : kind);
      }
      setFetched(list);
      if (checked.length === 0) setChecked(list.slice(0, 20));
    } catch (e) {
      setErr(t("providerDialog.fetchFailed", { err: String(e) }));
    } finally {
      setFetching(false);
    }
  };

  const toggle = (m: string) => setChecked((l) => (l.includes(m) ? l.filter((x) => x !== m) : [...l, m]));
  const addManual = () => {
    const ids = manual.split(/[\s,]+/).map((s) => s.trim()).filter(Boolean);
    setChecked((l) => [...l, ...ids.filter((i) => !l.includes(i))]);
    setFetched((l) => [...l, ...ids.filter((i) => !l.includes(i))]);
    setManual("");
  };

  const pickTpl = (tp: Template | null) => {
    setTpl(tp);
    setFetched([]);
    setErr(null);
    if (!tp) return;
    setUnifiedNew(false);
    setName(tp.name);
    const k = only && tp.endpoints[only] ? only : tp.api;
    setKind(only ?? k);
    setBaseUrl(tp.endpoints[k]!);
    if (!(codex && isNew)) setChecked(tp.models);
  };
  const setProto = (k: ApiKind) => {
    setKind(k);
    // A template's other protocols live at their own address.
    if (tpl?.endpoints[k] && baseUrl.trim() === tpl.endpoints[kind]) setBaseUrl(tpl.endpoints[k]!);
  };

  /** Hands the result to the app; the button stays disabled until it is done (no double save). */
  const submit = async (out: ProviderSave) => {
    setSaving(true);
    try {
      await onSave(out);
    } catch (e) {
      setErr(String(e).replace(/^Error: /, ""));
    } finally {
      setSaving(false);
    }
  };

  const save = () => {
    if (!canSave) return;
    if (tplForward) {
      void submit({ input: null, viaForward: { name: name.trim(), baseUrl: baseUrl.trim(), api: tpl!.api, apiKey: key.trim(), models: codex ? tpl!.models : checked, officialAuth: codex ? officialAuth : undefined } });
      return;
    }
    const url = onUnified ? poolBase : baseUrl.trim();
    const authChanged = codex && officialAuth !== (editing?.officialAuth ?? false);
    const changed = isNew || unmanaged || name.trim() !== editing!.name || (!gw && (url !== (editing!.baseUrl ?? "") || kind !== editing!.api)) || !!key.trim() || authChanged;
    const auth = codex ? { officialAuth } : {};
    const out: ProviderSave = {
      input: changed
        ? unifiedNew
          ? { id: null, name: name.trim(), baseUrl: poolBase, api: kind, apiKey: GATEWAY_KEY, models: checked, ...auth }
          : { id: isNew ? null : editing!.id, name: name.trim(), baseUrl: url, api: kind, apiKey: key.trim() || null, models: isNew ? checked : [], ...auth }
        : null,
      unified: unifiedNew,
      draftKey: editing?.draftKey,
      connect: !isNew && connect !== (viaGateway ? "gateway" : "direct") ? connect : null,
    };
    if (!isNew && listMode && !unmanaged) out.codexModels = same(checked, codexOriginal) ? null : checked;
    if (!isNew && roleList.length && !unmanaged) {
      const clean = Object.fromEntries(Object.entries(roles).filter(([, v]) => v));
      const orig = JSON.stringify(Object.entries(rolesOriginal).sort());
      out.roles = JSON.stringify(Object.entries(clean).sort()) === orig ? null : clean;
    }
    if (!isNew && !listMode) {
      const visible: Record<string, boolean> = {};
      for (const m of perModels) visible[m.id] = checked.includes(m.id);
      out.models = { visible, added: checked.filter((m) => !perModels.some((p) => p.id === m)) };
    }
    void submit(out);
  };

  const keyHint = codex ? t("providerDialog.keyCodex")
    : claude ? t("providerDialog.keyClaude")
    : st.id === "opencode" || isProjectId(st.id) ? t("providerDialog.keyOpencode")
    : st.id === "zcode" ? t("providerDialog.keyZcode")
    : st.id === "mimo" ? t("providerDialog.keyMimo")
    : st.id === "hermes" ? t("providerDialog.keyHermes")
    : st.id === "gemini" ? t("providerDialog.keyGemini")
    : st.id === "qwen" ? t("providerDialog.keyQwen")
    : st.id === "kilo" ? t("providerDialog.keyKilo")
    : st.id === "pi" ? t("providerDialog.keyPi")
    : t("providerDialog.keyOther", { agent: st.name });

  return (
    <div className="modal-bg" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="modal wide" role="dialog" aria-modal="true" aria-label={isNew ? t("providerDialog.addTitle") : t("providerDialog.editTitle")}>
        <div className="modal-head">
          <h2>{isNew ? t("providerDialog.addHead", { agent: st.name }) : t("providerDialog.editHead", { name: editing!.name })}</h2>
          <button className="icon-btn" aria-label={t("common.close")} onClick={onClose}><Icon.close /></button>
        </div>

        <div className="modal-body">
          {isNew && gatewayCapable(st.id) && !unifiedNew && <TemplatePicker value={tpl} onPick={pickTpl} />}
          {tplForward && (
            <div className="gw-toggle on">
              <Icon.gateway size={16} />
              <div className="grow minw0">
                <div className="small strong">{t("providerDialog.viaGatewayTitle")}</div>
                <div className="tiny muted">{t("providerDialog.viaGatewayDesc", { vendor: tpl!.vendor, only: API_LABEL[only!], agent: st.name, api: API_LABEL[tpl!.api] })}</div>
              </div>
            </div>
          )}
          <div className="field">
            <label htmlFor="pd-name">{t("common.name")}</label>
            <input id="pd-name" ref={first} className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("providerDialog.namePlaceholder")} />
          </div>

          {!isNew && editing?.baseUrl && !onUnified && gatewayCapable(st.id) && (
            <div className={`gw-toggle${gw ? " on" : ""}`}>
              <Icon.gateway size={16} />
              <div className="grow minw0">
                <div className="small strong">{t("providerDialog.useGateway")}</div>
                <div className="tiny muted">
                  {gw
                    ? viaGateway && gatewayRoute
                      ? t("providerDialog.gwOnRoute", { url: gatewayRoute.upstreamUrl ?? "", api: API_LABEL[gatewayRoute.upstreamApi] })
                      : t("providerDialog.gwOnNew")
                    : viaGateway
                      ? t("providerDialog.gwOffWas")
                      : t("providerDialog.gwOff")}
                </div>
              </div>
              <button type="button" className={`switch${gw ? " on" : ""}`} role="switch" aria-checked={gw} aria-label={t("providerDialog.useGateway")}
                onClick={() => {
                  if (gw) {
                    setConnect("direct");
                    // Leaving the gateway: show the upstream it forwarded to, not the local address.
                    if (viaGateway && gatewayRoute?.upstreamUrl && baseUrl === (editing.baseUrl ?? "")) {
                      setBaseUrl(gatewayRoute.upstreamUrl);
                      if (!only) setKind(gatewayRoute.upstreamApi);
                    }
                  } else setConnect("gateway");
                }}><span /></button>
            </div>
          )}
          {!isNew && onUnified && (
            <>
              <div className="gw-toggle on">
                <Icon.gateway size={16} />
                <div className="grow minw0">
                  <div className="small strong">{pool.length ? t("providerDialog.unifiedHeadPicked") : t("providerDialog.unifiedHeadAll")}</div>
                  <div className="tiny muted">{t("providerDialog.unifiedDesc")}</div>
                </div>
              </div>
              <ForwardPicker routes={gateway?.routes ?? []} value={pool} onChange={setPool} />
            </>
          )}
          {isNew && gatewayCapable(st.id) && (
            <div className={`gw-toggle${unifiedNew ? " on" : ""}`}>
              <Icon.gateway size={16} />
              <div className="grow minw0">
                <div className="small strong">{t("providerDialog.useGateway")}</div>
                <div className="tiny muted">
                  {unifiedNew
                    ? pool.length
                      ? tn("providerDialog.newPoolPicked", pool.length, { url: poolBase })
                      : t("providerDialog.newPoolAll", { url: poolBase })
                    : t("providerDialog.newGwOff")}
                </div>
              </div>
              <button type="button" className={`switch${unifiedNew ? " on" : ""}`} role="switch" aria-checked={unifiedNew} aria-label={t("providerDialog.useGateway")}
                onClick={() => {
                  if (!unifiedNew) setTpl(null);
                  setUnifiedNew((v) => !v);
                  if (!unifiedNew && !name.trim()) setName(t("providerDialog.gatewayName"));
                }}><span /></button>
            </div>
          )}

          {unifiedNew && <ForwardPicker routes={gateway?.routes ?? []} value={pool} onChange={setPool} />}

          {unifiedNew && !only && (
            <div className="field">
              <span className="field-label">{t("providerDialog.gatewayProtocol")}</span>
              <div className="seg">
                {API_OPTIONS.map((o) => <button key={o.v} type="button" className={kind === o.v ? "on" : ""} title={t(o.hint)} onClick={() => setKind(o.v)}>{o.label}</button>)}
              </div>
              <em className="muted tiny">{t("providerDialog.gatewayProtocolNote")}</em>
            </div>
          )}

          {!gw && !unifiedNew && !onUnified && (
            <>
              <div className="field">
                <label htmlFor="pd-url">{t("providerDialog.baseUrl")}</label>
                <input id="pd-url" className="input mono sensitive" value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="https://api.example.com/v1" />
                {baseUrl && !urlOk && <em className="field-err">{t("providerDialog.urlBad")}</em>}
              </div>
              <div className="form2">
                <div className="field">
                  <span className="field-label">{t("providerDialog.apiType")}</span>
                  <div className="seg">
                    {(only === "gemini" ? [{ v: "gemini" as ApiKind, label: "Gemini", hint: "providerDialog.apiGeminiHint" as TKey }] : API_OPTIONS).map((o) => {
                      const missing = !!tpl && !only && !tpl.endpoints[o.v];
                      return (
                        <button key={o.v} type="button" className={kind === o.v ? "on" : ""} disabled={(!!only && o.v !== only) || missing}
                          title={only && o.v !== only ? t("providerDialog.onlySupports", { agent: st.name, api: API_LABEL[only] }) : missing ? t("providerDialog.vendorNoApi", { vendor: tpl!.vendor, api: o.label }) : t(o.hint)} onClick={() => setProto(o.v)}>{o.label}</button>
                      );
                    })}
                  </div>
                </div>
                <div className="field">
                  <label htmlFor="pd-key">{t("providerDialog.apiKey")}</label>
                  <input id="pd-key" className="input mono" type="password" autoComplete="off" value={key} onChange={(e) => setKey(e.target.value)}
                    placeholder={!isNew && editing?.hasKey ? t("providerDialog.keySet") : "sk-..."} />
                  <em className="muted tiny">
                    {tplForward ? t("providerDialog.keyForward") : keyHint}
                    {tpl && <> <button type="button" className="link" onClick={() => api.openUrl(tpl.keyUrl).catch(() => undefined)}>{t("providerDialog.getKey", { vendor: tpl.vendor })}</button></>}
                  </em>
                </div>
              </div>
            </>
          )}

          {codex && (
            <div className={`gw-toggle${officialAuth ? " on" : ""}`}>
              <Icon.key size={16} />
              <div className="grow minw0">
                <div className="small strong">{t("providerDialog.officialAuth")}</div>
                <div className="tiny muted">{officialAuth ? t("providerDialog.officialAuthOn") : t("providerDialog.officialAuthOff")}</div>
              </div>
              <button type="button" className={`switch${officialAuth ? " on" : ""}`} role="switch" aria-checked={officialAuth} aria-label={t("providerDialog.officialAuth")}
                onClick={() => setOfficialAuth((v) => !v)}><span /></button>
            </div>
          )}

          <div className="field">
            <div className="row between">
              <span className="field-label">{t("providerDialog.modelList")} <em className="muted tiny">{t("providerDialog.modelListScope", { agent: st.name })}</em></span>
              <button type="button" className="btn small" disabled={(!gw && !unifiedNew && !urlOk) || fetching} onClick={fetchList}>
                <Icon.refresh size={12} />{fetching ? t("providerDialog.fetching") : t("providerDialog.fetchFromUrl")}
              </button>
            </div>
            <em className="muted tiny">
              {codex
                ? isNew
                  ? t("providerDialog.codexNewNote")
                  : isCurrent
                    ? t("providerDialog.codexCurrentNote")
                    : t("providerDialog.codexOtherNote")
                : claude
                  ? unmanaged
                    ? t("providerDialog.claudeUnmanagedNote")
                    : t("providerDialog.claudeNote")
                  : t("providerDialog.pickNote", { agent: st.name })}
            </em>
            {!(codex && isNew) && !unmanaged && (
              <>
                <div className="mpick-bar">
                  <span className="tiny muted">{t("providerDialog.selectedN", { n: checked.length })}</span>
                  {candidates.length > 8 && <input className="input mono mpick-filter" value={filter} onChange={(e) => setFilter(e.target.value)} placeholder={t("providerDialog.filter")} />}
                  <span className="grow" />
                  {candidates.length > 0 && <button type="button" className="link tiny" onClick={() => setChecked(checked.length === candidates.length ? [] : candidates)}>{checked.length === candidates.length ? t("providerDialog.selectNone") : t("providerDialog.selectAll")}</button>}
                </div>
                <div className="pick-list wide">
                  {candidates.length === 0 && <div className="muted small">{codex && !isCurrent ? t("providerDialog.codexEmpty") : t("providerDialog.empty")}</div>}
                  {shown.map((m) => (
                    <label key={m} className="pick">
                      <input type="checkbox" checked={checked.includes(m)} onChange={() => toggle(m)} />
                      <span className="mono small">{m}</span>
                      {fetched.includes(m) && !codexStart.includes(m) && !perModels.some((p) => p.id === m) && !isNew && <span className="mtag new">{t("providerDialog.tagNew")}</span>}
                    </label>
                  ))}
                </div>
                <div className="row gap6">
                  <input className="input mono grow" value={manual} onChange={(e) => setManual(e.target.value)}
                    onKeyDown={(e) => { if (e.key === "Enter") { e.preventDefault(); addManual(); } }} placeholder={t("providerDialog.manualPlaceholder")} />
                  <button type="button" className="btn" disabled={!manual.trim()} onClick={addManual}>{t("common.add")}</button>
                </div>
              </>
            )}
          </div>
          {roleList.length > 0 && !isNew && !unmanaged && (
            <div className="field">
              <span className="field-label">{t("providerDialog.roleAssign")} <em className="muted tiny">{claude ? t("providerDialog.roleScopeClaude") : t("providerDialog.roleScopeHermes")}</em></span>
              <div className="roles-grid">
                {roleList.map((r) => (
                  <div key={r.role} className="role-row" title={t(r.hint)}>
                    <span className="small strong">{t(r.label)}</span>
                    <Dropdown value={roles[r.role] ?? ""} label={t(r.label)} onChange={(v) => setRoles((x) => ({ ...x, [r.role]: v }))}
                      options={[{ value: "", label: t("providerDialog.roleUnset"), hint: r.role === "default" ? t("providerDialog.roleAgentDefault", { agent: st.name }) : t("providerDialog.roleInherit") }, ...checked.map((m) => ({ value: m, label: m }))]} />
                  </div>
                ))}
              </div>
              <em className="muted tiny">{t("providerDialog.roleNote")}</em>
            </div>
          )}
          {err && <div className="err">{scrub(err)}</div>}
        </div>

        <div className="modal-foot">
          <span className="muted tiny grow">{t("providerDialog.pendingNote")}</span>
          <button className="btn" onClick={onClose}>{t("common.cancel")}</button>
          <button className="btn primary" disabled={!canSave} onClick={save}>{saving ? t("providerDialog.saving") : isNew ? t("common.add") : t("common.save")}</button>
        </div>
      </div>
    </div>
  );
}

/** Which gateway forwards a unified provider may use; none checked = all of them. */
function ForwardPicker({ routes, value, onChange }: { routes: GatewayRouteView[]; value: string[]; onChange: (ids: string[]) => void }) {
  const usable = routes.filter((r) => !r.upstreamMissing);
  // Picked earlier but since deleted: still listed so they can be unticked.
  const gone = value.filter((id) => !usable.some((r) => r.id === id));
  const toggle = (id: string) => onChange(value.includes(id) ? value.filter((x) => x !== id) : [...value, id]);
  return (
    <div className="field">
      <div className="row between">
        <span className="field-label">{t("providerDialog.fwdLabel")} <em className="muted tiny">{value.length ? t("providerDialog.fwdPicked", { n: value.length }) : t("providerDialog.fwdAll")}</em></span>
        {value.length > 0 && <button type="button" className="link tiny" onClick={() => onChange([])}>{t("providerDialog.fwdReset")}</button>}
      </div>
      {usable.length === 0 && gone.length === 0 ? (
        <em className="muted tiny">{t("providerDialog.fwdNone")}</em>
      ) : (
        <div className="fwd-pick">
          {usable.map((r) => {
            const b = r.breaker && r.breaker.state !== "closed" ? r.breaker : null;
            return (
              <label key={r.id} className={`fwd-opt${value.includes(r.id) ? " on" : ""}`} title={scrub(b?.reason) ?? `${scrub(r.upstreamUrl) ?? ""}${r.models.length ? tn("providerDialog.fwdModels", r.models.length) : ""}`}>
                <input type="checkbox" checked={value.includes(r.id)} onChange={() => toggle(r.id)} />
                <span className={`api-chip api-${r.upstreamApi}`}>{API_LABEL[r.upstreamApi]}</span>
                <span className="small strong ellipsis">{r.name}</span>
                {!r.enabled ? <span className="chip-muted">{t("providerDialog.paused")}</span> : b ? <span className="chip-bad">{t("providerDialog.tripped")}</span> : null}
              </label>
            );
          })}
          {gone.map((id) => (
            <label key={id} className="fwd-opt on" title={t("providerDialog.fwdGone")}>
              <input type="checkbox" checked onChange={() => toggle(id)} />
              <span className="small mono ellipsis">{id}</span>
              <span className="chip-muted">{t("providerDialog.deleted")}</span>
            </label>
          ))}
        </div>
      )}
      <em className="muted tiny">{t("providerDialog.fwdNote")}</em>
    </div>
  );
}
