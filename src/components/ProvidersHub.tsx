import { useMemo, useState } from "react";
import type { AgentState } from "../api";
import { API_LABEL, type Group, type Station, USE_LABEL, writableAgents } from "../services";
import { AgentIcon, Icon } from "./icons";
import { Avatar, Bars, type Latency, latencyText, latencyTone, stationColor } from "./ProviderCard";
import { type TKey, t, tn } from "../i18n";
import { scrubHost } from "../privacy";
import { onActivateKey } from "../util";

interface Props {
  agents: AgentState[];
  stations: Station[];
  latency: Record<string, Latency>;
  selected: string | null;
  onSelect: (key: string | null) => void;
  onAdd: () => void;
  onTestAll: () => void;
  onTestOne: (url: string) => void;
  envLabel: string;
}

type Filter = "all" | "used" | "idle";

const FILTERS: [Filter, TKey][] = [["all", "providersHub.filterAll"], ["used", "providersHub.filterUsed"], ["idle", "providersHub.filterIdle"]];

const liveUses = (g: Group) => g.uses.filter((u) => u.state !== "removing");
const used = (s: Station) => s.groups.some((g) => liveUses(g).length > 0);

/** Every provider in one place, by station (host) and its groups. Address and key live here; model lists live in each agent. */
export function ProvidersHub({ agents, stations, latency, selected, onSelect, onAdd, onTestAll, onTestOne, envLabel }: Props) {
  const [q, setQ] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const shown = writableAgents(agents);

  const api = stations.filter((s) => !s.builtin);
  const accounts = stations.filter((s) => s.builtin);

  const list = useMemo(() => {
    const k = q.trim().toLowerCase();
    return api.filter((s) => {
      if (filter === "used" && !used(s)) return false;
      if (filter === "idle" && used(s)) return false;
      return !k || s.name.toLowerCase().includes(k) || s.host.includes(k)
        || s.groups.some((g) => g.name.toLowerCase().includes(k) || g.baseUrl.toLowerCase().includes(k) || g.uses.some((u) => u.p?.name.toLowerCase().includes(k)));
    });
  }, [api, q, filter]);

  const counts = { all: api.length, used: api.filter(used).length, idle: api.filter((s) => !used(s)).length };

  return (
    <main className="page">
      <div className="page-top">
        <div className="page-head">
          <div className="page-title">
            <h1>{t("common.providers")}</h1>
            <span className="muted small">
              {t("providersHub.intro", { env: envLabel })}
            </span>
          </div>
          <button className="btn" onClick={onTestAll}><Icon.pulse />{t("providersHub.testLatency")}</button>
          <button className="btn primary" onClick={onAdd}><Icon.plus />{t("common.addProvider")}</button>
        </div>
        <div className="toolbar">
          <div className="seg" role="tablist" aria-label={t("common.filter")}>
            {FILTERS.map(([id, label]) => (
              <button key={id} role="tab" aria-selected={filter === id} className={filter === id ? "on" : ""} onClick={() => setFilter(id)}>
                {t(label)}<b>{counts[id]}</b>
              </button>
            ))}
          </div>
          <label className="search-box">
            <Icon.search size={13} />
            <input value={q} onChange={(e) => setQ(e.target.value)} placeholder={t("providersHub.searchPlaceholder")} />
          </label>
        </div>
      </div>

      <div className="page-body">
        <div className="hgrid">
          {list.map((s) => {
            const lat = s.baseUrl ? latencyText(latency[s.baseUrl]) : { text: "", level: 0 };
            const extra = s.groups.length - 4;
            return (
              <section
                key={s.key}
                className={`hcard${selected === s.key ? " selected" : ""}`}
                data-url={s.baseUrl ?? undefined}
                data-ctx="station"
                data-station={s.key}
                tabIndex={0}
                role="button"
                aria-pressed={selected === s.key}
                onClick={() => onSelect(selected === s.key ? null : s.key)}
                onKeyDown={onActivateKey(() => onSelect(selected === s.key ? null : s.key))}
              >
                <div className="hcard-head">
                  <Avatar name={s.name} color={stationColor(s)} />
                  <span className="pcard-title">
                    <span className="pcard-name"><span className="ellipsis">{scrubHost(s.name)}</span></span>
                    <span className="pcard-host mono ellipsis">{scrubHost(s.host)}</span>
                  </span>
                  {s.baseUrl && (
                    <button className={`hlat lat-btn${lat.level ? ` ${latencyTone(lat.level)}` : ""}`} title={t("common.retestHint")}
                      disabled={latency[s.baseUrl] === "pending"}
                      onClick={(e) => { e.stopPropagation(); onTestOne(s.baseUrl!); }}>
                      <Bars level={lat.level} />{lat.text}<span className="lat-re" aria-hidden="true">↻</span>
                    </button>
                  )}
                </div>
                <div className="hgroups">
                  {s.groups.slice(0, 4).map((g) => {
                    const uses = liveUses(g);
                    return (
                      <div key={g.key} className={`hgroup${uses.length ? "" : " idle"}`}>
                        <span className={`api-chip api-${g.api}`}>{API_LABEL[g.api]}</span>
                        <span className="grow minw0 ellipsis small">{g.name}</span>
                        <span className="hgroup-agents">
                          {uses.length === 0 && <span className="tiny faint">{t("providersHub.notAdded")}</span>}
                          {[...new Map(uses.map((u) => [u.agent.id, u])).values()].map((u) => (
                            <span key={u.agent.id} className={`hgroup-agent ${u.state}`} title={t("providersHub.agentState", { agent: u.agent.name, state: USE_LABEL[u.state] })}>
                              <AgentIcon id={u.agent.id} size={16} />
                            </span>
                          ))}
                        </span>
                      </div>
                    );
                  })}
                  {extra > 0 && <div className="hgroup more tiny muted">{tn("providersHub.moreGroups", extra)}</div>}
                </div>
                <div className="hcard-foot">
                  <span className="tiny muted">{tn("providersHub.groupCount", s.groups.length)}</span>
                  <span className="grow" />
                  {s.groups.some((g) => g.lib)
                    ? <span className="hmeta" title={t("providersHub.inLibraryTitle")}><Icon.key size={12} />{t("providersHub.inLibrary", { n: s.groups.filter((g) => g.lib).length })}</span>
                    : <span className="hmeta faint" title={t("providersHub.notInLibraryTitle")}>{t("providersHub.notInLibrary")}</span>}
                </div>
              </section>
            );
          })}
          {filter !== "used" && !q && (
            <button className="hcard-add" onClick={onAdd}>
              <Icon.plus size={16} />
              <strong>{t("common.addProvider")}</strong>
              <span className="tiny muted">{t("providersHub.addCardHint", { agents: shown.map((a) => a.name).join(" / ") || t("providersHub.eachAgent") })}</span>
            </button>
          )}
        </div>
        {list.length === 0 && (q || filter !== "all") && <div className="empty">{t("providersHub.noMatch")}</div>}

        {accounts.length > 0 && filter === "all" && !q && (
          <>
            <div className="section-label">{t("providersHub.accounts")}</div>
            <div className="acct-row">
              {accounts.map((s) => {
                const u = s.groups[0].uses[0];
                return (
                  <button key={s.key} className={`acct${selected === s.key ? " selected" : ""}`} onClick={() => onSelect(selected === s.key ? null : s.key)}>
                    <AgentIcon id={u.agent.id} size={22} />
                    <span className="grow minw0">
                      <span className="block small strong ellipsis">{s.name}</span>
                      <span className="block tiny muted ellipsis">{u.agent.name} · {USE_LABEL[u.state]}</span>
                    </span>
                  </button>
                );
              })}
            </div>
          </>
        )}
      </div>
    </main>
  );
}
