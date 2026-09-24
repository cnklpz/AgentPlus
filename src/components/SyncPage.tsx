import { useEffect, useState } from "react";
import { type SyncStatus, type SyncSuggestion, api } from "../api";
import { AgentIcon } from "./icons";
import { syncSuggestionIds } from "../services";
import { locale, t, useLang } from "../i18n";
import { scrub } from "../privacy";
import { errText, type Flash, toggled } from "../util";

interface Props {
  flash: Flash;
  /** Adds the chosen suggestions to the drafts of their agents. */
  onAdopt: (s: SyncSuggestion[]) => void;
}

export function SyncPage({ flash, onAdopt }: Props) {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [folder, setFolder] = useState("");
  const [sugs, setSugs] = useState<SyncSuggestion[] | null>(null);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const lang = useLang();

  const load = () => api.syncStatus().then((s) => { setStatus(s); setFolder(s.folder ?? ""); }).catch((e) => flash(errText(e), true));
  useEffect(() => { load(); }, []);
  // The suggestions' titles are rendered by the backend: re-fetch them in the new language.
  useEffect(() => { if (sugs) api.syncPreview().then(setSugs).catch(() => undefined); }, [lang]);

  const wrap = async (fn: () => Promise<void>) => {
    setBusy(true);
    try { await fn(); } catch (e) { flash(errText(e), true); } finally { setBusy(false); }
  };

  const saveFolder = () => wrap(async () => { await api.syncSetFolder(folder); flash(t("syncPage.folderSaved")); await load(); });
  const exportNow = () => wrap(async () => { flash(await api.syncExport()); await load(); });
  const preview = () => wrap(async () => {
    const s = await api.syncPreview();
    setSugs(s);
    setChosen(new Set(syncSuggestionIds(s)));
    if (s.length === 0) flash(t("syncPage.upToDate"));
  });
  const ids = syncSuggestionIds(sugs ?? []);
  const picked = (sugs ?? []).filter((_, i) => chosen.has(ids[i]));
  const adopt = () => {
    if (!sugs) return;
    onAdopt(picked);
    setSugs(null);
  };

  return (
    <main className="page">
      <div className="page-top">
        <div className="page-head">
          <div className="page-title">
            <h1>{t("syncPage.title")}</h1>
            <span className="muted small">{t("syncPage.intro")}</span>
          </div>
        </div>
      </div>
      <div className="page-body">
        <div className="settings">
          <section className="sgroup">
            <h2>{t("syncPage.folder")}</h2>
            <div className="srow">
              <input className="input mono grow sensitive" value={folder} onChange={(e) => setFolder(e.target.value)} placeholder={t("syncPage.folderPlaceholder")} />
              <button className="btn" disabled={busy || !folder.trim()} onClick={saveFolder}>{t("common.save")}</button>
              {status?.folder && <button className="btn" onClick={() => { api.openPath(status.folder!).catch((e) => flash(errText(e), true)); }}>{t("common.open")}</button>}
            </div>
            <div className="srow muted small">
              {status?.fileExists
                ? t(status.machine ? "syncPage.fileFromMachine" : "syncPage.fileExported", {
                    machine: status.machine ?? "",
                    time: status.exportedAt ? new Date(status.exportedAt).toLocaleString(locale()) : t("syncPage.unknownTime"),
                  })
                : status?.folder ? t("syncPage.noFile") : t("syncPage.noFolder")}
            </div>
          </section>

          <section className="sgroup">
            <h2>{t("syncPage.sync")}</h2>
            <div className="srow">
              <div className="grow">
                <div className="slabel">{t("syncPage.exportLabel")}</div>
                <div className="muted small">{t("syncPage.exportDesc")}</div>
              </div>
              <button className="btn primary" disabled={busy || !status?.folder} onClick={exportNow}>{t("syncPage.export")}</button>
            </div>
            <div className="srow">
              <div className="grow">
                <div className="slabel">{t("syncPage.importLabel")}</div>
                <div className="muted small">{t("syncPage.importDesc")}</div>
              </div>
              <button className="btn" disabled={busy || !status?.fileExists} onClick={preview}>{t("syncPage.compare")}</button>
            </div>
          </section>

          {sugs && sugs.length > 0 && (
            <section className="sgroup">
              <h2>{t("syncPage.importable")}</h2>
              {sugs.map((s, i) => { const id = ids[i]; return (
                <label key={id} className="srow check-row">
                  <input type="checkbox" checked={chosen.has(id)} onChange={() => setChosen((c) => toggled(c, id))} />
                  <AgentIcon id={s.agent} size={22} />
                  <div className="grow minw0">
                    <div className="slabel">{s.title}</div>
                    <div className="muted small ellipsis">{scrub(s.detail)}</div>
                  </div>
                </label>
              ); })}
              <div className="srow">
                <span className="grow muted small">{t("syncPage.noKeyHint")}</span>
                <button className="btn primary" disabled={picked.length === 0} onClick={adopt}>{t("syncPage.adopt", { n: picked.length })}</button>
              </div>
            </section>
          )}
        </div>
      </div>
    </main>
  );
}
