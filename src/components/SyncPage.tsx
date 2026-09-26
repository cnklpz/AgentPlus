import { type KeyboardEvent, useEffect, useRef, useState } from "react";
import { type SyncHistoryEntry, type SyncOptions, type SyncStatus, type SyncSuggestion, api } from "../api";
import { AgentIcon, Icon } from "./icons";
import { ErrorBox, Seg, SettingRow, Switch, ToggleRow } from "./controls";
import { ConfirmFrame, Modal } from "./Modal";
import { ask } from "./Confirm";
import { useEscape } from "../hooks";
import { syncSuggestionIds } from "../services";
import { locale, t, tn, useLang } from "../i18n";
import { scrub } from "../privacy";
import { copyText, errText, type Flash, toggled } from "../util";

interface Props {
  flash: Flash;
  /** Adds the chosen suggestions to the drafts of their agents (library changes are written right away). */
  onAdopt: (s: SyncSuggestion[]) => Promise<boolean>;
  /** Changes after an automatic sync: reload the status and the records. */
  tick: number;
}

/** Same minimum as the backend (`seal::MIN_LEN`). */
const MIN_PASSWORD = 8;
/** Choices for the number of sync records kept. */
const KEEP_CHOICES = [5, 10, 20, 50];

/** Setting a new sync password, or unlocking a file another device encrypted. */
type DialogMode = "set" | "unlock";

const when = (iso: string | null) => (iso ? new Date(iso).toLocaleString(locale(), { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" }) : t("syncPage.unknownTime"));

export function SyncPage({ flash, onAdopt, tick }: Props) {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [history, setHistory] = useState<SyncHistoryEntry[]>([]);
  /** The folder field; differs from `status.folder` while being edited. */
  const [path, setPath] = useState("");
  const [sugs, setSugs] = useState<SyncSuggestion[] | null>(null);
  /** The sync record the suggestions come from; null = the current sync file. */
  const [source, setSource] = useState<SyncHistoryEntry | null>(null);
  const [chosen, setChosen] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [dialog, setDialog] = useState<DialogMode | null>(null);
  const importRef = useRef<HTMLElement>(null);
  const lang = useLang();

  const load = async () => {
    try {
      const [s, h] = await Promise.all([api.syncStatus(), api.syncHistory()]);
      setStatus(s);
      setPath(s.folder ?? "");
      setHistory(h);
    } catch (e) { flash(errText(e), true); }
  };
  useEffect(() => { load(); }, [tick]);
  // The suggestions' titles are rendered by the backend: re-fetch them in the new language.
  useEffect(() => { if (sugs) api.syncPreview(source?.id).then(setSugs).catch(() => undefined); }, [lang]);

  const wrap = async (fn: () => Promise<void>) => {
    setBusy(true);
    try { await fn(); } catch (e) { flash(errText(e), true); } finally { setBusy(false); }
  };

  const applyFolder = async (p: string) => { await api.syncSetFolder(p); flash(t("syncPage.folderSaved")); setSugs(null); await load(); };
  // A picked folder is saved right away: choosing it is the whole intent.
  const browse = () => wrap(async () => {
    const p = await api.pickFolder(status?.folder ?? null, "sync");
    if (p) await applyFolder(p);
  });
  const pathDirty = path.trim() !== (status?.folder ?? "");
  const savePath = () => { if (path.trim() && pathDirty) wrap(() => applyFolder(path.trim())); };
  const openFolder = () => { if (status?.folder) api.openPath(status.folder).catch((e) => flash(errText(e), true)); };
  const exportNow = () => wrap(async () => { flash(await api.syncExport()); await load(); });
  /** Compares the sync file, or one record, with this device. */
  const compare = (rec: SyncHistoryEntry | null) => wrap(async () => {
    const s = await api.syncPreview(rec?.id);
    setSource(rec);
    setSugs(s);
    setChosen(new Set(syncSuggestionIds(s)));
    if (s.length === 0) flash(t(rec ? "syncPage.recordUpToDate" : "syncPage.upToDate"));
    else requestAnimationFrame(() => importRef.current?.scrollIntoView({ behavior: "smooth", block: "nearest" }));
    if (!rec) await load();
  });
  const deleteRecord = (rec: SyncHistoryEntry) => wrap(async () => {
    const title = t("syncPage.deleteRecordTitle", { time: when(rec.exportedAt), machine: rec.machine ?? t("syncPage.unknownDevice") });
    if (!(await ask({ title, message: t(rec.current ? "syncPage.deleteCurrentMsg" : "syncPage.deleteRecordMsg"), danger: true, confirmText: t("common.delete") }))) return;
    flash(await api.syncDeleteRecords([rec.id]));
    if (source?.id === rec.id) { setSugs(null); setSource(null); }
    await load();
  });
  const ids = syncSuggestionIds(sugs ?? []);
  const picked = (sugs ?? []).filter((_, i) => chosen.has(ids[i]));
  const adopt = () => wrap(async () => {
    if (sugs && await onAdopt(picked)) { setSugs(null); setSource(null); }
  });

  const on = !!status?.hasPassword;
  const locked = !!status?.fileEncrypted && (!on || !!status.passwordError);
  const turnOff = () => wrap(async () => { flash(await api.syncSetPassword(null, false)); await load(); });
  const setIncludeKeys = (v: boolean) => wrap(async () => { await api.syncSetIncludeKeys(v); await load(); });
  const [askPlainKeys, setAskPlainKeys] = useState(false);
  // Without a password the keys would be written in clear text: ask first.
  const toggleKeys = (v: boolean) => { if (v && !on) setAskPlainKeys(true); else setIncludeKeys(v); };
  const passwordSaved = async (msg: string) => { setDialog(null); flash(msg); await load(); };

  const setOptions = (o: Partial<SyncOptions>) => wrap(async () => {
    if (!status) return;
    const next = { ...status.options, ...o };
    // Fewer records than there are now deletes the oldest ones from the folder.
    const drop = history.length - next.keep;
    if (o.keep !== undefined && drop > 0
      && !(await ask({ title: t("syncPage.keepTitle", { n: next.keep }), message: tn("syncPage.keepMsg", drop), danger: true, confirmText: t("common.delete") }))) return;
    await api.syncSetOptions(next);
    await load();
  });

  const fileLine = status?.fileExists
    ? t(status.machine ? "syncPage.fileFromMachine" : "syncPage.fileExported", { machine: status.machine ?? "", time: status.exportedAt ? new Date(status.exportedAt).toLocaleString(locale()) : t("syncPage.unknownTime") })
    : t("syncPage.noFile");

  return (
    <>
      <main className="page">
        <div className="page-top">
          <div className="page-head">
            <div className="page-title">
              <h1>{t("syncPage.title")}</h1>
              <span className="muted small hint">{t("syncPage.intro")}</span>
            </div>
          </div>
        </div>
        <div className="page-body">
          <div className="settings">
            <section className="sgroup">
              <h2>{t("syncPage.folder")}</h2>
              <div className="srow stacked sync-folder">
                <div className={`path-field${pathDirty ? " dirty" : ""}`}>
                  <input className="mono sensitive" value={path} spellCheck={false} aria-label={t("syncPage.folder")} placeholder={t("syncPage.folderPlaceholder")}
                    onChange={(e) => setPath(e.target.value)}
                    onKeyDown={(e) => { if (e.key === "Enter") savePath(); if (e.key === "Escape") setPath(status?.folder ?? ""); }} />
                  {pathDirty && path.trim() && <button className="pf-btn accent" disabled={busy} onClick={savePath}>{t("common.save")}</button>}
                  {!pathDirty && status?.folder && <button className="pf-btn" onClick={openFolder}>{t("common.open")}</button>}
                  <button className={`pf-btn${status?.folder ? "" : " accent"}`} disabled={busy} onClick={browse}>{t("syncPage.browse")}</button>
                </div>
                <div className="sync-file muted small">
                  {status?.folder ? (
                    <>
                      <span>{fileLine}</span>
                      {status.fileExists && <span className={`sync-chip ${status.fileEncrypted ? "chip-ok" : "chip-muted"}`}>{t(status.fileEncrypted ? "syncPage.encrypted" : "syncPage.notEncrypted")}</span>}
                      {status.filePlainKeys && <span className="sync-chip chip-bad">{t("syncPage.plainKeysChip")}</span>}
                    </>
                  ) : <span>{t("syncPage.noFolderHint")}</span>}
                </div>
              </div>
            </section>

            <section className="sgroup">
              <h2>{t("syncPage.encryption")}</h2>
              {locked && (
                <SettingRow className="sync-locked" label={t("syncPage.lockedLabel")} desc={t("syncPage.lockedDesc")} keepDesc>
                  <button className="btn primary" onClick={() => setDialog("unlock")}>{t("syncPage.enterPassword")}</button>
                </SettingRow>
              )}
              <SettingRow keepDesc
                label={<span className="sync-label">{t("syncPage.password")}<span className={`sync-chip ${on ? "chip-ok" : "chip-muted"}`}>{t(on ? "syncPage.on" : "syncPage.off")}</span></span>}
                desc={on ? t(status!.systemProtected ? "syncPage.passwordOnSystem" : "syncPage.passwordOnFile") : t("syncPage.passwordOff")}
                note={status?.passwordError ? <ErrorBox text={status.passwordError} className="sync-pw-err" /> : undefined}>
                <div className="sync-actions">
                  {on && <button className="btn danger" disabled={busy} onClick={turnOff}>{t("syncPage.turnOff")}</button>}
                  <button className={`btn${on ? "" : " primary"}`} disabled={busy} onClick={() => setDialog("set")}>{t(on ? "syncPage.changePassword" : "syncPage.setPassword")}</button>
                </div>
              </SettingRow>
              <SettingRow label={t("syncPage.includeKeys")} keepDesc desc={t(on ? "syncPage.includeKeysDesc" : status?.includeKeys ? "syncPage.includeKeysPlainOn" : "syncPage.includeKeysPlainOff")}>
                <Switch on={!!status?.includeKeys} disabled={busy} label={t("syncPage.includeKeys")} onChange={toggleKeys} />
              </SettingRow>
            </section>

            <section className="sgroup">
              <h2>{t("syncPage.sync")}</h2>
              <SettingRow label={t("syncPage.exportLabel")}
                desc={t(!on ? (status?.includeKeys ? "syncPage.exportDescPlainKeys" : "syncPage.exportDescPlain") : status?.includeKeys ? "syncPage.exportDescKeys" : "syncPage.exportDescEncrypted")}>
                <button className="btn primary" disabled={busy || !status?.folder || !!status?.passwordError} onClick={exportNow}>{t("syncPage.export")}</button>
              </SettingRow>
              <SettingRow keepDesc={!!status?.remotePending}
                label={<span className="sync-label">{t("syncPage.importLabel")}{status?.remotePending && <span className="sync-chip chip-ok">{t("syncPage.newFrom", { machine: status.machine ?? "" })}</span>}</span>}
                desc={t(status?.remotePending ? "syncPage.importDescPending" : "syncPage.importDesc")}>
                <button className={`btn${status?.remotePending ? " primary" : ""}`} disabled={busy || !status?.fileExists || locked} onClick={() => compare(null)}>{t("syncPage.compare")}</button>
              </SettingRow>
            </section>

            {sugs && sugs.length > 0 && (
              <section className="sgroup" ref={importRef}>
                <h2 className="row between">
                  <span>{source ? t("syncPage.fromRecord", { time: when(source.exportedAt), machine: source.machine ?? "" }) : t("syncPage.importable")}</span>
                  <button className="link" onClick={() => { setSugs(null); setSource(null); }}>{t("common.close")}</button>
                </h2>
                {sugs.map((s, i) => { const id = ids[i]; return (
                  <SettingRow key={id} as="label" className="check-row" label={s.title} desc={scrub(s.detail)} descClassName="ellipsis" keepDesc
                    lead={<><input type="checkbox" checked={chosen.has(id)} onChange={() => setChosen((c) => toggled(c, id))} />
                      {s.agent === "library" ? <span className="sync-row-icon on" title={t("syncPage.library")}><Icon.layers size={14} /></span> : <AgentIcon id={s.agent} size={22} />}</>} />
                ); })}
                <div className="srow">
                  <span className="grow muted small hint">{t(source ? "syncPage.restoreHint" : "syncPage.adoptHint")}</span>
                  <button className="btn primary" disabled={busy || picked.length === 0} onClick={adopt}>{t(source ? "syncPage.restore" : "syncPage.adopt", { n: picked.length })}</button>
                </div>
              </section>
            )}
          </div>
        </div>
        {askPlainKeys && (
          <PlainKeysWarning
            onCancel={() => setAskPlainKeys(false)}
            onSetPassword={() => { setAskPlainKeys(false); setDialog("set"); }}
            onContinue={() => { setAskPlainKeys(false); setIncludeKeys(true); }} />
        )}
        {dialog && <PasswordDialog mode={dialog} changing={on} flash={flash} onClose={() => setDialog(null)} onSaved={passwordSaved} />}
      </main>

      <aside className="aside sync-aside" aria-label={t("syncPage.asideLabel")}>
        <section className="aside-cur">
          <h2>{t("syncPage.autoTitle")}</h2>
          <ToggleRow title={t("syncPage.onStart")} hint={t("syncPage.onStartHint")} on={!!status?.options.onStart} disabled={busy || !status?.folder}
            onChange={(v) => setOptions({ onStart: v })} />
          <ToggleRow title={t("syncPage.onChange")} hint={t("syncPage.onChangeHint")} on={!!status?.options.onChange} disabled={busy || !status?.folder}
            onChange={(v) => setOptions({ onChange: v })} />
          <div className="sync-keep">
            <span className="small">{t("syncPage.keep")}</span>
            <Seg value={status?.options.keep ?? 10} label={t("syncPage.keep")} onChange={(v) => setOptions({ keep: v })}
              options={[...new Set([...KEEP_CHOICES, status?.options.keep ?? 10])].sort((a, b) => a - b).map((n) => ({ value: n, label: String(n) }))} />
          </div>
          {!status?.folder && <span className="muted tiny">{t("syncPage.autoNeedsFolder")}</span>}
        </section>
        <div className="sync-history-head">
          <h2>{t("syncPage.history")}</h2>
          <span className="count">{history.length}</span>
        </div>
        <div className="sync-history">
          {history.length === 0 ? (
            <div className="dempty">
              <span className="small strong">{t("syncPage.historyEmpty")}</span>
              <span className="muted tiny">{t("syncPage.historyEmptyHint")}</span>
            </div>
          ) : history.map((h) => (
            <div key={h.id} className={`sync-rec${source?.id === h.id && sugs ? " on" : ""}`}>
              <button className="sync-rec-main" disabled={busy} onClick={() => compare(h)} title={t("syncPage.recordTitle")}>
                <span className="sync-rec-time">{when(h.exportedAt)}</span>
                <span className="sync-rec-meta">
                  <span className="ellipsis">{h.machine ?? t("syncPage.unknownDevice")}</span>
                  {h.mine && <span className="sync-tag">{t("syncPage.thisDevice")}</span>}
                  {h.current && <span className="sync-tag accent">{t("syncPage.currentFile")}</span>}
                  {!h.encrypted && <span className="sync-tag warn">{t("syncPage.notEncrypted")}</span>}
                </span>
              </button>
              <button className="sync-rec-del" disabled={busy} onClick={() => deleteRecord(h)}
                aria-label={t("syncPage.deleteRecord")} title={t("syncPage.deleteRecord")}><Icon.trash size={13} /></button>
            </div>
          ))}
        </div>
      </aside>
    </>
  );
}

/** Sets the sync password (typed, or a generated random key), or unlocks an encrypted file. */
function PasswordDialog({ mode, changing, flash, onClose, onSaved }: {
  mode: DialogMode; changing: boolean; flash: Flash; onClose: () => void; onSaved: (msg: string) => void;
}) {
  const [kind, setKind] = useState<"password" | "key">("password");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [key, setKey] = useState("");
  const [keptKey, setKeptKey] = useState(false);
  const [show, setShow] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const newKey = () => api.syncGeneratePassword().then((k) => { setKey(k); setKeptKey(false); }).catch((e) => setErr(errText(e)));
  // Saving it to a file counts as keeping it.
  const saveKey = async () => {
    setBusy(true);
    try {
      const msg = await api.syncSaveKey(key);
      if (msg) { flash(msg); setKeptKey(true); }
    } catch (e) { setErr(errText(e)); } finally { setBusy(false); }
  };
  const pickKind = (k: typeof kind) => { setKind(k); setErr(null); if (k === "key" && !key) newKey(); };

  const unlock = mode === "unlock";
  const secret = unlock || kind === "password" ? password : key;
  const problem: string | null =
    secret.trim().length < MIN_PASSWORD ? t("syncPage.passwordTooShort", { n: MIN_PASSWORD })
    : !unlock && kind === "password" && password !== confirm ? t("syncPage.passwordMismatch")
    : !unlock && kind === "key" && !keptKey ? t("syncPage.keyNotKept")
    : null;
  // Only complain once something was typed.
  const shown = err ?? (password || confirm ? problem : null);

  const save = async () => {
    if (problem || busy) return;
    setBusy(true);
    setErr(null);
    try { onSaved(await api.syncSetPassword(secret, unlock)); } catch (e) { setErr(errText(e)); } finally { setBusy(false); }
  };
  const enter = (e: KeyboardEvent) => { if (e.key === "Enter") save(); };
  const title = t(unlock ? "syncPage.unlockTitle" : changing ? "syncPage.changeTitle" : "syncPage.setTitle");

  return (
    <Modal label={title} title={title} busy={busy} onClose={onClose} className="sync-pw-dialog" foot={<>
      <span className="grow" />
      <button className="btn" disabled={busy} onClick={onClose}>{t("common.cancel")}</button>
      <button className="btn primary" disabled={busy || !!problem} onClick={save}>{t(unlock ? "syncPage.unlock" : "common.save")}</button>
    </>}>
      <p className="muted small">{t(unlock ? "syncPage.unlockIntro" : "syncPage.setIntro")}</p>
      {!unlock && (
        <Seg value={kind} onChange={pickKind} label={t("syncPage.kind")} options={[
          { value: "password", label: t("syncPage.kindPassword") },
          { value: "key", label: t("syncPage.kindKey") },
        ]} />
      )}
      {unlock || kind === "password" ? (
        <>
          <div className="field">
            <label htmlFor="sync-pw">{t(unlock ? "syncPage.password" : "syncPage.newPassword")}</label>
            <div className="row gap6">
              <input id="sync-pw" className="input mono grow sensitive" type={show ? "text" : "password"} autoComplete="new-password" autoFocus
                value={password} onChange={(e) => { setPassword(e.target.value); setErr(null); }} onKeyDown={enter}
                placeholder={t(unlock ? "syncPage.unlockPlaceholder" : "syncPage.passwordPlaceholder", { n: MIN_PASSWORD })} />
              <button className="btn" type="button" onClick={() => setShow(!show)}>{t(show ? "syncPage.hide" : "syncPage.show")}</button>
            </div>
          </div>
          {!unlock && (
            <div className="field">
              <label htmlFor="sync-pw2">{t("syncPage.confirmPassword")}</label>
              <input id="sync-pw2" className="input mono sensitive" type={show ? "text" : "password"} autoComplete="new-password"
                value={confirm} onChange={(e) => { setConfirm(e.target.value); setErr(null); }} onKeyDown={enter} />
            </div>
          )}
        </>
      ) : (
        <div className="field">
          <span className="field-label">{t("syncPage.keyLabel")}</span>
          <input className="input mono sensitive sync-key" readOnly value={key} aria-label={t("syncPage.keyLabel")} onFocus={(e) => e.target.select()} />
          <em className="muted tiny">{t("syncPage.keyHint")}</em>
          <div className="row gap6">
            <button className="btn" type="button" disabled={!key} onClick={() => copyText(key, flash)}><Icon.copy size={12} /> {t("common.copy")}</button>
            <button className="btn" type="button" disabled={!key || busy} onClick={saveKey}><Icon.download size={12} /> {t("syncPage.saveKey")}</button>
            <span className="grow" />
            <button className="btn" type="button" onClick={newKey}><Icon.refresh size={12} /> {t("syncPage.regenerate")}</button>
          </div>
        </div>
      )}
      {!unlock && kind === "key" && (
        <label className="row gap6 small sync-kept">
          <input type="checkbox" checked={keptKey} onChange={(e) => setKeptKey(e.target.checked)} />
          {t("syncPage.keyKept")}
        </label>
      )}
      {!unlock && <p className="muted tiny">{t("syncPage.sameEverywhere")}</p>}
      {shown && <ErrorBox text={shown} />}
    </Modal>
  );
}

type PlainKeysProps = { onCancel: () => void; onSetPassword: () => void; onContinue: () => void };

/** The "Excessive" motion level, unless the system asks for reduced motion. */
const excessiveMotion = () => document.documentElement.dataset.motion === "rich" && !window.matchMedia("(prefers-reduced-motion: reduce)").matches;

/** Asks before API keys go into an unencrypted sync file: a plain warning dialog, or the
 *  full-screen hazard tapes at the "Excessive" motion level. */
function PlainKeysWarning(props: PlainKeysProps) {
  const [loud] = useState(excessiveMotion);
  if (loud) return <HazardScreen {...props} />;
  const { onCancel, onSetPassword, onContinue } = props;
  return (
    <ConfirmFrame title={t("syncPage.hazardTitle")} icon={<Icon.warn size={16} />} danger message={t("syncPage.hazardText")} onClose={onCancel}
      foot={<>
        <button className="btn danger" onClick={onContinue}>{t("syncPage.hazardContinue")}</button>
        <span className="grow" />
        <button className="btn" onClick={onSetPassword}>{t("syncPage.setPassword")}</button>
        <button className="btn primary" autoFocus onClick={onCancel}>{t("common.cancel")}</button>
      </>} />
  );
}

/** Two hazard tapes fly in from opposite sides and cross over the whole window, then the question pops up. */
/** How long the exit plays (card out, tapes fly on, backdrop clears) before the choice goes through. */
const HAZARD_EXIT_MS = 560;

function HazardScreen({ onCancel, onSetPassword, onContinue }: PlainKeysProps) {
  const [leaving, setLeaving] = useState(false);
  const timer = useRef(0);
  useEffect(() => () => window.clearTimeout(timer.current), []);
  /** Plays the exit, then runs the choice; a second click while leaving is ignored. */
  const leave = (then: () => void) => () => {
    if (leaving) return;
    setLeaving(true);
    timer.current = window.setTimeout(then, HAZARD_EXIT_MS);
  };
  useEscape(leave(onCancel));
  return (
    <div className={`hz-screen${leaving ? " out" : ""}`} role="alertdialog" aria-modal="true" aria-labelledby="hz-title" aria-describedby="hz-text">
      <div className="hz-tape a" />
      <div className="hz-tape b" />
      <div className="hz-card">
        <span className="hz-sign"><Icon.warn size={30} /></span>
        <h2 id="hz-title">{t("syncPage.hazardTitle")}</h2>
        <p id="hz-text">{t("syncPage.hazardText")}</p>
        <div className="hz-actions">
          <button className="btn hz-btn" disabled={leaving} onClick={leave(onContinue)}>{t("syncPage.hazardContinue")}</button>
          <span className="grow" />
          <button className="btn hz-btn" disabled={leaving} onClick={leave(onSetPassword)}>{t("syncPage.setPassword")}</button>
          <button className="btn hz-btn solid" autoFocus disabled={leaving} onClick={leave(onCancel)}>{t("common.cancel")}</button>
        </div>
      </div>
    </div>
  );
}
