import { useEffect, useRef, useState } from "react";
import { type AgentId, type AgentState, type ApiKind, api } from "../api";
import { AGENT_NAME, API_LABEL, type Group, ONLY_API, PROTOCOLS, type Protocol, type Use, freeAgents, gatewayCapable, useKey } from "../services";
import { AgentIcon, Icon } from "./icons";
import { Modal } from "./Modal";
import { ErrorBox, Seg, ToggleRow } from "./controls";
import { ModelPicker, useModelPool } from "./ModelPicker";
import { TemplateKeyLink, TemplatePicker } from "./TemplatePicker";
import type { Template } from "../templates";
import { type TKey, t } from "../i18n";
import { errText, isHttpUrl, toggled } from "../util";

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
  /** Add them through a local gateway forward (protocol converted) instead of the address itself. */
  gateway: boolean;
  /** Template: the same key at another protocol's address, for agents that need that protocol. */
  alt: { api: ApiKind; baseUrl: string; addTo: AgentId[] }[];
}

interface Props {
  agents: AgentState[];
  /** null = add a new provider. */
  group: Group | null;
  /** New group inside an existing station: suggested name and address. */
  prefill?: { name: string; baseUrl: string; station: string } | null;
  onSave: (v: ServiceSave) => Promise<void>;
  onClose: () => void;
}

const API_HINT: Record<Protocol, TKey> = {
  responses: "serviceDialog.hintResponses",
  chat: "common.apiHintChat",
  anthropic: "common.apiHintAnthropic",
};

export function ServiceDialog({ agents, group, prefill, onSave, onClose }: Props) {
  const isNew = !group;
  const [name, setName] = useState(group?.name ?? prefill?.name ?? "");
  const [baseUrl, setBaseUrl] = useState(group?.baseUrl ?? prefill?.baseUrl ?? "");
  const [kind, setKind] = useState<ApiKind>(group?.api ?? "responses");
  const [key, setKey] = useState("");
  const [models, setModels] = useState<string[]>(group?.lib?.models ?? []);
  const [pool, addToPool] = useModelPool(() => group?.lib?.models ?? []);
  const [fetching, setFetching] = useState(false);
  const [saving, setSaving] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const first = useRef<HTMLInputElement>(null);

  const editable = (group?.uses ?? []).filter((u) => u.p && u.p.editable && !u.p.isNew && u.state !== "removing");
  const [sync, setSync] = useState<Set<string>>(new Set(editable.map(useKey)));
  const free = freeAgents(agents, group);
  const [addTo, setAddTo] = useState<Set<AgentId>>(new Set());
  const [viaGw, setViaGw] = useState(false);
  const [tpl, setTpl] = useState<Template | null>(null);
  const pickTpl = (tp: Template | null) => {
    setTpl(tp);
    if (!tp) return;
    setName(tp.name);
    setKind(tp.api);
    setBaseUrl(tp.endpoints[tp.api]!);
    setModels(tp.models);
    addToPool(tp.models);
  };
  const setProto = (k: ApiKind) => {
    setKind(k);
    // A template's other protocols live at their own address.
    if (tpl?.endpoints[k] && baseUrl.trim() === tpl.endpoints[kind]) setBaseUrl(tpl.endpoints[k]!);
  };
  /** Template, direct: an agent that needs another protocol uses the template's address for it. */
  const altFor = (a: AgentId): ApiKind | null => {
    const only = ONLY_API[a];
    return !viaGw && tpl && only && only !== kind && tpl.endpoints[only] ? only : null;
  };
  /** Why an agent can't take this provider (null = it can). */
  const blockedBy = (a: AgentId): string | null => {
    const only = ONLY_API[a];
    if (altFor(a)) return null;
    if (viaGw) return gatewayCapable(a) ? null : t("serviceDialog.blockedGateway", { agent: AGENT_NAME[a] });
    return only && kind !== only ? t("serviceDialog.blockedProto", { agent: AGENT_NAME[a], api: API_LABEL[only] }) : null;
  };

  // Focus the first field once, when the dialog opens (not on every parent re-render).
  useEffect(() => { first.current?.focus(); }, []);

  const url = baseUrl.trim().replace(/\/+$/, "");
  const urlOk = isHttpUrl(url);
  const changedAddr = !!group && url !== (group.baseUrl ?? "").replace(/\/+$/, "");
  const changedKey = key.trim() !== "";
  const changedApi = !!group && kind !== group.api;
  const changed = changedAddr || changedKey || changedApi;
  const canSave = name.trim() !== "" && urlOk && !saving && (!tpl || key.trim() !== "");

  const fetchList = async () => {
    setErr(null);
    setFetching(true);
    try {
      const src = editable[0];
      const list = !key.trim() && src && !changedAddr
        ? await api.fetchModels(src.agent.id, src.p!.id)
        : await api.fetchModelsUrl(url, key.trim() || null, kind);
      addToPool(list);
      if (models.length === 0) setModels(list.slice(0, 20));
    } catch (e) {
      setErr(t("common.fetchFailed", { err: errText(e) }));
    } finally {
      setFetching(false);
    }
  };

  const addManual = (ids: string[]) => {
    addToPool(ids);
    setModels((l) => [...l, ...ids.filter((i) => !l.includes(i))]);
  };

  const save = async () => {
    if (!canSave) return;
    setSaving(true);
    setErr(null);
    try {
      await onSave({
        name: name.trim(), baseUrl: url, api: kind, apiKey: key.trim() || null, models,
        sync: changed ? editable.filter((u) => sync.has(useKey(u))) : [],
        addTo: [...addTo].filter((a) => !blockedBy(a) && !altFor(a)),
        gateway: viaGw,
        alt: (["responses", "chat", "anthropic", "gemini"] as ApiKind[]).map((k) => ({
          api: k, baseUrl: tpl?.endpoints[k] ?? "", addTo: [...addTo].filter((a) => altFor(a) === k),
        })).filter((x) => x.addTo.length),
      });
    } catch (e) {
      setErr(errText(e));
      setSaving(false);
    }
  };

  const foot = (
    <>
      <span className="muted tiny grow">{t("serviceDialog.footNote")}</span>
      <button className="btn" onClick={onClose}>{t("common.cancel")}</button>
      <button className="btn primary" disabled={!canSave} onClick={save}>{saving ? t("common.saving") : isNew ? t("common.add") : t("common.save")}</button>
    </>
  );
  return (
    <Modal label={isNew ? t("common.addProvider") : t("common.editProvider")} wide onClose={onClose}
      title={isNew ? (prefill ? t("serviceDialog.addGroupTo", { station: prefill.station }) : t("common.addProvider")) : t("serviceDialog.editGroup", { name: group!.name })} foot={foot}>
      {isNew && !prefill && <TemplatePicker value={tpl} onPick={pickTpl} />}
      <div className="form2">
        <label className="field">
          <span>{t("common.name")}</span>
          <input ref={first} className="input" value={name} onChange={(e) => setName(e.target.value)} placeholder={t("common.providerNamePlaceholder")} />
        </label>
        <label className="field">
          <span>{t("common.baseUrlLabel")}</span>
          <input className="input mono sensitive" value={baseUrl} onChange={(e) => setBaseUrl(e.target.value)} placeholder="https://api.example.com/v1" />
          {baseUrl && !urlOk && <em className="field-err">{t("common.urlInvalid")}</em>}
        </label>
      </div>
      <div className="form2">
        <div className="field">
          <span>{t("serviceDialog.protocol")}</span>
          <Seg value={kind} onChange={setProto} label={t("serviceDialog.protocol")}
            options={PROTOCOLS.map((v) => {
              const missing = !!tpl && !tpl.endpoints[v];
              return { value: v, label: API_LABEL[v], disabled: missing, title: missing ? t("serviceDialog.protoMissing", { vendor: tpl!.vendor, api: API_LABEL[v] }) : t(API_HINT[v]) };
            })} />
          <em className="muted tiny">
            {tpl
              ? t("serviceDialog.tplProtocols", { list: (Object.keys(tpl.endpoints) as ApiKind[]).map((k) => API_LABEL[k]).join(" / ") })
              : t("serviceDialog.groupHint")}
          </em>
        </div>
        <label className="field">
          <span>{t("common.apiKeyLabel")}</span>
          <input className="input mono" type="password" autoComplete="off" value={key} onChange={(e) => setKey(e.target.value)}
            placeholder={group?.lib?.hasKey || editable.some((u) => u.p!.hasKey) ? t("common.keyKeepPlaceholder") : "sk-..."} />
          <em className="muted tiny">
            {t("serviceDialog.keyStorage")}
            {tpl && <TemplateKeyLink tpl={tpl} />}
          </em>
        </label>
      </div>

      <div className="field">
        <div className="row between">
          <span>{t("serviceDialog.commonModels")} <em className="muted tiny">{t("serviceDialog.commonModelsHint")}</em></span>
          <button type="button" className="btn small" disabled={!urlOk || fetching} onClick={fetchList}>
            <Icon.refresh size={12} />{fetching ? t("common.fetching") : t("common.fetchFromUrl")}
          </button>
        </div>
        <ModelPicker pool={pool} checked={models} onChange={setModels} onAdd={addManual} empty={t("serviceDialog.noModels")} />
      </div>

      {editable.length > 0 && (
        <div className="field">
          <span>{t("serviceDialog.syncTo")} {!changed && <em className="muted tiny">{t("serviceDialog.syncHint")}</em>}</span>
          <div className="agent-picks">
            {editable.map((u) => (
              <label key={useKey(u)} className={`apick${sync.has(useKey(u)) && changed ? " on" : ""}${!changed ? " dim" : ""}`}>
                <input type="checkbox" disabled={!changed} checked={sync.has(useKey(u))}
                  onChange={() => setSync((p) => toggled(p, useKey(u)))} />
                <AgentIcon id={u.agent.id} size={18} />
                <span className="small ellipsis">{u.agent.name} · {u.p!.name}</span>
              </label>
            ))}
          </div>
        </div>
      )}

      {free.length > 0 && (
        <div className="field">
          <span>{t("serviceDialog.addTo")}</span>
          <ToggleRow on={viaGw} onChange={setViaGw} icon={<Icon.gateway size={16} />} title={t("common.useGateway")}
            hint={viaGw ? t("serviceDialog.gatewayOn") : t("serviceDialog.gatewayOff")} />
          <div className="agent-picks">
            {free.map((a) => {
              const why = blockedBy(a.id);
              const only = ONLY_API[a.id];
              return (
                <label key={a.id} className={`apick${addTo.has(a.id) && !why ? " on" : ""}${why ? " dim" : ""}`} title={why ?? undefined}>
                  <input type="checkbox" disabled={!!why} checked={addTo.has(a.id) && !why}
                    onChange={() => setAddTo((p) => toggled(p, a.id))} />
                  <AgentIcon id={a.id} size={18} />
                  <span className="small">{a.name}</span>
                  {why && <span className="tiny muted">{t("serviceDialog.needsApi", { api: API_LABEL[only!] })}</span>}
                  {!why && viaGw && only && only !== kind && <span className="tiny muted">{t("serviceDialog.convertsTo", { api: API_LABEL[only] })}</span>}
                  {altFor(a.id) && <span className="tiny muted">{t("serviceDialog.usesAltUrl", { api: API_LABEL[only!] })}</span>}
                </label>
              );
            })}
          </div>
          {isNew && <em className="muted tiny">{t("serviceDialog.noneRequired")}</em>}
        </div>
      )}

      {err && <ErrorBox text={err} />}
    </Modal>
  );
}
