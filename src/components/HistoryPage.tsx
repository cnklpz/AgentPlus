import { useEffect, useState } from "react";
import { type BackupDetail, type BackupEntry, type BackupFileDetail, api } from "../api";
import { type TKey, t, tn, tx, useLang } from "../i18n";
import { Icon } from "./icons";

/** Product names stay as-is; AgentPlus's own maintenance jobs are translated. */
const AGENT_NAME: Record<string, string | { key: TKey }> = {
  codex: "Codex", zcode: "ZCode", mimo: "MiMo Desktop",
  "codex-cleanup": { key: "historyPage.agentCodexCleanup" }, "codex-repair": { key: "historyPage.agentCodexRepair" },
};

function agentName(id: string): string {
  const n = AGENT_NAME[id];
  return n == null ? id : typeof n === "string" ? n : t(n.key);
}

function fmtStamp(s: string): string {
  const m = s.match(/^(\d{4})(\d{2})(\d{2})-(\d{2})(\d{2})(\d{2})/);
  return m ? `${m[1]}-${m[2]}-${m[3]} ${m[4]}:${m[5]}:${m[6]}` : s;
}

function size(b: number): string {
  return b >= 1_048_576 ? `${(b / 1_048_576).toFixed(1)} MB` : `${Math.max(1, Math.round(b / 1024))} KB`;
}

export function HistoryPage({ flash, onChanged }: { flash: (t: string, e?: boolean) => void; onChanged: () => void }) {
  const [list, setList] = useState<BackupEntry[] | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [sel, setSel] = useState<string | null>(null);
  // Bumped after a rollback so the open detail re-reads the current files.
  const [rev, setRev] = useState(0);
  const load = () => api.listBackups().then(setList).catch((e) => flash(String(e), true));
  // Reasons and "blocked" texts come from the backend in the UI language: reload on switch.
  const lang = useLang();
  useEffect(() => { load(); }, [lang]);

  const restore = async (id: string) => {
    setBusy(true);
    try {
      flash(await api.restoreBackup(id));
      setConfirm(null);
      load();
      setRev((n) => n + 1);
      onChanged();
    } catch (e) {
      flash(String(e), true);
    } finally {
      setBusy(false);
    }
  };

  const picked = list?.find((b) => b.id === sel) ?? null;
  const rollback = (b: BackupEntry, small: boolean) => !b.restorable ? (
    <span className="tiny muted" title={b.blocked ?? undefined}>{t(b.blockedMissing ? "historyPage.fileGone" : "historyPage.noAutoRollback")}</span>
  ) : confirm === b.id ? (
    <>
      <button className={`btn${small ? " small" : ""}`} disabled={busy} onClick={() => setConfirm(null)}>{t("common.cancel")}</button>
      <button className={`btn primary${small ? " small" : ""}`} disabled={busy} onClick={() => restore(b.id)}>{t(busy ? "historyPage.rollingBack" : "historyPage.confirmRollback")}</button>
    </>
  ) : (
    <button className={`btn${small ? " small" : ""}`} onClick={() => setConfirm(b.id)}>{t("historyPage.rollbackBefore")}</button>
  );

  return (
    <>
      <main className="page">
        <div className="page-top">
          <div className="page-head">
            <div className="page-title">
              <h1>{t("historyPage.title")}</h1>
              <span className="muted small">{t("historyPage.subtitle")}</span>
            </div>
            <button className="btn" onClick={() => { load(); setRev((n) => n + 1); }}>{t("common.refresh")}</button>
          </div>
        </div>
        <div className="page-body">
          {!list && <div className="empty">{t("historyPage.reading")}</div>}
          {list && list.length === 0 && <div className="empty">{t("historyPage.empty")}</div>}
          {list && list.length > 0 && (
            <div className="stable">
              {list.map((b) => (
                <div key={b.id} className={`hrow pick${sel === b.id ? " on" : ""}`} role="button" tabIndex={0} aria-pressed={sel === b.id}
                  onClick={() => setSel(sel === b.id ? null : b.id)}
                  onKeyDown={(e) => { if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) { e.preventDefault(); setSel(sel === b.id ? null : b.id); } }}>
                  <div className="minw0">
                    <div className="row gap6">
                      <span className="strong small">{fmtStamp(b.stamp)}</span>
                      <span className="ptag tag-soft">{agentName(b.agent)}</span>
                      <span className="tiny muted">{b.reason}</span>
                    </div>
                    <div className="mono tiny muted ellipsis">{b.files.map((f) => f.name).join(t("historyPage.listSep"))} · {size(b.bytes)}</div>
                  </div>
                  <div className="row gap6" onClick={(e) => e.stopPropagation()}>{rollback(b, true)}</div>
                </div>
              ))}
            </div>
          )}
        </div>
      </main>
      <aside className="aside" aria-label={t("historyPage.detailTitle")}>
        {picked ? (
          <HistoryDetail key={picked.id + rev + lang} b={picked} onClose={() => setSel(null)} actions={rollback(picked, false)} />
        ) : (
          <section className="aside-cur">
            <h2>{t("historyPage.detailTitle")}</h2>
            <span className="muted tiny">{t("historyPage.detailHint")}</span>
            {list && list.length > 0 && (
              <div className="hub-stats">
                <div><b>{list.length}</b><span>{t("historyPage.statRecords")}</span></div>
                <div><b>{list.filter((b) => b.restorable).length}</b><span>{t("historyPage.statRestorable")}</span></div>
                <div><b>{size(list.reduce((n, b) => n + b.bytes, 0))}</b><span>{t("historyPage.statSize")}</span></div>
              </div>
            )}
          </section>
        )}
      </aside>
    </>
  );
}

function HistoryDetail({ b, onClose, actions }: { b: BackupEntry; onClose: () => void; actions: React.ReactNode }) {
  const [d, setD] = useState<BackupDetail | string | null>(null);
  useEffect(() => {
    let alive = true;
    api.backupDetail(b.id).then((x) => alive && setD(x)).catch((e) => alive && setD(String(e)));
    return () => { alive = false; };
  }, [b.id]);
  const changed = typeof d === "object" && d ? d.files.filter((f) => !f.same).length : 0;

  return (
    <>
      <section className="pdetail hdetail-head">
        <div className="row between">
          <div className="minw0">
            <h2>{fmtStamp(b.stamp)}</h2>
            <div className="row gap6">
              <span className="ptag tag-soft">{agentName(b.agent)}</span>
              <span className="tiny muted">{b.reason}</span>
            </div>
          </div>
          <button className="icon-btn" aria-label={t("historyPage.closeDetail")} onClick={onClose}><Icon.close /></button>
        </div>
        <div className="kv">
          <div className="kv-row"><span className="tiny muted">{t("historyPage.backupLocation")}</span><span className="mono tiny ellipsis" title={typeof d === "object" && d ? d.dir : undefined}>{typeof d === "object" && d ? d.dir : "…"}</span></div>
          <div className="kv-row"><span className="tiny muted">{t("historyPage.files")}</span><span className="tiny">{tn("historyPage.filesValue", b.files.length, { size: size(b.bytes) })}</span></div>
          <div className="kv-row">
            <span className="tiny muted">{t("historyPage.vsNow")}</span>
            <span className="tiny">{typeof d === "object" && d ? (changed ? tn("historyPage.changedFiles", changed) : t("historyPage.allSame")) : "…"}</span>
          </div>
          <div className="kv-row"><span className="tiny muted">{t("historyPage.rollback")}</span><span className="tiny">{b.restorable ? t("historyPage.canRollback") : t("historyPage.cannotRollback", { why: b.blocked ?? t("historyPage.unknownReason") })}</span></div>
        </div>
      </section>
      <section className="aside-diff">
        {d === null && <span className="muted small">{t("historyPage.reading")}</span>}
        {typeof d === "string" && <div className="err">{d}</div>}
        {typeof d === "object" && d && (
          <>
            <span className="tiny muted">{tx("historyPage.diffLegend", {
              red: <span className="hd-key del">{t("historyPage.red")}</span>,
              green: <span className="hd-key add">{t("historyPage.green")}</span>,
            })}</span>
            {d.files.map((f) => <FileDiff key={f.name} f={f} />)}
          </>
        )}
      </section>
      <div className="aside-foot">
        <div className="row gap6 hd-actions">{actions}</div>
      </div>
    </>
  );
}

function FileDiff({ f }: { f: BackupFileDetail }) {
  const status = f.same ? t("historyPage.sameAsNow")
    : f.currentBytes == null ? t("historyPage.fileGone")
    : f.binary ? t("historyPage.binary")
    : `+${f.added} −${f.removed}`;
  return (
    <div className="dgroup">
      <div className="dfile hd-file">
        <div className="row between gap6">
          <span className="mono strong ellipsis">{f.name}</span>
          <span className={`tiny ${f.same ? "muted" : f.currentBytes == null ? "warn-text" : ""}`}>{status}</span>
        </div>
        {f.path && <span className="mono tiny muted ellipsis" title={f.path}>{f.path}</span>}
        <span className="tiny muted">
          {t("historyPage.backupSize", { size: size(f.backupBytes) })}
          {f.currentBytes != null && t("historyPage.nowSize", { size: size(f.currentBytes) })}
          {f.currentModified && t("historyPage.modifiedAt", { time: f.currentModified })}
        </span>
      </div>
      {f.diff.length > 0 && (
        <div className="hd-lines">
          {f.diff.map((r, i) => r.kind === "…" ? (
            <div key={i} className="hd-fold tiny muted">⋯ {r.text}</div>
          ) : (
            <div key={i} className={`hd-line mono${r.kind === "+" ? " add" : r.kind === "-" ? " del" : ""}`}>
              <span className="hd-no">{r.kind === "-" ? r.old : r.new}</span>
              <span className="hd-sign">{r.kind === " " ? "" : r.kind === "-" ? "−" : "+"}</span>
              <span className="hd-text">{r.text || " "}</span>
            </div>
          ))}
          {f.truncated && <div className="hd-fold tiny muted">{t("historyPage.truncated")}</div>}
        </div>
      )}
    </div>
  );
}
