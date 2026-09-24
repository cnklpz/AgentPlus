import type { ReactNode } from "react";
import type { AgentId, AgentState } from "../api";
import type { Draft } from "../draft";
import { usePreviews } from "../hooks";
import { DiffGroups, UpToDate } from "./Aside";
import { ErrorBox } from "./controls";
import type { Station } from "../services";
import { AgentIcon } from "./icons";
import { t, tn } from "../i18n";

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
  const diffs = usePreviews(agents, drafts);

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
          <h2>{t("common.pendingChanges")}</h2>
          <span className={`count${total ? " warn" : ""}`}>{total ? `${tn("hubAside.agentCount", withOps.length)} · ${tn("common.changeCount", total)}` : t("common.none")}</span>
        </div>
        {withOps.map((a) => {
          const d = diffs[a.id];
          return (
            <div key={a.id} className="hub-agent">
              <div className="row gap6">
                <AgentIcon id={a.id} size={18} />
                <strong className="small grow">{a.name}</strong>
                <span className="tiny muted">{tn("common.changeCount", Object.keys(drafts[a.id]).length)}</span>
                <button className="link" onClick={() => onDiscard(a.id)}>{t("common.discard")}</button>
              </div>
              {typeof d === "string" && <ErrorBox text={d} />}
              {Array.isArray(d) && <DiffGroups groups={d} />}
            </div>
          );
        })}
        {total === 0 && <UpToDate hint={t("hubAside.emptyHint")} />}
      </section>

      <div className="aside-foot">
        <div className="grid2">
          <button className="btn full" disabled={!total || busy} onClick={() => onDiscard(null)}>{t("common.discardAll")}</button>
          <button className="btn primary full" disabled={!total || busy} onClick={onApplyAll}>{busy ? t("common.writing") : withOps.length > 1 ? tn("hubAside.applyTo", withOps.length) : t("common.apply")}</button>
        </div>
        <span className="muted tiny center">{t("hubAside.backupNote")}</span>
      </div>
    </aside>
  );
}
