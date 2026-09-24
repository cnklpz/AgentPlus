import { useEffect, useState } from "react";
import { type CleanupPreview, api } from "../api";
import { locale, t, tn, useLang } from "../i18n";
import { fmtSize } from "../format";
import { scrub } from "../privacy";
import { errText, type Flash } from "../util";
import { useLoad } from "../hooks";
import { ErrorBox, SettingRow } from "./controls";

const DAYS = [3, 7, 14];

export function MaintenanceTab({ flash }: { flash: Flash }) {
  const [days, setDays] = useState(3);
  const [pre, setPre] = useState<CleanupPreview | null>(null);
  const [preErr, setPreErr] = useState<string | null>(null);
  const [tmp, setTmp] = useState(true);
  const [logs, setLogs] = useState(true);
  const [wal, setWal] = useState(true);
  const [busy, setBusy] = useState(false);

  // Health titles/details come from the backend in the UI language: re-check on switch.
  const lang = useLang();
  const { data: health, error: healthErr, reload } = useLoad(() => api.codexHealth(), [lang], { clear: true });
  const check = () => { void reload(); };
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
        {healthErr && <div className="srow"><ErrorBox className="grow" text={healthErr} /></div>}
        {!health && !healthErr && <div className="srow muted small">{t("maintenanceTab.checking")}</div>}
        {health?.map((h) => (
          <SettingRow key={h.key} lead={<span className={`hdot ${h.status}`} />} label={h.title} desc={scrub(h.detail)} />
        ))}
      </section>

      <section className="sgroup">
        <h2>{t("maintenanceTab.cleanupTitle")}</h2>
        {preErr && <div className="srow"><ErrorBox className="grow" text={preErr} /></div>}
        <SettingRow as="label" className="check-row" lead={<input type="checkbox" checked={tmp} onChange={(e) => setTmp(e.target.checked)} />}
          label={t("maintenanceTab.tmpLabel")} desc={pre ? tn("maintenanceTab.tmpHint", pre.tmpCount, { size: fmtSize(pre.tmpBytes) }) : waiting} />
        <SettingRow as="label" className="check-row" lead={<input type="checkbox" checked={logs} onChange={(e) => setLogs(e.target.checked)} />}
          label={t("maintenanceTab.logsLabel")}
          desc={pre ? t("maintenanceTab.logsHint", { size: fmtSize(pre.logsBytes), old: pre.logsOldRows.toLocaleString(locale()), total: pre.logsRows.toLocaleString(locale()), save: fmtSize(logsSave) }) : waiting}
          note={<div className="muted small">{t("maintenanceTab.logsNote")}</div>}>
          <div className="chips">
            {DAYS.map((d) => (
              <button key={d} type="button" className={`chip${days === d ? " on" : ""}`} onClick={(e) => { e.preventDefault(); setDays(d); }}>{tn("maintenanceTab.keepDays", d)}</button>
            ))}
          </div>
        </SettingRow>
        <SettingRow as="label" className="check-row" lead={<input type="checkbox" checked={wal} onChange={(e) => setWal(e.target.checked)} />}
          label={t("maintenanceTab.walLabel")} desc={pre ? t("maintenanceTab.walHint", { size: fmtSize(pre.walBytes) }) : waiting} />
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
