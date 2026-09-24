import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { type AgentDetect, type AgentId, type EnvInfo, api } from "../api";
import { inTauri } from "../tauri";
import type { CloseAction, Motion, Prefs, RestartProgressPref, Theme } from "../prefs";
import { LANGS, type LangPref, type TKey, locale, t, useLang } from "../i18n";
import { useLoad, usePageEscape } from "../hooks";
import { AGENT_NAME } from "../services";
import { AgentIcon, EnvIcon, Icon } from "./icons";
import { Modal } from "./Modal";
import { ErrorBox, Seg, SettingRow, Switch } from "./controls";
import { TabBar, useSlideDir } from "./TabBar";
import { scrub } from "../privacy";
import { fmtSize } from "../format";
import { RELEASES_URL, checkUpdate, installUpdate, updateBusy, useUpdate } from "../updater";
import { errText, type Flash, toggled } from "../util";

export type SettingsTab = "general" | "agents";

interface Props {
  tab: SettingsTab;
  setTab: (t: SettingsTab) => void;
  prefs: Prefs;
  setPrefs: (p: Prefs) => void;
  envs: EnvInfo[];
  switching: boolean;
  onEnv: (id: string) => void;
  onHistory: () => void;
  /** Detection or a folder changed: reload agents. */
  onAgentsChanged: () => void;
  flash: Flash;
  /** Back to where the user came from. */
  onClose: () => void;
}

const MOTION: { v: Motion; label: TKey; hint: TKey }[] = [
  { v: "rich", label: "settingsPage.motionRich", hint: "settingsPage.motionRichHint" },
  { v: "full", label: "settingsPage.motionFull", hint: "settingsPage.motionFullHint" },
  { v: "reduced", label: "settingsPage.motionReduced", hint: "settingsPage.motionReducedHint" },
  { v: "off", label: "settingsPage.motionOff", hint: "settingsPage.motionOffHint" },
];

const THEMES: { v: Theme; label: TKey; icon: React.ReactNode }[] = [
  { v: "auto", label: "settingsPage.themeAuto", icon: <Icon.monitor size={13} /> },
  { v: "light", label: "settingsPage.themeLight", icon: <Icon.sun size={13} /> },
  { v: "dark", label: "settingsPage.themeDark", icon: <Icon.moon size={13} /> },
];

const RESTART_PROGRESS: { v: RestartProgressPref; label: TKey; hint: TKey }[] = [
  { v: "dialog", label: "settingsPage.restartDialog", hint: "settingsPage.restartDialogHint" },
  { v: "toast", label: "settingsPage.restartToast", hint: "settingsPage.restartToastHint" },
];

const CLOSE_ACTIONS: { v: CloseAction; label: TKey; hint: TKey }[] = [
  { v: "ask", label: "settingsPage.closeAsk", hint: "settingsPage.closeAskHint" },
  { v: "tray", label: "common.minimizeToTray", hint: "settingsPage.closeTrayHint" },
  { v: "quit", label: "common.quitApp", hint: "settingsPage.closeQuitHint" },
];

/** AgentPlus's own settings (top-right gear): general options and agent detection. */
export function SettingsPage(props: Props) {
  const { tab, setTab } = props;
  const env = props.envs.find((e) => e.current);
  const slide = useSlideDir(tab, ["general", "agents"] as const);
  // Esc goes back, unless a dialog or menu is open or a field is being edited.
  usePageEscape(props.onClose, true, { skipInputs: true });
  return (
    <main className="page settings-page">
      <div className="page-top">
        <div className="page-head">
          <span className="page-icon"><Icon.gear size={20} /></span>
          <div className="page-title">
            <h1>{t("settingsPage.title")}</h1>
            <span className="muted small">{t("settingsPage.subtitle")}</span>
          </div>
          <button className="icon-btn" aria-label={t("settingsPage.closeSettings")} title={t("settingsPage.closeEsc")} onClick={props.onClose}><Icon.close /></button>
        </div>
        <TabBar items={[{ id: "general", label: t("settingsPage.tabGeneral") }, { id: "agents", label: t("settingsPage.tabAgents") }]} value={tab} onChange={setTab} />
      </div>
      <div className={`page-body slide-${slide}`} key={tab}>
        {tab === "general" ? <General {...props} /> : <Detection envLabel={env?.label ?? t("common.localWindows")} onChanged={props.onAgentsChanged} flash={props.flash} prefs={props.prefs} setPrefs={props.setPrefs} />}
      </div>
    </main>
  );
}

function General({ prefs, setPrefs, envs, switching, onEnv, onHistory, flash }: Props) {
  // The app's own version (tauri.conf.json); not available in the plain-browser preview.
  const [version, setVersion] = useState<string | null>(null);
  useEffect(() => {
    if (!inTauri) return;
    let alive = true;
    getVersion().then((v) => { if (alive) setVersion(v); }).catch(() => undefined);
    return () => { alive = false; };
  }, []);
  return (
    <div className="settings">
      <section className="sgroup">
        <h2>{t("settingsPage.envsTitle")}</h2>
        <div className="srow stacked">
          <div className="env-cards row-cards">
            {envs.map((e) => (
              <button key={e.id} className={`env-card${e.current ? " on" : ""}`} disabled={switching} onClick={() => !e.current && onEnv(e.id)}>
                <span className="env-ico"><EnvIcon id={e.id} /></span>
                <span className="grow minw0">
                  <span className="block small strong">{e.label}</span>
                  <span className="block tiny muted ellipsis">{scrub(e.detail)}</span>
                </span>
                {e.current && <Icon.check size={14} color="var(--accent)" />}
              </button>
            ))}
          </div>
          <span className="muted tiny">{t("settingsPage.envsHint")}</span>
        </div>
      </section>

      <section className="sgroup">
        <h2>{t("settingsPage.uiTitle")}</h2>
        <SettingRow label={t("settingsPage.theme")} desc={t("settingsPage.themeHint")}>
          <Seg value={prefs.theme} onChange={(v) => setPrefs({ ...prefs, theme: v })} label={t("settingsPage.theme")}
            options={THEMES.map((m) => ({ value: m.v, label: <>{m.icon}{t(m.label)}</> }))} />
        </SettingRow>
        <SettingRow label={t("settingsPage.language")} desc={t("settingsPage.languageHint")}>
          <Seg value={prefs.lang} onChange={(v) => setPrefs({ ...prefs, lang: v })} label={t("settingsPage.language")}
            options={[{ value: "auto" as LangPref, label: t("settingsPage.langAuto") }, ...LANGS.map((l) => ({ value: l.id, label: l.label, lang: l.id }))]} />
        </SettingRow>
        <SettingRow label={t("settingsPage.motion")} desc={t("settingsPage.motionHint", { hint: t(MOTION.find((m) => m.v === prefs.motion)?.hint ?? "settingsPage.motionFullHint") })}>
          <Seg value={prefs.motion} onChange={(v) => setPrefs({ ...prefs, motion: v })} label={t("settingsPage.motion")}
            options={MOTION.map((m) => ({ value: m.v, label: t(m.label), title: t(m.hint) }))} />
        </SettingRow>
        <SettingRow label={t("settingsPage.autoLatency")} desc={t("settingsPage.autoLatencyHint")}>
          <Switch on={prefs.autoLatency} onChange={(v) => setPrefs({ ...prefs, autoLatency: v })} label={t("settingsPage.autoLatency")} />
        </SettingRow>
        <SettingRow label={t("settingsPage.restartProgress")}
          desc={t("settingsPage.restartProgressHint", { hint: t(RESTART_PROGRESS.find((m) => m.v === prefs.restartProgress)?.hint ?? "settingsPage.restartDialogHint") })}>
          <Seg value={prefs.restartProgress} onChange={(v) => setPrefs({ ...prefs, restartProgress: v })} label={t("settingsPage.restartProgress")}
            options={RESTART_PROGRESS.map((m) => ({ value: m.v, label: t(m.label) }))} />
        </SettingRow>
        <SettingRow label={t("settingsPage.privacy")} desc={t("settingsPage.privacyHint")}>
          <Switch on={prefs.privacy} onChange={(v) => setPrefs({ ...prefs, privacy: v })} label={t("settingsPage.privacy")} />
        </SettingRow>
        <SettingRow label={t("settingsPage.closeAction")}
          desc={t("settingsPage.closeActionHint", { hint: t(CLOSE_ACTIONS.find((m) => m.v === prefs.closeAction)?.hint ?? "settingsPage.closeAskHint") })}>
          <Seg value={prefs.closeAction} onChange={(v) => setPrefs({ ...prefs, closeAction: v })} label={t("settingsPage.closeAction")}
            options={CLOSE_ACTIONS.map((m) => ({ value: m.v, label: t(m.label) }))} />
        </SettingRow>
      </section>

      <section className="sgroup">
        <h2>{t("settingsPage.dataTitle")}</h2>
        <SettingRow label={t("settingsPage.dataDir")} desc={t("settingsPage.dataDirHint")} descClassName="mono">
          <button className="btn" onClick={() => api.openDataDir().catch((e) => flash(errText(e), true))}><Icon.folder />{t("common.open")}</button>
          <button className="btn" onClick={onHistory}><Icon.history size={14} />{t("settingsPage.backups")}</button>
        </SettingRow>
      </section>

      <section className="sgroup">
        <h2>{t("settingsPage.aboutTitle")}</h2>
        <SettingRow label={version ? t("settingsPage.aboutVersion", { version }) : "AgentPlus"} desc={t("settingsPage.aboutHint")} />
        <UpdateRow flash={flash} />
        <SettingRow label={t("settingsPage.autoUpdate")} desc={t("settingsPage.autoUpdateHint")}>
          <Switch on={prefs.autoUpdate} onChange={(v) => setPrefs({ ...prefs, autoUpdate: v })} label={t("settingsPage.autoUpdate")} />
        </SettingRow>
      </section>
    </div>
  );
}

/** Check for a new release, read its notes, then download and install it. */
function UpdateRow({ flash }: { flash: Flash }) {
  const u = useUpdate();
  const info = u.kind === "available" || u.kind === "downloading" || u.kind === "installing" || u.kind === "error" ? u.info : null;
  const busy = updateBusy(u);
  const openRelease = () => api.openUrl(info ? `${RELEASES_URL}/tag/v${info.version}` : RELEASES_URL).catch((e) => flash(errText(e), true));
  const pct = u.kind === "downloading" ? (u.total ? u.done / u.total : null) : u.kind === "installing" ? 1 : null;
  const date = info?.date ? new Date(info.date) : null;
  const status =
    u.kind === "checking" ? t("settingsPage.checkingUpdate")
    : u.kind === "latest" ? t("settingsPage.upToDate")
    : u.kind === "downloading" ? t("settingsPage.updateDownloading", { pct: u.total ? `${Math.floor((u.done / u.total) * 100)}%` : fmtSize(u.done) })
    : u.kind === "installing" ? t("settingsPage.updateInstalling")
    : null;
  return (
    <div className="srow stacked">
      <div className="row gap6">
        <div className="grow minw0">
          <div className="slabel">
            {info ? t("settingsPage.updateFound", { version: info.version }) : t("settingsPage.checkUpdate")}
            {info && <span className="chip-ok">v{info.version}</span>}
          </div>
          {date && !Number.isNaN(date.getTime()) && <div className="muted small">{t("settingsPage.updateDate", { date: date.toLocaleDateString(locale()) })}</div>}
          {status && <div className="muted small" role="status">{status}</div>}
          {u.kind === "error" && <ErrorBox text={u.error} alert />}
        </div>
        {info && <button className="btn" onClick={openRelease}><Icon.external size={13} />{t("settingsPage.viewRelease")}</button>}
        {info
          ? <button className="btn primary" disabled={busy} onClick={() => { installUpdate(); }}><Icon.download size={13} />{t("settingsPage.installUpdate")}</button>
          : <button className="btn" disabled={busy} onClick={() => { checkUpdate(); }}><Icon.refresh size={13} />{t(u.kind === "checking" ? "settingsPage.checkingUpdate" : "settingsPage.checkUpdate")}</button>}
      </div>
      {pct !== null && <div className="upd-bar" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct * 100)}><span style={{ width: `${pct * 100}%` }} /></div>}
      {info?.notes && (
        <div className="upd-notes">
          <div className="tiny muted strong">{t("settingsPage.releaseNotes")}</div>
          <div className="small">{info.notes}</div>
        </div>
      )}
      {info && !busy && <span className="tiny muted">{t("settingsPage.updateInstallHint")}</span>}
    </div>
  );
}

function Detection({ envLabel, onChanged, flash, prefs, setPrefs }: {
  envLabel: string; onChanged: () => void; flash: Flash; prefs: Prefs; setPrefs: (p: Prefs) => void;
}) {
  const hidden = new Set(prefs.hiddenAgents);
  const toggleShown = (id: string) => setPrefs({ ...prefs, hiddenAgents: [...toggled(hidden, id)] });
  const [edit, setEdit] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);

  // Shown in the sidebar first, then detected (hidden, or detect-only), then not found.
  // Sorted when loaded, so flipping a switch doesn't make the card jump away.
  const rank = (d: AgentDetect) => (d.enabled && !hidden.has(d.id) ? 0 : d.enabled || d.manual ? 1 : 2);
  const lang = useLang();
  const { data: list, error, reload: load } = useLoad(
    () => api.detectAgents().then((l) => l.map((d, i) => [d, i] as const).sort(([a, i], [b, j]) => rank(a) - rank(b) || i - j).map(([d]) => d)),
    [envLabel, lang],
  );

  const save = async (id: AgentId, path: string | null) => {
    setSaving(id);
    try {
      await api.setAgentDir(id, path);
      setEdit((m) => { const n = { ...m }; delete n[id]; return n; });
      await load();
      onChanged();
      flash(t(path ? "settingsPage.dirUsed" : "settingsPage.dirReset"));
    } catch (e) {
      flash(errText(e), true);
    } finally {
      setSaving(null);
    }
  };

  return (
    <div className="settings">
      <div className="row between">
        <span className="muted small">{t("settingsPage.detectIntro", { env: envLabel })}</span>
        <span className="row gap6 noshrink">
          <button className="btn small" onClick={() => setShowAll(true)}><Icon.layers size={12} />{t("settingsPage.supportedAgents")}</button>
          <button className="btn small" onClick={() => { void load(true); }}><Icon.refresh size={12} />{t("settingsPage.redetect")}</button>
        </span>
      </div>
      {showAll && <SupportedAgents found={new Set(list?.filter((d) => d.enabled || d.manual).map((d) => d.id))} onClose={() => setShowAll(false)} />}
      {error && (list ? <ErrorBox text={error} /> : <div className="empty">{scrub(error)}</div>)}
      {!list && !error && <div className="empty">{t("settingsPage.detecting")}</div>}
      <div className="detect-grid">
      {list?.map((d) => {
        const draft = edit[d.id];
        return (
          <section key={d.id} className={`sgroup detect${d.enabled || d.manual ? "" : " off"}`}>
            <div className="detect-head">
              <AgentIcon id={d.id} size={36} />
              <div className="grow minw0">
                <div className="slabel">{d.name}{d.version && <span className="tiny muted mono">{d.version}</span>}</div>
                <div className="muted small">
                  {t(d.appFound ? (d.running ? "settingsPage.installedRunning" : "settingsPage.installed") : "settingsPage.appNotFound")}
                  {" · "}{t(d.configFound ? "settingsPage.configFound" : "settingsPage.noConfig")}
                </div>
              </div>
              {d.manual ? <span className="chip-muted">{t("settingsPage.detectOnly")}</span> : <span className={d.enabled ? "chip-ok" : "chip-muted"}>{t(d.enabled ? "settingsPage.detected" : "settingsPage.notDetected")}</span>}
            </div>
            {d.enabled && (
              <label className="srow detect-show">
                <span className="grow minw0">
                  <span className="small strong">{t("settingsPage.showInSidebar")}</span>
                  <span className="block tiny muted">{t("settingsPage.showInSidebarHint")}</span>
                </span>
                <Switch on={!hidden.has(d.id)} onChange={() => toggleShown(d.id)} label={t("settingsPage.showInSidebarAria", { name: d.name })} />
              </label>
            )}
            {d.note && <div className="detect-note tiny">{scrub(d.note)}</div>}
            {!d.manual && <div className="srow stacked">
              <div className="row gap6">
                <span className="small strong">{t("settingsPage.configDir")}</span>
                <span className={`ptag ${d.customDir ? "tag-new" : "tag-soft"}`}>{t(d.customDir ? "settingsPage.dirCustom" : "settingsPage.dirDefault")}</span>
                <span className="grow" />
                <button className="link" onClick={() => api.openPath(d.configDir).catch((e) => flash(errText(e), true))}>{t("common.open")}</button>
              </div>
              {draft === undefined ? (
                <div className="row gap6">
                  <span className="mono small grow ellipsis dir-line" title={scrub(d.configDir)}>{scrub(d.configDir)}</span>
                  <button className="btn small" onClick={() => setEdit((m) => ({ ...m, [d.id]: d.customDir ?? d.configDir }))}><Icon.edit size={12} />{t("settingsPage.change")}</button>
                  {d.customDir && <button className="btn small" disabled={saving === d.id} onClick={() => save(d.id, null)}>{t("settingsPage.restoreDefault")}</button>}
                </div>
              ) : (
                <div className="row gap6">
                  <input className="input mono grow sensitive" autoFocus value={draft} placeholder={scrub(d.defaultDir)}
                    onChange={(e) => setEdit((m) => ({ ...m, [d.id]: e.target.value }))}
                    onKeyDown={(e) => { if (e.key === "Enter") save(d.id, draft); if (e.key === "Escape") setEdit((m) => { const n = { ...m }; delete n[d.id]; return n; }); }} />
                  <button className="btn small" onClick={() => setEdit((m) => { const n = { ...m }; delete n[d.id]; return n; })}>{t("common.cancel")}</button>
                  <button className="btn small primary" disabled={!draft.trim() || saving === d.id} onClick={() => save(d.id, draft)}>{t(saving === d.id ? "settingsPage.checking" : "settingsPage.use")}</button>
                </div>
              )}
              {draft !== undefined && <span className="tiny muted">{t("settingsPage.dirInputHint", { dir: d.defaultDir })}</span>}
            </div>}
          </section>
        );
      })}
      </div>
    </div>
  );
}

/** Every agent AgentPlus knows, in the order it shows them. */
const SUPPORTED: { id: AgentId; kind: TKey }[] = [
  { id: "codex", kind: "settingsPage.kindDesktopCli" },
  { id: "claude", kind: "settingsPage.kindCli" },
  { id: "opencode", kind: "settingsPage.kindDesktopCli" },
  { id: "zcode", kind: "settingsPage.kindDesktop" },
  { id: "mimo", kind: "settingsPage.kindDesktop" },
  { id: "hermes", kind: "settingsPage.kindCli" },
  { id: "gemini", kind: "settingsPage.kindCli" },
  { id: "codebuddy", kind: "settingsPage.kindIdeCli" },
  { id: "qwen", kind: "settingsPage.kindCli" },
  { id: "kimi", kind: "settingsPage.kindCli" },
  { id: "kilo", kind: "settingsPage.kindCliVscode" },
  { id: "droid", kind: "settingsPage.kindCli" },
  { id: "pi", kind: "settingsPage.kindCli" },
  { id: "openclaw", kind: "settingsPage.kindCli" },
  { id: "trae", kind: "settingsPage.kindIdeDetectOnly" },
];

function SupportedAgents({ found, onClose }: { found: Set<AgentId>; onClose: () => void }) {
  return (
    <Modal label={t("settingsPage.supportedAgents")} wide onClose={onClose}
      title={<>{t("settingsPage.supportedAgents")} <span className="muted small">{t("settingsPage.supportedCount", { n: SUPPORTED.length })}</span></>}>
      <div className="agent-wall">
        {SUPPORTED.map((a) => (
          <div key={a.id} className="agent-tile" title={found.has(a.id) ? t("settingsPage.detectedHere") : undefined}>
            <span className="agent-tile-icon">
              <AgentIcon id={a.id} size={44} />
              {found.has(a.id) && <span className="agent-tile-dot" />}
            </span>
            <span className="small strong ellipsis">{AGENT_NAME[a.id]}</span>
            <span className="tiny muted ellipsis">{t(a.kind)}</span>
          </div>
        ))}
      </div>
      <span className="tiny muted">{t("settingsPage.supportedHint")}</span>
    </Modal>
  );
}
