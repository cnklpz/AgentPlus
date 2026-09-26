import { useEffect, useRef, useState } from "react";
import { type AgentState, type ApiKind, type GatewayRouteView, type GatewayStatus, type ProviderInput, api, isProjectId } from "../api";
import { type Draft, type ViewProvider, isVisible, keys, settingValue, viewModels } from "../draft";
import { API_LABEL, DEFAULT_GATEWAY_PORT, GATEWAY_KEY, ONLY_API, PROTOCOLS, gatewayCapable, gatewayPoolBase, gatewayPoolIds, tripped } from "../services";
import { Dropdown } from "./Dropdown";
import { Icon } from "./icons";
import { Modal } from "./Modal";
import { ErrorBox, Seg, SegMulti, ToggleRow } from "./controls";
import { ModelPicker, useModelPool } from "./ModelPicker";
import { TemplateKeyLink, TemplatePicker } from "./TemplatePicker";
import { type Template, modelsAfter, modelsFor, modelsOn } from "../templates";
import { type TKey, t, tn, tx } from "../i18n";
import { scrub } from "../privacy";
import { errText, isHttpUrl, toggledIn } from "../util";

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
  /** New providers for the other protocols picked (the first one is `input`). */
  extra?: ProviderInput[];
  /**
   * New provider through the gateway (asked for, or the agent's only protocol isn't offered):
   * save each protocol's address to the library, forward each, and point the new provider at
   * the forward (several: their combined entry, which routes by model).
   */
  viaForward?: { name: string; apiKey: string; parts: { name: string; api: ApiKind; baseUrl: string; models: string[] }[]; models: string[]; officialAuth?: boolean } | null;
  /** Agent switch settings to turn on as well (Codex: the official sign-in mix options). */
  settingsOn?: string[];
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

const API_HINT: Record<ApiKind, TKey> = {
  responses: "providerDialog.apiResponsesHint",
  chat: "common.apiHintChat",
  anthropic: "common.apiHintAnthropic",
  gemini: "providerDialog.apiGeminiHint",
};

/** Codex settings that only matter with the official sign-in mix on. */
const MIX_SETTINGS = ["quota_unlock", "hide_usage_banner"];

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
  /** The official sign-in mix options still off (pending changes counted). */
  const mixOff = codex ? st.settings.filter((s) => MIX_SETTINGS.includes(s.key) && settingValue(s, draft) !== true) : [];
  const [mixSync, setMixSync] = useState(true);
  const mixOn = officialAuth && mixSync ? mixOff.map((s) => s.key) : [];
  const viaGateway = gatewayRoute !== undefined;
  const [connect, setConnect] = useState<"direct" | "gateway">(viaGateway ? "gateway" : "direct");
  /** An existing provider that already points at the gateway's unified entry (or a combination of forwards). */
  const onUnified = !isNew && gatewayPoolIds(editing?.baseUrl) !== null;
  /** New provider: use the gateway's unified entry instead of an address. */
  const [unifiedNew, setUnifiedNew] = useState(false);
  /** Forwards the unified provider may use; empty = all of them. */
  const [pool, setPool] = useState<string[]>(() => (isNew ? [] : gatewayPoolIds(editing?.baseUrl) ?? []));
  const poolBase = gatewayPoolBase(gateway?.port ?? DEFAULT_GATEWAY_PORT, pool);
  const [err, setErr] = useState<string | null>(null);
  const first = useRef<HTMLInputElement>(null);
  const [tpl, setTpl] = useState<Template | null>(null);
  /** Template without the agent's only protocol: reached through a gateway forward. */
  const tplForward = !!tpl && !!only && !tpl.endpoints[only];
  /** New provider forwarded through the gateway on request. */
  const [forwardOn, setForwardOn] = useState(false);
  /** The new provider goes through a gateway forward: `kind` / `apis` are then the upstream's. */
  const viaFwd = tplForward || forwardOn;
  /** The protocol the agent is limited to, unless the gateway converts. */
  const lockApi = viaFwd ? undefined : only;
  /** New provider: the protocols to add it with (a provider, or a forward, each); `kind` is the first. */
  const [apis, setApis] = useState<ApiKind[]>([kind]);
  const multi = isNew && !unifiedNew;

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
  const [fetching, setFetching] = useState(false);
  const rolesOp = editing ? draft[keys.roles(editing.id)] : undefined;
  const rolesOriginal: Record<string, string> = {};
  for (const m of editing?.models ?? []) for (const r of roleList) if (m.tags.some((g) => g.id === `role:${r.role}`)) rolesOriginal[r.role] = m.id;
  const [roles, setRoles] = useState<Record<string, string>>(rolesOp && rolesOp.op === "set_model_roles" ? rolesOp.roles : rolesOriginal);

  const [modelPool, addToPool, resetPool] = useModelPool(() => [
    ...(isNew ? [] : codex ? [...(st.catalog ?? []).map((m) => m.id), ...codexStart] : claude ? [...(editing?.models ?? []).map((m) => m.id), ...codexStart] : perModels.map((m) => m.id)),
    ...checked,
  ]);

  // Focus the first field once, when the dialog opens (not on every parent re-render).
  useEffect(() => { first.current?.focus(); }, []);

  const urlOk = isHttpUrl(baseUrl);
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
        await ensureGateway();
        list = await api.gatewayModels(pool);
      } else if (!isNew && gw && gatewayRoute) {
        // Forwarded through the gateway, but the list still comes from the original endpoint.
        list = await api.fetchModelsLib(gatewayRoute.library);
      } else if (!isNew && !key.trim() && editing && baseUrl.trim() === (editing.baseUrl ?? "")) {
        list = await api.fetchModels(st.id, editing.id);
      } else {
        list = await api.fetchModelsUrl(baseUrl.trim(), key.trim() || null, kind);
      }
      setFetched(list);
      addToPool(list);
      if (checked.length === 0) setChecked(list.slice(0, 20));
    } catch (e) {
      setErr(t("common.fetchFailed", { err: errText(e) }));
    } finally {
      setFetching(false);
    }
  };

  const addManual = (ids: string[]) => {
    addToPool(ids);
    setChecked((l) => [...l, ...ids.filter((i) => !l.includes(i))]);
    setFetched((l) => [...l, ...ids.filter((i) => !l.includes(i))]);
  };

  /** The template's models on protocol `k`, in the picker and ticked. */
  const applyTplModels = (tp: Template, k: ApiKind) => {
    // A new Codex provider has no picker: it keeps the models ticked now.
    if (codex && isNew) return resetPool(checked);
    resetPool(modelsFor(tp, k));
    setChecked(modelsFor(tp, k));
  };
  const pickTpl = (tp: Template | null) => {
    setTpl(tp);
    setForwardOn(false);
    setFetched([]);
    setErr(null);
    // Another template: the list starts over from the models ticked now (templates only show for a new provider).
    if (!tp) return resetPool(checked);
    setUnifiedNew(false);
    setName(tp.name);
    // Without the agent's protocol the template is forwarded, and `kind` is the upstream's.
    const k = only && tp.endpoints[only] ? only : tp.api;
    setKind(k);
    setApis([k]);
    setBaseUrl(tp.endpoints[k]!);
    applyTplModels(tp, k);
  };
  const setProto = (k: ApiKind) => {
    setKind(k);
    if (!tpl) return;
    // A template's other protocols live at their own address, and may serve other models.
    if (tpl.endpoints[k] && baseUrl.trim() === tpl.endpoints[kind]) setBaseUrl(tpl.endpoints[k]!);
    if (modelsFor(tpl, k) !== modelsFor(tpl, kind)) applyTplModels(tpl, k);
  };
  /** New provider: the protocols picked. */
  const pickApis = (next: ApiKind[]) => {
    const k = next[0];
    if (tpl) {
      // A template's other protocols live at their own address, and may serve other models.
      if (tpl.endpoints[k] && baseUrl.trim() === tpl.endpoints[kind]) setBaseUrl(tpl.endpoints[k]!);
      if (!(codex && isNew)) {
        const m = modelsAfter(tpl, apis, next, checked);
        resetPool([...new Set([...next.flatMap((x) => modelsFor(tpl, x)), ...m])]);
        setChecked(m);
      }
    }
    setApis(next);
    setKind(k);
  };
  /** Through the gateway (any protocol, several at once), or direct (the agent's own). */
  const setForward = (on: boolean) => {
    setForwardOn(on);
    if (!on && only) pickApis([only]);
    // Gemini CLI's protocol isn't one a forward's upstream speaks.
    if (on && only === "gemini") pickApis([tpl?.api ?? "chat"]);
  };

  /** Hands the result to the app; the button stays disabled until it is done (no double save). */
  const submit = async (out: ProviderSave) => {
    setSaving(true);
    try {
      await onSave(mixOn.length ? { ...out, settingsOn: mixOn } : out);
    } catch (e) {
      setErr(errText(e));
    } finally {
      setSaving(false);
    }
  };

  const save = () => {
    if (!canSave) return;
    if (multi) {
      const each = apis.map((k) => ({
        api: k,
        name: apis.length > 1 ? t("providerDialog.nameWithApi", { name: name.trim(), api: API_LABEL[k] }) : name.trim(),
        // A template's other protocols live at their own address (unless the address was edited).
        baseUrl: tpl?.endpoints[k] && baseUrl.trim() === tpl.endpoints[kind] ? tpl.endpoints[k]! : baseUrl.trim(),
        models: modelsOn(tpl, k, checked),
      }));
      if (viaFwd) {
        // A new Codex provider has no picker: its forwards get the template's models.
        const parts = codex ? each.map((p) => ({ ...p, models: tpl ? modelsFor(tpl, p.api) : [] })) : each;
        void submit({ input: null, viaForward: { name: name.trim(), apiKey: key.trim(), parts, models: checked, officialAuth: codex ? officialAuth : undefined } });
        return;
      }
      const auth = codex ? { officialAuth } : {};
      const [head, ...rest] = each.map((p): ProviderInput => ({ id: null, name: p.name, baseUrl: p.baseUrl, api: p.api, apiKey: key.trim() || null, models: codex ? checked : p.models, ...auth }));
      void submit({ input: head, extra: rest, draftKey: editing?.draftKey });
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

  const protoOptions = (lockApi === "gemini" ? (["gemini"] as const) : PROTOCOLS).map((v: ApiKind) => {
    const missing = !!tpl && !lockApi && !tpl.endpoints[v];
    return {
      value: v, label: API_LABEL[v], disabled: (!!lockApi && v !== lockApi) || missing,
      title: lockApi && v !== lockApi ? t("providerDialog.onlySupports", { agent: st.name, api: API_LABEL[lockApi] }) : missing ? t("providerDialog.vendorNoApi", { vendor: tpl!.vendor, api: API_LABEL[v] }) : t(API_HINT[v]),
    };
  });

  const foot = (
    <>
      <span className="muted tiny grow hint">{t("common.pendingNote")}</span>
      <button className="btn" onClick={onClose}>{t("common.cancel")}</button>
      <button className="btn primary" disabled={!canSave} onClick={save}>{saving ? t("common.saving") : isNew ? t("common.add") : t("common.save")}</button>
    </>
  );
  return (
    <Modal label={isNew ? t("common.addProvider") : t("common.editProvider")} wide onClose={onClose}
      title={isNew ? t("providerDialog.addHead", { agent: st.name }) : t("providerDialog.editHead", { name: editing!.name })} foot={foot}>
      {isNew && gatewayCapable(st.id) && !unifiedNew && <TemplatePicker value={tpl} onPick={pickTpl} />}
      <div className="field">
        <label htmlFor="pd-name">{t("common.name")}</label>
        <input id="pd-name" ref={first} className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("common.providerNamePlaceholder")} />
      </div>
      {isNew && gatewayCapable(st.id) && !tpl && (
        <ToggleRow on={unifiedNew} icon={<Icon.gateway size={16} />} title={t("common.useGateway")} keepHint={unifiedNew}
          hint={unifiedNew
            ? pool.length
              ? tn("providerDialog.newPoolPicked", pool.length, { url: poolBase })
              : t("providerDialog.newPoolAll", { url: poolBase })
            : t("providerDialog.newGwOff")}
          onChange={() => {
            if (!unifiedNew) setForwardOn(false);
            setUnifiedNew((v) => !v);
            if (!unifiedNew && !name.trim()) setName(t("providerDialog.gatewayName"));
          }} />
      )}

      {!isNew && editing?.baseUrl && !onUnified && gatewayCapable(st.id) && (
        <ToggleRow on={gw} icon={<Icon.gateway size={16} />} title={t("common.useGateway")}
          hint={gw
            ? viaGateway && gatewayRoute
              ? t("providerDialog.gwOnRoute", { url: gatewayRoute.upstreamUrl ?? "", api: API_LABEL[gatewayRoute.upstreamApi] })
              : t("providerDialog.gwOnNew")
            : viaGateway
              ? t("providerDialog.gwOffWas")
              : t("providerDialog.gwOff")}
          onChange={() => {
            if (gw) {
              setConnect("direct");
              // Leaving the gateway: show the upstream it forwarded to, not the local address.
              if (viaGateway && gatewayRoute?.upstreamUrl && baseUrl === (editing.baseUrl ?? "")) {
                setBaseUrl(gatewayRoute.upstreamUrl);
                if (!only) setKind(gatewayRoute.upstreamApi);
              }
            } else setConnect("gateway");
          }} />
      )}
      {!isNew && onUnified && (
        <>
          <ToggleRow on icon={<Icon.gateway size={16} />} title={pool.length ? t("providerDialog.unifiedHeadPicked") : t("providerDialog.unifiedHeadAll")}
            hint={t("providerDialog.unifiedDesc")} />
          <ForwardPicker routes={gateway?.routes ?? []} value={pool} onChange={setPool} />
        </>
      )}
      {unifiedNew && <ForwardPicker routes={gateway?.routes ?? []} value={pool} onChange={setPool} />}

      {unifiedNew && !only && (
        <div className="field">
          <span className="field-label">{t("providerDialog.gatewayProtocol")}</span>
          <Seg value={kind} onChange={setKind} label={t("providerDialog.gatewayProtocol")}
            options={PROTOCOLS.map((v) => ({ value: v, label: API_LABEL[v], title: t(API_HINT[v]) }))} />
          <em className="muted tiny hint">{t("providerDialog.gatewayProtocolNote")}</em>
        </div>
      )}

      {!gw && !unifiedNew && !onUnified && (
        <>
          <div className="field">
            <label htmlFor="pd-url">{t("common.baseUrlLabel")}</label>
            <input id="pd-url" className="input mono sensitive" value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="https://api.example.com/v1" />
            {baseUrl && !urlOk && <em className="field-err">{t("common.urlInvalid")}</em>}
          </div>
          <div className="form2">
            <div className="field">
              <span className="field-label">{multi ? t("providerDialog.apiTypeMulti") : t("providerDialog.apiType")}</span>
              {multi
                ? <SegMulti value={apis} onChange={pickApis} label={t("providerDialog.apiTypeMulti")} options={protoOptions} />
                : <Seg value={kind} onChange={setProto} label={t("providerDialog.apiType")} options={protoOptions} />}
              {multi && apis.length > 1 && <em className="muted tiny hint">{viaFwd ? t("providerDialog.multiForward") : t("providerDialog.multiDirect")}</em>}
            </div>
            <div className="field">
              <label htmlFor="pd-key">{t("common.apiKeyLabel")}</label>
              <input id="pd-key" className="input mono" type="password" autoComplete="off" value={key} onChange={(e) => setKey(e.target.value)}
                placeholder={!isNew && editing?.hasKey ? t("common.keyKeepPlaceholder") : "sk-..."} />
              <em className={`muted tiny${tpl ? "" : " hint"}`}>
                {viaFwd ? t("providerDialog.keyForward") : keyHint}
                {tpl && <TemplateKeyLink tpl={tpl} />}
              </em>
            </div>
          </div>
        </>
      )}

      {codex && (
        <ToggleRow on={officialAuth} onChange={setOfficialAuth} icon={<Icon.key size={16} />} title={t("providerDialog.officialAuth")}
          hint={officialAuth ? t("providerDialog.officialAuthOn") : t("providerDialog.officialAuthOff")} />
      )}
      {officialAuth && mixOff.length > 0 && (
        <label className="check-row mix-sync">
          <input type="checkbox" checked={mixSync} onChange={(e) => setMixSync(e.target.checked)} />
          <span className="grow minw0">
            <span className="small">
              {mixOff.length > 1
                ? t("providerDialog.mixSyncTwo", { a: mixOff[0].label, b: mixOff[1].label })
                : t("providerDialog.mixSyncOne", { a: mixOff[0].label })}
            </span>
            <em className="muted tiny hint">{t("providerDialog.mixSyncNote")}</em>
          </span>
        </label>
      )}

      {isNew && gatewayCapable(st.id) && !unifiedNew && (
        <ToggleRow on={viaFwd} onChange={tplForward ? undefined : setForward} icon={<Icon.gateway size={16} />} title={t("providerDialog.fwdTitle")}
          hint={tplForward
            ? t("providerDialog.viaGatewayDesc", { vendor: tpl!.vendor, only: API_LABEL[only!], agent: st.name, api: API_LABEL[kind] })
            : forwardOn
              ? t("providerDialog.fwdOn")
              : tpl?.session
                ? tx("providerDialog.sessionHint", {
                  vendor: tpl.name,
                  more: <button type="button" className="link" onClick={() => api.openUrl(tpl.session!).catch(() => undefined)}>{t("providerDialog.learnMore")}</button>,
                })
                : t("providerDialog.fwdOff")} />
      )}
      <div className="field">
        <div className="row between">
          <span className="field-label">{t("providerDialog.modelList")} <em className="muted tiny hint">{t("providerDialog.modelListScope", { agent: st.name })}</em></span>
          <button type="button" className="btn small" disabled={(!gw && !unifiedNew && !urlOk) || fetching} onClick={fetchList}>
            <Icon.refresh size={12} />{fetching ? t("common.fetching") : t("common.fetchFromUrl")}
          </button>
        </div>
        <em className={`muted tiny${!codex && claude && unmanaged ? "" : " hint"}`}>
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
          <ModelPicker bar pool={modelPool} checked={checked} onChange={setChecked} onAdd={addManual}
            empty={codex && !isCurrent ? t("providerDialog.codexEmpty") : t("providerDialog.empty")}
            isNew={(m) => !isNew && fetched.includes(m) && !codexStart.includes(m) && !perModels.some((p) => p.id === m)} />
        )}
      </div>
      {roleList.length > 0 && !isNew && !unmanaged && (
        <div className="field">
          <span className="field-label">{t("providerDialog.roleAssign")} <em className="muted tiny hint">{claude ? t("providerDialog.roleScopeClaude") : t("providerDialog.roleScopeHermes")}</em></span>
          <div className="roles-grid">
            {roleList.map((r) => (
              <div key={r.role} className="role-row" title={t(r.hint)}>
                <span className="small strong">{t(r.label)}</span>
                <Dropdown value={roles[r.role] ?? ""} label={t(r.label)} onChange={(v) => setRoles((x) => ({ ...x, [r.role]: v }))}
                  options={[{ value: "", label: t("providerDialog.roleUnset"), hint: r.role === "default" ? t("providerDialog.roleAgentDefault", { agent: st.name }) : t("providerDialog.roleInherit") }, ...checked.map((m) => ({ value: m, label: m }))]} />
              </div>
            ))}
          </div>
          <em className="muted tiny hint">{t("providerDialog.roleNote")}</em>
        </div>
      )}
      {err && <ErrorBox text={err} />}
    </Modal>
  );
}

/** Which gateway forwards a unified provider may use; none checked = all of them. */
function ForwardPicker({ routes, value, onChange }: { routes: GatewayRouteView[]; value: string[]; onChange: (ids: string[]) => void }) {
  const usable = routes.filter((r) => !r.upstreamMissing);
  // Picked earlier but since deleted: still listed so they can be unticked.
  const gone = value.filter((id) => !usable.some((r) => r.id === id));
  const toggle = (id: string) => onChange(toggledIn(value, id));
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
            const b = tripped(r);
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
      <em className="muted tiny hint">{t("providerDialog.fwdNote")}</em>
    </div>
  );
}
