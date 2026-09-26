import { useEffect, useMemo, useState } from "react";
import { type SessionRow, api } from "../api";
import { type TKey, t, tn, useLang } from "../i18n";
import { fmtAgo, fmtSize } from "../format";
import { Dropdown } from "./Dropdown";
import { Seg } from "./controls";
import { Icon } from "./icons";
import { useLoad } from "../hooks";
import { scrub } from "../privacy";
import { copyText, errText, type Flash, toggled } from "../util";
import { isMac } from "../platform";

interface Props {
  /** Default target: the fixed id when on, else the configured provider. */
  target: string;
  flash: Flash;
  /** Prefilled search (e.g. a session id picked in Ctrl+K). */
  initialQuery?: string;
}

const KIND_LABEL: Record<SessionRow["kind"], TKey> = {
  user: "sessionsTab.kindUser", automation: "sessionsTab.kindAutomation", subagent: "sessionsTab.kindSubagent",
  review: "sessionsTab.kindReview", exec: "sessionsTab.kindExec", agent: "sessionsTab.kindAgent",
};

export function SessionsTab({ target, flash, initialQuery }: Props) {
  const [provider, setProvider] = useState<string>("all");
  const [allKinds, setAllKinds] = useState(!!initialQuery);
  const [showArchived, setShowArchived] = useState(!!initialQuery);
  const [q, setQ] = useState(initialQuery ?? "");
  const [busy, setBusy] = useState(false);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [sources, setSources] = useState<Set<string> | null>(null);
  const [repairTarget, setRepairTarget] = useState(target);
  const [moveTarget, setMoveTarget] = useState(target);
  const [confirm, setConfirm] = useState<string | null>(null); // "repair" | "move" | session id

  useEffect(() => { if (initialQuery) { setQ(initialQuery); setAllKinds(true); setShowArchived(true); } }, [initialQuery]);

  // Backend notes and hidden-reasons are rendered in the UI language: reload on switch.
  const lang = useLang();
  const { data, error, reload } = useLoad(() => api.codexSessions().then((d) => { setPicked(new Set()); return d; }), [lang]);
  const load = () => { void reload(); };
  useEffect(() => { setRepairTarget(target); setMoveTarget(target); }, [target]);

  const targetOptions = useMemo(() => {
    const ids = data ? [...data.targets] : [];
    if (!ids.includes(target)) ids.push(target);
    return ids.map((v) => ({
      value: v,
      label: v,
      hint: v === target ? t("sessionsTab.currentProvider") : data && !data.targets.includes(v) ? t("sessionsTab.notDefined") : undefined,
    }));
  }, [data, target, lang]);

  // Sessions not on the repair target, grouped by their provider.
  const misplacedBy = useMemo(() => {
    const m = new Map<string, number>();
    for (const s of data?.sessions ?? []) if (s.provider && s.provider !== repairTarget) m.set(s.provider, (m.get(s.provider) ?? 0) + 1);
    return [...m.entries()].sort((a, b) => b[1] - a[1]);
  }, [data, repairTarget]);
  const chosenSources = sources ?? new Set(misplacedBy.map(([p]) => p));
  const repairIds = (data?.sessions ?? []).filter((s) => chosenSources.has(s.provider) && s.provider !== repairTarget).map((s) => s.id);

  const rows = useMemo(() => {
    if (!data) return [];
    const needle = q.trim().toLowerCase();
    return data.sessions.filter((s) =>
      (provider === "all" || s.provider === provider) &&
      (allKinds || s.kind === "user" || s.kind === "automation") &&
      (showArchived || !s.archived) &&
      (!needle || s.title.toLowerCase().includes(needle) || s.cwd.toLowerCase().includes(needle) || s.id.includes(needle)),
    );
  }, [data, provider, allKinds, showArchived, q]);
  const shown = rows.slice(0, 300);
  const allShownPicked = shown.length > 0 && shown.every((s) => picked.has(s.id));

  const run = async (fn: () => Promise<string>) => {
    setBusy(true);
    try {
      flash(await fn());
      setSources(null);
      setConfirm(null);
      load();
    } catch (e) {
      flash(errText(e), true);
    } finally {
      setBusy(false);
    }
  };

  if (error) return <div className="empty">{scrub(error)}</div>;
  if (!data) return <div className="empty">{t("sessionsTab.loading")}</div>;

  const canWrite = data.writable && !data.codexRunning;
  const blockedWhy = data.codexRunning ? t("sessionsTab.quitCodexFirst") : !data.writable ? t("sessionsTab.readOnly") : undefined;
  const definedTarget = (id: string) => data.targets.includes(id);
  const last = data.lastRepair && !data.lastRepair.undone ? data.lastRepair : null;
  const togglePick = (id: string) => setPicked((s) => toggled(s, id));

  return (
    <div className="stack12">
      {misplacedBy.length > 0 && (
        <section className="card repair2">
          <div className="repair2-icon" aria-hidden="true"><Icon.warn /></div>
          <div className="grow minw0 stack8">
            <div>
              <div className="strong">{tn("sessionsTab.misplacedTitle", misplacedBy.reduce((n, [, c]) => n + c, 0))}</div>
              <div className="muted small hint">{t("sessionsTab.misplacedHint")}</div>
            </div>
            <div className="repair2-bar">
              <span className="tiny muted">{t("sessionsTab.source")}</span>
              <div className="chips left">
                {misplacedBy.map(([p, n]) => {
                  const on = chosenSources.has(p);
                  return (
                    <button key={p} className={`chip${on ? " on" : ""}`} aria-pressed={on}
                      onClick={() => setSources(toggled(chosenSources, p))}>
                      {p} <b>{n}</b>
                    </button>
                  );
                })}
              </div>
              <span className="tiny muted">{t("sessionsTab.moveTo")}</span>
              <Dropdown value={repairTarget} options={targetOptions} label={t("sessionsTab.targetProvider")} onChange={(v) => { setRepairTarget(v); setSources(null); setConfirm(null); }} />
              <span className="grow" />
              {confirm === "repair" ? (
                <>
                  <button className="btn" disabled={busy} onClick={() => setConfirm(null)}>{t("common.cancel")}</button>
                  <button className="btn primary" disabled={busy} onClick={() => run(() => api.codexRepair(repairIds, repairTarget))}>{busy ? t("sessionsTab.working") : t("sessionsTab.confirmRepair", { n: repairIds.length })}</button>
                </>
              ) : (
                <button className="btn primary" disabled={!canWrite || busy || repairIds.length === 0} title={blockedWhy} onClick={() => setConfirm("repair")}>
                  {blockedWhy ? blockedWhy : t("sessionsTab.repair", { n: repairIds.length })}
                </button>
              )}
            </div>
            {!definedTarget(repairTarget) && <div className="tiny warn-text">{t("sessionsTab.targetUndefined", { target: repairTarget })}</div>}
          </div>
        </section>
      )}

      {last && (
        <div className="row between note-line">
          <span className="small">{tn("sessionsTab.lastAction", last.count, { target: last.target })}</span>
          <button className="link" disabled={!canWrite || busy} onClick={() => run(() => api.codexUndoRepair(last.stamp))}>{t("common.undo")}</button>
        </div>
      )}
      {data.note && <div className="notes"><span>{scrub(data.note)}</span></div>}

      <div className="toolbar">
        <Seg value={provider} onChange={setProvider} label={t("common.provider")} options={[
          { value: "all", label: <>{t("sessionsTab.all")} <b>{data.sessions.length}</b></> },
          ...data.providers.map(([p, n]) => ({ value: p, label: <>{p || t("sessionsTab.emptyProvider")} <b>{n}</b></> })),
        ]} />
        <label className="toggle"><input type="checkbox" checked={allKinds} onChange={(e) => setAllKinds(e.target.checked)} /><span>{t("sessionsTab.subagentsReviews")}</span></label>
        <label className="toggle"><input type="checkbox" checked={showArchived} onChange={(e) => setShowArchived(e.target.checked)} /><span>{t("sessionsTab.archived")}</span></label>
        <div className="search-box">
          <Icon.search size={14} />
          <input placeholder={t("sessionsTab.searchPlaceholder")} value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
        <button className="icon-btn" title={t("common.refresh")} aria-label={t("common.refresh")} onClick={load}>
          <Icon.refresh size={13} />
        </button>
      </div>

      {picked.size > 0 && (
        <div className="movebar">
          <span className="strong small">{t("sessionsTab.picked", { n: picked.size })}</span>
          <span className="tiny muted">{t("sessionsTab.moveTo")}</span>
          <Dropdown value={moveTarget} options={targetOptions} label={t("sessionsTab.moveTo")} onChange={(v) => { setMoveTarget(v); setConfirm(null); }} />
          <span className="grow" />
          {confirm === "move" ? (
            <>
              <button className="btn" disabled={busy} onClick={() => setConfirm(null)}>{t("common.cancel")}</button>
              <button className="btn primary" disabled={busy} onClick={() => run(() => api.codexRepair([...picked], moveTarget))}>{t(busy ? "sessionsTab.working" : "sessionsTab.confirmMove")}</button>
            </>
          ) : (
            <button className="btn primary" disabled={!canWrite || busy} title={blockedWhy} onClick={() => setConfirm("move")}>{t("sessionsTab.move", { n: picked.size })}</button>
          )}
          <button className="link" onClick={() => { setPicked(new Set()); setConfirm(null); }}>{t("sessionsTab.clearSelection")}</button>
        </div>
      )}

      <div className="stable">
        <div className="srow-h">
          <input type="checkbox" aria-label={t("sessionsTab.selectAll")} checked={allShownPicked}
            onChange={() => setPicked((s) => { const n = new Set(s); shown.forEach((r) => (allShownPicked ? n.delete(r.id) : n.add(r.id))); return n; })} />
          <span>{t("sessionsTab.colSession")}</span><span>{t("common.provider")}</span><span>{t("sessionsTab.colUpdated")}</span><span />
        </div>
        {shown.map((s) => {
          const misplaced = !!s.provider && s.provider !== target;
          return (
            <div key={s.id} className={`sess${picked.has(s.id) ? " picked" : ""}`} data-ctx="session" data-sid={s.id} data-path={s.rolloutPath} data-title={s.title}>
              <input type="checkbox" aria-label={t("sessionsTab.selectRow", { title: s.title })} checked={picked.has(s.id)} onChange={() => togglePick(s.id)} />
              <div className="minw0">
                <div className="row gap6 minw0">
                  <span className="sess-title ellipsis sensitive">{s.title}</span>
                  {s.kind !== "user" && <span className="mtag">{t(KIND_LABEL[s.kind])}</span>}
                  {s.archived && <span className="mtag">{t("sessionsTab.archived")}</span>}
                </div>
                <div className="mono tiny muted ellipsis sensitive">{scrub(s.cwd)}</div>
                {s.hidden.length > 0 && <div className="why2"><Icon.warn />{s.hidden.join(t("sessionsTab.hiddenSep"))}</div>}
              </div>
              <span className={`ptag ${misplaced ? "tag-warn" : "tag-soft"}`}>{s.provider || "—"}</span>
              <span className="meta2">
                <span className="small">{fmtAgo(s.updatedMs)}</span>
                <span className="tiny muted mono">{s.rolloutExists ? fmtSize(s.size) : t("sessionsTab.fileMissing")}</span>
              </span>
              <span className="row gap6 sess-actions">
                {misplaced && s.rolloutExists && (
                  confirm === s.id ? (
                    <>
                      <button className="btn xs primary" disabled={busy} onClick={() => run(() => api.codexRepair([s.id], target))}>{busy ? "…" : t("common.confirm")}</button>
                      <button className="btn xs" disabled={busy} onClick={() => setConfirm(null)}>{t("common.cancel")}</button>
                    </>
                  ) : (
                    <button className="btn xs restore" disabled={!canWrite || busy} title={blockedWhy ?? t("sessionsTab.restoreTitle", { target })} onClick={() => setConfirm(s.id)}>{t("sessionsTab.restore")}</button>
                  )
                )}
                <button className="icon-btn sm" title={t("sessionsTab.copyResumeTitle", { cmd: `codex resume ${s.id}` })} aria-label={t("sessionsTab.copyResume")}
                  onClick={() => copyText(`codex resume ${s.id}`, flash, t("sessionsTab.resumeCopied"))}>
                  <Icon.copy size={12} />
                </button>
                <button className="icon-btn sm" title={t(isMac ? "sessionsTab.revealTitleMac" : "sessionsTab.revealTitle")} aria-label={t("sessionsTab.reveal")} disabled={!s.rolloutExists} onClick={() => { api.revealPath(s.rolloutPath).catch((e) => flash(errText(e), true)); }}>
                  <Icon.folder size={12} />
                </button>
              </span>
            </div>
          );
        })}
        {shown.length === 0 && <div className="empty-row muted small">{t("sessionsTab.noMatch")}</div>}
        <div className="mtable-foot muted small">{t("sessionsTab.footer", { shown: shown.length, total: rows.length })}</div>
      </div>
    </div>
  );
}
