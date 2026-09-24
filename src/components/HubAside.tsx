import { type ReactNode, useEffect, useState } from "react";
import { type AgentId, type AgentState, type DiffGroup, api } from "../api";
import { type Draft, opsToWrite } from "../draft";
import type { Station } from "../services";
import { AgentIcon, Icon } from "./icons";
import { t, tn, useLang } from "../i18n";
import { scrub } from "../privacy";

interface Props {
  agents: AgentState[];
  drafts: Record<string, Draft>;
  stations: Station[];
  detail: ReactNode | null;
  busy: boolean;
  onDiscard: (agent: AgentId | null) => void;
  onApplyAll: () => void;
}

/** Hub page right column: service details on top, every agent's pending changes below. */
export function HubAside({ agents, drafts, stations, detail, busy, onDiscard, onApplyAll }: Props) {
  const withOps = agents.filter((a) => Object.keys(drafts[a.id] ?? {}).length > 0);
  const total = withOps.reduce((n, a) => n + Object.keys(drafts[a.id]).length, 0);
  const [diffs, setDiffs] = useState<Record<string, DiffGroup[] | string>>({});
  // Diffs and errors are rendered by the backend in the current language.
  const lang = useLang();

  useEffect(() => {
    let alive = true;
    for (const a of withOps) {
      api.preview(a.id, opsToWrite(a, drafts[a.id]))
        .then((d) => alive && setDiffs((m) => ({ ...m, [a.id]: d })))
        .catch((e) => alive && setDiffs((m) => ({ ...m, [a.id]: String(e) })));
    }
    return () => { alive = false; };
  }, [drafts, agents, lang]);

  const api_ = stations.filter((s) => !s.builtin);
  const groups = api_.reduce((n, s) => n + s.groups.length, 0);
  const usable = agents.filter((a) => a.installed && !a.readonly).length;

  return (
    <aside className="aside" aria-label={t("hubAside.aria")}>
      {detail ?? (
        <section className="aside-cur">
          <div className="row between">
            <h2>{t("hubAside.library")}</h2>
            <span className="muted tiny">{t("hubAside.savedIn")}</span>
          </div>
          <div className="hub-stats">
            <div><b>{api_.length}</b><span>{tn("hubAside.relays", api_.length)}</span></div>
            <div><b>{groups}</b><span>{tn("hubAside.groups", groups)}</span></div>
            <div><b>{usable}</b><span>{tn("hubAside.agentsAvailable", usable)}</span></div>
          </div>
          <span className="muted tiny">{t("hubAside.hint")}</span>
        </section>
      )}

      <section className="aside-diff">
        <div className="row between">
          <h2>{t("hubAside.pending")}</h2>
          <span className={`count${total ? " warn" : ""}`}>{total ? `${tn("hubAside.agentCount", withOps.length)} · ${tn("hubAside.itemCount", total)}` : t("common.none")}</span>
        </div>
        {withOps.map((a) => {
          const d = diffs[a.id];
          return (
            <div key={a.id} className="hub-agent">
              <div className="row gap6">
                <AgentIcon id={a.id} size={18} />
                <strong className="small grow">{a.name}</strong>
                <span className="tiny muted">{tn("hubAside.itemCount", Object.keys(drafts[a.id]).length)}</span>
                <button className="link" onClick={() => onDiscard(a.id)}>{t("hubAside.discard")}</button>
              </div>
              {typeof d === "string" && <div className="err">{scrub(d)}</div>}
              {Array.isArray(d) && d.map((g) => (
                <div key={g.file} className="dgroup">
                  <div className="dfile mono ellipsis">{scrub(g.file)}</div>
                  {g.lines.map((l, i) => <div key={i} className={`dline mono ${l.add ? "add" : "del"}`}>{scrub(l.text)}</div>)}
                </div>
              ))}
            </div>
          );
        })}
        {total === 0 && (
          <div className="dempty">
            <Icon.check size={20} color="#16A34A" />
            <strong>{t("hubAside.upToDate")}</strong>
            <span className="muted small">{t("hubAside.emptyHint")}</span>
          </div>
        )}
      </section>

      <div className="aside-foot">
        <div className="grid2">
          <button className="btn full" disabled={!total || busy} onClick={() => onDiscard(null)}>{t("hubAside.discardAll")}</button>
          <button className="btn primary full" disabled={!total || busy} onClick={onApplyAll}>{busy ? t("hubAside.writing") : withOps.length > 1 ? tn("hubAside.applyTo", withOps.length) : t("common.apply")}</button>
        </div>
        <span className="muted tiny center">{t("hubAside.backupNote")}</span>
      </div>
    </aside>
  );
}
