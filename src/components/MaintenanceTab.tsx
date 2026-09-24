import { useEffect, useState } from "react";
import { type CleanupPreview, type HealthItem, api } from "../api";
import { locale, t, tn, useLang } from "../i18n";

const DAYS = [3, 7, 14];

function mb(b: number): string {
  return `${(b / 1_048_576).toFixed(1)} MB`;
}

export function MaintenanceTab({ flash }: { flash: (text: string, error?: boolean) => void }) {
  const [health, setHealth] = useState<HealthItem[] | null>(null);
  const [healthErr, setHealthErr] = useState<string | null>(null);
  const [days, setDays] = useState(3);
  const [pre, setPre] = useState<CleanupPreview | null>(null);
  const [tmp, setTmp] = useState(true);
  const [logs, setLogs] = useState(true);
  const [wal, setWal] = useState(true);
  const [busy, setBusy] = useState(false);

  const check = () => {
    setHealth(null);
    setHealthErr(null);
    api.codexHealth().then(setHealth).catch((e) => setHealthErr(String(e)));
  };
  // Health titles/details come from the backend in the UI language: re-check on switch.
  const lang = useLang();
  useEffect(check, [lang]);
  useEffect(() => { api.codexCleanupPreview(days).then(setPre).catch(() => setPre(null)); }, [days, busy]);

  // Rough estimate: deleted rows' share of the file, plus pages already free.
  const logsSave = pre ? Math.round((pre.logsBytes - pre.logsFreeBytes) * (pre.logsOldRows / Math.max(1, pre.logsRows))) + pre.logsFreeBytes : 0;
  const total = pre ? (tmp ? pre.tmpBytes : 0) + (logs ? logsSave : 0) + (wal ? pre.walBytes : 0) : 0;

  const clean = async () => {
    setBusy(true);
    try {
      flash(await api.codexCleanup(tmp, logs ? days : null, wal));
      check();
    } catch (e) {
      flash(String(e), true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="settings">
      <section className="sgroup">
        <h2 className="row between">{t("maintenanceTab.healthTitle")}<button className="link tiny" onClick={check}>{t("maintenanceTab.recheck")}</button></h2>
        {healthErr && <div className="srow"><span className="err grow">{healthErr}</span></div>}
        {!health && !healthErr && <div className="srow muted small">{t("maintenanceTab.checking")}</div>}
        {health?.map((h) => (
          <div key={h.key} className="srow">
            <span className={`hdot ${h.status}`} />
            <div className="grow minw0">
              <div className="slabel">{h.title}</div>
              <div className="muted small">{h.detail}</div>
            </div>
          </div>
        ))}
      </section>

      <section className="sgroup">
        <h2>{t("maintenanceTab.cleanupTitle")}</h2>
        <label className="srow check-row">
          <input type="checkbox" checked={tmp} onChange={(e) => setTmp(e.target.checked)} />
          <div className="grow">
            <div className="slabel">{t("maintenanceTab.tmpLabel")}</div>
            <div className="muted small">{pre ? tn("maintenanceTab.tmpHint", pre.tmpCount, { size: mb(pre.tmpBytes) }) : "…"}</div>
          </div>
        </label>
        <label className="srow check-row">
          <input type="checkbox" checked={logs} onChange={(e) => setLogs(e.target.checked)} />
          <div className="grow">
            <div className="slabel">{t("maintenanceTab.logsLabel")}</div>
            <div className="muted small">
              {pre ? t("maintenanceTab.logsHint", { size: mb(pre.logsBytes), old: pre.logsOldRows.toLocaleString(locale()), total: pre.logsRows.toLocaleString(locale()), save: mb(logsSave) }) : "…"}
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
            <div className="muted small">{pre ? t("maintenanceTab.walHint", { size: mb(pre.walBytes) }) : "…"}</div>
          </div>
        </label>
        <div className="srow">
          <div className="grow muted small">
            {pre?.codexRunning ? t("maintenanceTab.codexRunning") : t("maintenanceTab.estimate", { size: mb(total) })}
          </div>
          <button className="btn primary" disabled={busy || !pre || pre.codexRunning || (!tmp && !logs && !wal)} onClick={clean}>
            {t(busy ? "maintenanceTab.cleaning" : "maintenanceTab.clean")}
          </button>
        </div>
      </section>
    </div>
  );
}
