import { useEffect, useState } from "react";
import { type CleanupPreview, type HealthItem, api } from "../api";
import { locale, t, tn, useLang } from "../i18n";
import { fmtSize } from "../format";
import { scrub } from "../privacy";
import { errText, type Flash } from "../util";

const DAYS = [3, 7, 14];

export function MaintenanceTab({ flash }: { flash: Flash }) {
  const [health, setHealth] = useState<HealthItem[] | null>(null);
  const [healthErr, setHealthErr] = useState<string | null>(null);
  const [days, setDays] = useState(3);
  const [pre, setPre] = useState<CleanupPreview | null>(null);
  const [preErr, setPreErr] = useState<string | null>(null);
  const [tmp, setTmp] = useState(true);
  const [logs, setLogs] = useState(true);
  const [wal, setWal] = useState(true);
  const [busy, setBusy] = useState(false);

  const check = () => {
    setHealth(null);
    setHealthErr(null);
    api.codexHealth().then(setHealth).catch((e) => setHealthErr(errText(e)));
  };
  // Health titles/details come from the backend in the UI language: re-check on switch.
  const lang = useLang();
  useEffect(check, [lang]);
  // Only the latest request counts: switching day counts quickly must not show an older answer.
  useEffect(() => {
    let alive = true;
    setPreErr(null);
    api.codexCleanupPreview(days)
      .then((p) => { if (alive) setPre(p); })
      .catch((e) => { if (alive) { setPre(null); setPreErr(errText(e)); } });
    return () => { alive = false; };
  }, [days, busy]);
  /** Shown where a number would be while the preview loads, or when it failed. */
  const waiting = preErr ? "—" : "…";

  // Rough estimate: deleted rows' share of the file, plus pages already free.
  const logsSave = pre ? Math.round((pre.logsBytes - pre.logsFreeBytes) * (pre.logsOldRows / Math.max(1, pre.logsRows))) + pre.logsFreeBytes : 0;
  const total = pre ? (tmp ? pre.tmpBytes : 0) + (logs ? logsSave : 0) + (wal ? pre.walBytes : 0) : 0;

  const clean = async () => {
    setBusy(true);
    try {
      flash(await api.codexCleanup(tmp, logs ? days : null, wal));
      check();
    } catch (e) {
      flash(errText(e), true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="settings">
      <section className="sgroup">
        <h2 className="row between">{t("maintenanceTab.healthTitle")}<button className="link tiny" onClick={check}>{t("maintenanceTab.recheck")}</button></h2>
        {healthErr && <div className="srow"><span className="err grow">{scrub(healthErr)}</span></div>}
        {!health && !healthErr && <div className="srow muted small">{t("maintenanceTab.checking")}</div>}
        {health?.map((h) => (
          <div key={h.key} className="srow">
            <span className={`hdot ${h.status}`} />
            <div className="grow minw0">
              <div className="slabel">{h.title}</div>
              <div className="muted small">{scrub(h.detail)}</div>
            </div>
          </div>
        ))}
      </section>

      <section className="sgroup">
        <h2>{t("maintenanceTab.cleanupTitle")}</h2>
        {preErr && <div className="srow"><span className="err grow">{scrub(preErr)}</span></div>}
        <label className="srow check-row">
          <input type="checkbox" checked={tmp} onChange={(e) => setTmp(e.target.checked)} />
          <div className="grow">
            <div className="slabel">{t("maintenanceTab.tmpLabel")}</div>
            <div className="muted small">{pre ? tn("maintenanceTab.tmpHint", pre.tmpCount, { size: fmtSize(pre.tmpBytes) }) : waiting}</div>
          </div>
        </label>
        <label className="srow check-row">
          <input type="checkbox" checked={logs} onChange={(e) => setLogs(e.target.checked)} />
          <div className="grow">
            <div className="slabel">{t("maintenanceTab.logsLabel")}</div>
            <div className="muted small">
              {pre ? t("maintenanceTab.logsHint", { size: fmtSize(pre.logsBytes), old: pre.logsOldRows.toLocaleString(locale()), total: pre.logsRows.toLocaleString(locale()), save: fmtSize(logsSave) }) : waiting}
              {t("maintenanceTab.logsNote")}
            </div>
          </div>
          <div className="chips">
            {DAYS.map((d) => (
              <button key={d} type="button" className={`chip${days === d ? " on" : ""}`} onClick={(e) => { e.preventDefault(); setDays(d); }}>{tn("maintenanceTab.keepDays", d)}</button>
            ))}
          </div>
        </label>
        <label className="srow check-row">
          <input type="checkbox" checked={wal} onChange={(e) => setWal(e.target.checked)} />
          <div className="grow">
            <div className="slabel">{t("maintenanceTab.walLabel")}</div>
            <div className="muted small">{pre ? t("maintenanceTab.walHint", { size: fmtSize(pre.walBytes) }) : waiting}</div>
          </div>
        </label>
        <div className="srow">
          <div className="grow muted small">
            {pre?.codexRunning ? t("maintenanceTab.codexRunning") : t("maintenanceTab.estimate", { size: fmtSize(total) })}
          </div>
          <button className="btn primary" disabled={busy || !pre || pre.codexRunning || (!tmp && !logs && !wal)} onClick={clean}>
            {t(busy ? "maintenanceTab.cleaning" : "maintenanceTab.clean")}
          </button>
        </div>
      </section>
    </div>
  );
}
