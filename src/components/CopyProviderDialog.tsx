import { useMemo, useState } from "react";
import type { AgentId, AgentState, ApiKind, LibEntry, Provider } from "../api";
import { useEscape } from "../hooks";
import { t, tn, useLang } from "../i18n";
import { API_LABEL } from "../services";
import { AgentIcon, Icon } from "./icons";
import { colorFor, initials } from "./ProviderCard";
import { scrubHost } from "../privacy";
import { toggled } from "../util";

/** One provider that can be copied into a project. */
interface Source {
  key: string;
  from: string;
  fromName: string;
  provider: string;
  name: string;
  host: string;
  api: ApiKind;
  models: number;
  hasKey: boolean;
  /** Already reaches the project through the global OpenCode config. */
  inherited: boolean;
}

export interface CopyPick {
  fromAgent: string;
  provider: string;
  name: string;
  api: ApiKind;
  label: string;
  /** Turn the inherited global original off in this project. */
  disableInherited: boolean;
}

interface Props {
  target: AgentState;
  /** Detected agents (their providers are copy sources). */
  agents: AgentState[];
  lib: LibEntry[];
  onCopy: (picks: CopyPick[]) => void;
  onClose: () => void;
}

const COPYABLE: ApiKind[] = ["chat", "responses", "anthropic"];

/** Copies providers (address, key and visible models) from global OpenCode, other agents or the library into a project. */
export function CopyProviderDialog({ target, agents, lib, onCopy, onClose }: Props) {
  useEscape(onClose);
  const [q, setQ] = useState("");
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [disable, setDisable] = useState(true);
  const lang = useLang();

  const inheritedIds = new Set(target.providers.filter((p) => !p.editable && !p.builtin).map((p) => p.id));
  const groups = useMemo(() => {
    const out: { id: string; title: string; agent?: AgentId; items: Source[] }[] = [];
    const ordered = [...agents.filter((a) => a.id === "opencode"), ...agents.filter((a) => a.id !== "opencode")];
    for (const a of ordered) {
      const items = a.providers
        .filter((p) => !p.builtin && p.baseUrl && COPYABLE.includes(p.api))
        .map((p) => ({
          key: `${a.id}|${p.id}`, from: a.id, fromName: a.id === "opencode" ? t("copyProviderDialog.globalOpenCode") : a.name, provider: p.id, name: p.name,
          host: p.host, api: p.api, models: p.models.filter((m) => m.visible).length, hasKey: p.hasKey,
          inherited: a.id === "opencode" && inheritedIds.has(p.id),
        }));
      if (items.length) out.push({ id: a.id, title: a.id === "opencode" ? t("copyProviderDialog.globalOpenCode") : a.name, agent: a.id, items });
    }
    const libItems = lib.filter((e) => COPYABLE.includes(e.api)).map((e) => ({
      key: `library|${e.id}`, from: "library", fromName: t("copyProviderDialog.library"), provider: e.id, name: e.name, host: e.baseUrl.replace(/^https?:\/\//, ""),
      api: e.api, models: e.models.length, hasKey: e.hasKey, inherited: false,
    }));
    if (libItems.length) out.push({ id: "library", title: t("copyProviderDialog.library"), items: libItems });
    return out;
  }, [agents, lib, target, lang]);

  const needle = q.trim().toLowerCase();
  const match = (s: Source) => !needle || `${s.name} ${s.host} ${s.provider}`.toLowerCase().includes(needle);
  const all = groups.flatMap((g) => g.items);
  const chosen = all.filter((s) => picked.has(s.key));
  const anyInherited = chosen.some((s) => s.inherited);
  const toggle = (k: string) => setPicked((cur) => toggled(cur, k));

  const submit = () => onCopy(chosen.map((s) => ({
    fromAgent: s.from, provider: s.provider, name: s.name, api: s.api, label: s.fromName, disableInherited: disable && s.inherited,
  })));

  return (
    <div className="modal-bg" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="modal wide" role="dialog" aria-modal="true" aria-label={t("copyProviderDialog.ariaTitle")}>
        <div className="modal-head">
          <h2>{t("copyProviderDialog.title", { name: target.name })}</h2>
          <button className="icon-btn" aria-label={t("common.close")} onClick={onClose}><Icon.close /></button>
        </div>
        <div className="modal-body">
          <span className="muted small">{t("copyProviderDialog.intro")}</span>
          <input className="search-input" autoFocus placeholder={t("copyProviderDialog.filterPlaceholder")} value={q} onChange={(e) => setQ(e.target.value)} />
          {all.length === 0 && <div className="empty">{t("copyProviderDialog.empty")}</div>}
          {groups.map((g) => {
            const items = g.items.filter(match);
            if (!items.length) return null;
            return (
              <div key={g.id} className="copy-group">
                <div className="copy-group-title">
                  {g.agent ? <AgentIcon id={g.agent} size={16} /> : <Icon.layers size={14} />}
                  <span>{g.title}</span>
                  <span className="tiny muted">{items.length}</span>
                </div>
                {items.map((s) => (
                  <label key={s.key} className={`copy-row${picked.has(s.key) ? " on" : ""}`}>
                    <input type="checkbox" checked={picked.has(s.key)} onChange={() => toggle(s.key)} />
                    <span className="pavatar sm" style={{ background: colorFor({ builtin: false, baseUrl: s.host, host: s.host, id: s.provider } as Provider) }}>{initials(s.name)}</span>
                    <span className="grow minw0">
                      <span className="row gap6 minw0">
                        <span className="strong ellipsis">{s.name}</span>
                        {s.inherited && <span className="ptag tag-soft">{t("copyProviderDialog.inherited")}</span>}
                      </span>
                      <span className="block mono tiny muted ellipsis">{scrubHost(s.host)}</span>
                    </span>
                    <span className="api-chip">{API_LABEL[s.api]}</span>
                    <span className="tiny muted copy-meta">{tn("copyProviderDialog.modelCount", s.models)}{s.hasKey ? "" : ` · ${t("copyProviderDialog.noKey")}`}</span>
                  </label>
                ))}
              </div>
            );
          })}
        </div>
        <div className="modal-foot">
          {anyInherited ? (
            <label className="check-row small grow">
              <input type="checkbox" checked={disable} onChange={(e) => setDisable(e.target.checked)} />
              <span>{t("copyProviderDialog.disableInherited")}</span>
            </label>
          ) : <span className="grow" />}
          <button className="btn" onClick={onClose}>{t("common.cancel")}</button>
          <button className="btn primary" disabled={!chosen.length} onClick={submit}>{tn("copyProviderDialog.copyN", chosen.length)}</button>
        </div>
      </div>
    </div>
  );
}
