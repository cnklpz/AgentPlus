import { useEffect, useState } from "react";
import { type AgentDetect, type AgentId, type EnvInfo, api } from "../api";
import type { Motion, Prefs, RestartProgressPref, Theme } from "../prefs";
import { LANGS, type TKey, t, useLang } from "../i18n";
import { useEscape } from "../hooks";
import { AGENT_NAME } from "../services";
import { AgentIcon, Icon } from "./icons";
import { TabBar, useSlideDir } from "./TabBar";

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
  flash: (text: string, error?: boolean) => void;
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

/** AgentPlus's own settings (top-right gear): general options and agent detection. */
export function SettingsPage(props: Props) {
  const { tab, setTab } = props;
  const env = props.envs.find((e) => e.current);
  const slide = useSlideDir(tab, ["general", "agents"] as const);
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = e.target instanceof Element ? e.target : null;
      if (e.key === "Escape" && !el?.closest("input, .modal-bg") && !document.querySelector(".modal-bg:not(.ap-ghost)")) props.onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [props.onClose]);
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
        {tab === "general" ? <General {...props} /> : <Detection envLabel={env?.label ?? t("settingsPage.localWindows")} onChanged={props.onAgentsChanged} flash={props.flash} prefs={props.prefs} setPrefs={props.setPrefs} />}
      </div>
    </main>
  );
}

function General({ prefs, setPrefs, envs, switching, onEnv, onHistory, flash }: Props) {
  return (
    <div className="settings">
      <section className="sgroup">
        <h2>{t("settingsPage.envsTitle")}</h2>
        <div className="srow stacked">
          <div className="env-cards row-cards">
            {envs.map((e) => (
              <button key={e.id} className={`env-card${e.current ? " on" : ""}`} disabled={switching} onClick={() => !e.current && onEnv(e.id)}>
                <span className="env-ico">{e.id.startsWith("wsl:") ? <Icon.terminal /> : <Icon.monitor />}</span>
                <span className="grow minw0">
                  <span className="block small strong">{e.label}</span>
                  <span className="block tiny muted ellipsis">{e.detail}</span>
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
        <div className="srow">
          <div className="grow minw0">
            <div className="slabel">{t("settingsPage.theme")}</div>
            <div className="muted small">{t("settingsPage.themeHint")}</div>
          </div>
          <div className="seg">
            {THEMES.map((m) => (
              <button key={m.v} className={prefs.theme === m.v ? "on" : ""} onClick={() => setPrefs({ ...prefs, theme: m.v })}>{m.icon}{t(m.label)}</button>
            ))}
          </div>
        </div>
        <div className="srow">
          <div className="grow minw0">
            <div className="slabel">{t("settingsPage.language")}</div>
            <div className="muted small">{t("settingsPage.languageHint")}</div>
          </div>
          <div className="seg">
            <button className={prefs.lang === "auto" ? "on" : ""} onClick={() => setPrefs({ ...prefs, lang: "auto" })}>{t("settingsPage.langAuto")}</button>
            {LANGS.map((l) => (
              <button key={l.id} lang={l.id} className={prefs.lang === l.id ? "on" : ""} onClick={() => setPrefs({ ...prefs, lang: l.id })}>{l.label}</button>
            ))}
          </div>
        </div>
        <div className="srow">
          <div className="grow minw0">
            <div className="slabel">{t("settingsPage.motion")}</div>
            <div className="muted small">{t("settingsPage.motionHint", { hint: t(MOTION.find((m) => m.v === prefs.motion)?.hint ?? "settingsPage.motionFullHint") })}</div>
          </div>
          <div className="seg">
            {MOTION.map((m) => (
              <button key={m.v} className={prefs.motion === m.v ? "on" : ""} title={t(m.hint)} onClick={() => setPrefs({ ...prefs, motion: m.v })}>{t(m.label)}</button>
            ))}
          </div>
        </div>
        <div className="srow">
          <div className="grow minw0">
            <div className="slabel">{t("settingsPage.autoLatency")}</div>
            <div className="muted small">{t("settingsPage.autoLatencyHint")}</div>
          </div>
          <button className={`switch${prefs.autoLatency ? " on" : ""}`} role="switch" aria-checked={prefs.autoLatency} aria-label={t("settingsPage.autoLatency")}
            onClick={() => setPrefs({ ...prefs, autoLatency: !prefs.autoLatency })}><span /></button>
        </div>
        <div className="srow">
          <div className="grow minw0">
            <div className="slabel">{t("settingsPage.restartProgress")}</div>
            <div className="muted small">{t("settingsPage.restartProgressHint", { hint: t(RESTART_PROGRESS.find((m) => m.v === prefs.restartProgress)?.hint ?? "settingsPage.restartDialogHint") })}</div>
          </div>
          <div className="seg">
            {RESTART_PROGRESS.map((m) => (
              <button key={m.v} className={prefs.restartProgress === m.v ? "on" : ""} onClick={() => setPrefs({ ...prefs, restartProgress: m.v })}>{t(m.label)}</button>
            ))}
          </div>
        </div>
      </section>

      <section className="sgroup">
        <h2>{t("settingsPage.dataTitle")}</h2>
        <div className="srow">
          <div className="grow minw0">
            <div className="slabel">{t("settingsPage.dataDir")}</div>
            <div className="muted small mono">{t("settingsPage.dataDirHint")}</div>
          </div>
          <button className="btn" onClick={() => api.openDataDir().catch((e) => flash(String(e), true))}><Icon.folder />{t("common.open")}</button>
          <button className="btn" onClick={onHistory}><Icon.history size={14} />{t("settingsPage.backups")}</button>
        </div>
      </section>

      <section className="sgroup">
        <h2>{t("settingsPage.aboutTitle")}</h2>
        <div className="srow">
          <div className="grow minw0">
            <div className="slabel">AgentPlus 0.1.0</div>
            <div className="muted small">{t("settingsPage.aboutHint")}</div>
          </div>
        </div>
      </section>
    </div>
  );
}

function Detection({ envLabel, onChanged, flash, prefs, setPrefs }: {
  envLabel: string; onChanged: () => void; flash: (t: string, e?: boolean) => void; prefs: Prefs; setPrefs: (p: Prefs) => void;
}) {
  const hidden = new Set(prefs.hiddenAgents);
  const toggleShown = (id: string) => {
    const next = new Set(hidden);
    if (next.has(id)) next.delete(id); else next.add(id);
    setPrefs({ ...prefs, hiddenAgents: [...next] });
  };
  const [list, setList] = useState<AgentDetect[] | null>(null);
  const [edit, setEdit] = useState<Record<string, string>>({});
  const [saving, setSaving] = useState<string | null>(null);
  const [showAll, setShowAll] = useState(false);

  // Shown in the sidebar first, then detected (hidden, or detect-only), then not found.
  // Sorted when loaded, so flipping a switch doesn't make the card jump away.
  const rank = (d: AgentDetect) => (d.enabled && !hidden.has(d.id) ? 0 : d.enabled || d.manual ? 1 : 2);
  const load = () => api.detectAgents()
    .then((l) => setList(l.map((d, i) => [d, i] as const).sort(([a, i], [b, j]) => rank(a) - rank(b) || i - j).map(([d]) => d)))
    .catch((e) => flash(String(e), true));
  const lang = useLang();
  useEffect(() => { load(); }, [envLabel, lang]);

  const save = async (id: AgentId, path: string | null) => {
    setSaving(id);
    try {
      await api.setAgentDir(id, path);
      setEdit((m) => { const n = { ...m }; delete n[id]; return n; });
      await load();
      onChanged();
      flash(t(path ? "settingsPage.dirUsed" : "settingsPage.dirReset"));
    } catch (e) {
      flash(String(e), true);
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
          <button className="btn small" onClick={() => { setList(null); load(); }}><Icon.refresh size={12} />{t("settingsPage.redetect")}</button>
        </span>
      </div>
      {showAll && <SupportedAgents found={new Set(list?.filter((d) => d.enabled || d.manual).map((d) => d.id))} onClose={() => setShowAll(false)} />}
      {!list && <div className="empty">{t("settingsPage.detecting")}</div>}
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
                <button type="button" className={`switch${hidden.has(d.id) ? "" : " on"}`} role="switch" aria-checked={!hidden.has(d.id)} aria-label={t("settingsPage.showInSidebarAria", { name: d.name })}
                  onClick={() => toggleShown(d.id)}><span /></button>
              </label>
            )}
            {d.note && <div className="detect-note tiny">{d.note}</div>}
            {!d.manual && <div className="srow stacked">
              <div className="row gap6">
                <span className="small strong">{t("settingsPage.configDir")}</span>
                <span className={`ptag ${d.customDir ? "tag-new" : "tag-soft"}`}>{t(d.customDir ? "settingsPage.dirCustom" : "settingsPage.dirDefault")}</span>
                <span className="grow" />
                <button className="link" onClick={() => api.openPath(d.configDir).catch((e) => flash(String(e), true))}>{t("common.open")}</button>
              </div>
              {draft === undefined ? (
                <div className="row gap6">
                  <span className="mono small grow ellipsis dir-line" title={d.configDir}>{d.configDir}</span>
                  <button className="btn small" onClick={() => setEdit((m) => ({ ...m, [d.id]: d.customDir ?? d.configDir }))}><Icon.edit size={12} />{t("settingsPage.change")}</button>
                  {d.customDir && <button className="btn small" disabled={saving === d.id} onClick={() => save(d.id, null)}>{t("settingsPage.restoreDefault")}</button>}
                </div>
              ) : (
                <div className="row gap6">
                  <input className="input mono grow" autoFocus value={draft} placeholder={d.defaultDir}
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
  useEscape(onClose);
  return (
    <div className="modal-bg" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="modal wide" role="dialog" aria-modal="true" aria-label={t("settingsPage.supportedAgents")}>
        <div className="modal-head">
          <h2>{t("settingsPage.supportedAgents")} <span className="muted small">{t("settingsPage.supportedCount", { n: SUPPORTED.length })}</span></h2>
          <button className="icon-btn" aria-label={t("common.close")} onClick={onClose}><Icon.close /></button>
        </div>
        <div className="modal-body">
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
        </div>
      </div>
    </div>
  );
}
