import { Fragment, type ReactNode, useEffect, useRef, useState } from "react";
import { type AgentId, type AgentState, type Issue, type Model, type ModelField, type ModelFieldValue, type ModelGuess, type ModelInput, type ModelTag, type Setting, type SettingValue, api, isProjectId } from "../api";
import {
  CATALOG, type Draft, type ViewModel, type ViewProvider, currentProvider, defaultModel, deleteModel, discardNewModel, editProvider, guessedModel, hasDefaultModel, isEnabled, isVisible, issueStaged, keys, mergeExtra, opCount,
  pluginOn, providerModelCount, setModelVisible, setPluginEnabled, setSetting, setSettingIn, settingValue, excludedOn, syncKeys, upsertModel, viewModels, viewProviders, visibleCount, visibleModelCount, withOp,
} from "../draft";
import { AgentIcon, Icon, OptCheck } from "./icons";
import { MaintenanceTab } from "./MaintenanceTab";
import { type Latency, ProviderCard, testUrl } from "./ProviderCard";
import { SessionsTab } from "./SessionsTab";
import { TabBar, useSlideDir } from "./TabBar";
import { OfficialFetch } from "./OfficialFetch";
import { ask } from "./Confirm";
import { ComboBox } from "./ComboBox";
import { Dropdown } from "./Dropdown";
import { ModelDialog } from "./ModelDialog";
import { Switch } from "./controls";
import { t, tn } from "../i18n";
import { scrub } from "../privacy";
import { errText, type Flash, toggled, toggledIn } from "../util";

export type Tab = "prov" | "models" | "mcp" | "skills" | "plugins" | "sessions" | "maint" | "projects" | "set";

interface Props {
  st: AgentState;
  draft: Draft;
  /** A function of the current draft after an `await` (see App's `setDraftFor`). */
  setDraft: (d: Draft | ((cur: Draft) => Draft)) => void;
  latency: Record<string, Latency>;
  onTestAll: () => void;
  onTestOne: (url: string) => void;
  restarting: boolean;
  /** Another agent is restarting (one restart runs at a time). */
  restartBlocked: boolean;
  onRestart: () => void;
  /** Restart without Codex's UI patches (quicker; for the official-list fetch). */
  onRestartPlain: () => void;
  onOpenDir: () => void;
  tab: Tab;
  setTab: (t: Tab) => void;
  railSel: string | null;
  setRailSel: (id: string | null) => void;
  selectedProvider: string | null;
  onSelectProvider: (id: string) => void;
  onProviderAction: (p: ViewProvider) => void;
  onAddProvider: () => void;
  flash: Flash;
  sessionQuery?: string;
  /** Codex: stop turning the fixed id on by default. */
  onDeclineFixed: () => void;
  /** Re-read this agent from disk. */
  onReload: () => void;
  /** Re-read this agent from disk on request (its icon), saying when it is done. */
  onRefresh: () => Promise<void>;
  /** Opens a provider's edit dialog (to add a missing key). */
  onEditProvider: (p: ViewProvider) => void;
  /** Codex without a catalog: create one from the model list built into Codex. */
  onCreateCatalog: () => Promise<void>;
  /** Replaces the icon / name / buttons row (project pages). */
  head?: ReactNode;
  /** Shows a "copy a provider from elsewhere" card next to "add". */
  onCopyProvider?: () => void;
  /** OpenCode: body of the Projects tab (per-folder configs) and how many projects there are. */
  projectsTab?: { body: ReactNode; count: number };
  /** Sections after the settings read from the agent's config (AgentPlus's own, saved right away). */
  settingsExtra?: ReactNode;
  /** Body of the MCP tab; agents without MCP support have none. */
  mcpTab?: ReactNode;
  /** Body of the Skills tab; agents without skills have none. */
  skillsTab?: ReactNode;
  /** Whether the Plugins tab is shown (Settings → Interface). */
  showPlugins: boolean;
}

export function AgentPage(props: Props) {
  const { st, draft, setDraft, latency, onTestAll, restarting, onRestart, onOpenDir, tab, setTab, railSel, setRailSel } = props;
  const cur = currentProvider(st, draft);
  const providers = viewProviders(st, draft, isProjectId(st.id));
  const plugins = props.showPlugins ? st.plugins : undefined;

  /** Agents with nothing to set (Hermes) get no settings tab. */
  const hasSettings = st.settings.length > 0 || !!props.settingsExtra;
  const provCount = providers.filter((p) => p.compatible && !p.isDeleted && (p.isNew || isEnabled(p, draft))).length;
  const tabs: [Tab, string, number | null][] = [
    ["prov", t("common.providers"), st.mode === "single" ? providers.filter((p) => p.compatible && !p.isDeleted).length : provCount],
    ["models", t("agentPage.tabModels"), visibleCount(st, draft)],
    ...(props.mcpTab ? ([["mcp", "MCP", null]] as [Tab, string, null][]) : []),
    ...(props.skillsTab ? ([["skills", t("agentPage.tabSkills"), null]] as [Tab, string, null][]) : []),
    ...(plugins ? ([["plugins", t("agentPage.tabPlugins"), plugins.filter((p) => pluginOn(p, draft)).length || null]] as [Tab, string, number | null][]) : []),
    ...(st.id === "codex" ? ([["sessions", t("agentPage.tabSessions"), null], ["maint", t("agentPage.tabMaint"), null]] as [Tab, string, null][]) : []),
    ...(props.projectsTab ? ([["projects", t("agentPage.tabProjects"), props.projectsTab.count || null]] as [Tab, string, number | null][]) : []),
    ...(hasSettings ? ([["set", t("agentPage.tabSettings"), null]] as [Tab, string, null][]) : []),
  ];
  // Coming from another agent's settings tab: this one has none.
  useEffect(() => { if ((tab === "set" && !hasSettings) || (tab === "mcp" && !props.mcpTab) || (tab === "skills" && !props.skillsTab) || (tab === "plugins" && !plugins)) setTab("prov"); }, [tab, hasSettings, !!props.mcpTab, !!props.skillsTab, !!plugins]);
  const slide = useSlideDir(tab, tabs.map((x) => x[0]));
  const project = isProjectId(st.id);
  const official = st.id === "codex" && !st.readonly ? (
    <OfficialFetch pending={opCount(draft)} running={st.running} restartable={st.restartable} restarting={restarting} onRestart={onRestart} onRestartPlain={props.onRestartPlain} onReload={props.onReload} flash={props.flash} />
  ) : null;
  // Sessions should belong to the fixed id when that mode is on, else the configured provider.
  const fixedSetting = st.settings.find((s) => s.key === "fixed_id");
  const fixedOn = fixedSetting?.value === true;
  const sessionTarget = fixedOn ? "agentplus" : st.currentProvider ?? "openai";

  const openWeb = () => {
    api.openWebUi(st.id).then((note) => { if (note) props.flash(note); }).catch((e) => props.flash(errText(e), true));
  };

  const toggleModel = (pid: string, m: Model) => setDraft(setModelVisible(draft, pid, m, !isVisible(pid, m, draft)));
  /** Bumped by a refresh: tabs that load their own data (MCP, skills, sessions…) remount and read it again. */
  const [reads, setReads] = useState(0);
  const refresh = async () => {
    await props.onRefresh();
    setReads((n) => n + 1);
  };

  return (
    <main className="page">
      <div className="page-top">
        {props.head ?? <div className="page-head">
          <RefreshIcon id={st.id} name={st.name} onRefresh={refresh} />
          <div className="page-title">
            <div className="row gap10 minw0 page-title-row">
              <h1 className="ellipsis" title={st.name}>{st.name}</h1>
              {st.installed ? (
                <span className="chip-ok">{t("agentPage.detected", { version: st.version ?? "?" })}{st.running ? t("agentPage.running") : ""}</span>
              ) : (
                <span className="chip-muted">{t("agentPage.notInstalled")}</span>
              )}
            </div>
            <span className="mono muted small ellipsis" title={scrub(st.files.join("\n"))}>{scrub(st.files.join(" · "))}</span>
          </div>
          <button className="btn" onClick={onOpenDir}><Icon.folder />{t("common.openConfigDir")}</button>
          {st.webUi && st.running && (
            <button className="btn" onClick={openWeb} disabled={restarting} title={t("agentPage.openWebTitle", { name: st.name })}><Icon.external />{t("agentPage.openWeb")}</button>
          )}
          {st.restartable && (
            // The page already names the agent: the button says only what it does.
            <button className="btn strong" onClick={onRestart} disabled={!st.installed || restarting || props.restartBlocked} title={t(st.running ? "common.restartAgent" : "common.startAgent", { name: st.name })}>
              {st.running ? <Icon.refresh /> : <Icon.play />}
              {restarting
                ? t(st.running ? "agentPage.restarting" : "agentPage.starting")
                : t(st.running ? "agentPage.restart" : "agentPage.start")}
            </button>
          )}
        </div>}
        {st.notes.length > 0 && (
          <div className="notes">{st.notes.map((n) => <span key={n}>{scrub(n)}</span>)}</div>
        )}
        <TabBar items={tabs.map(([id, label, n]) => ({ id, label, count: n }))} value={tab} onChange={setTab} />
      </div>

      <div className={`page-body slide-${slide}`} key={tab}>
        {tab === "prov" && (
          <div className="stack12">
            <ConfigIssues st={st} draft={draft} setDraft={setDraft} providers={providers} onEditProvider={props.onEditProvider} />
            <div className="row between">
              <span className={`muted small${st.fixedPending && fixedSetting && settingValue(fixedSetting, draft) !== true ? "" : " hint"}`}>
                {st.mode === "single"
                  ? t(st.id === "codex" ? "agentPage.singleNoteCodex" : st.id === "claude" ? "agentPage.singleNoteClaude" : "agentPage.singleNote", { name: st.name })
                  : project ? t("agentPage.projectNote")
                  : t("agentPage.multiNote", { name: st.name })}
                {st.fixedPending && fixedSetting && settingValue(fixedSetting, draft) !== true && (
                  <>
                    {" "}{t("agentPage.fixedOffNote")}
                    <button className="link" onClick={() => setDraft(setSetting(draft, fixedSetting, true))}>{t("agentPage.enableFixed")}</button>
                  </>
                )}
              </span>
              <button className="btn small" onClick={onTestAll}><Icon.pulse />{t("agentPage.testAll")}</button>
            </div>
            <div className="pgrid">
              {providers.map((p) => (
                <ProviderCard
                  key={p.id}
                  p={p}
                  mode={st.mode}
                  isCurrent={st.mode === "single" && cur === p.id}
                  switching={st.currentProvider !== p.id}
                  selected={props.selectedProvider === p.id}
                  enabled={p.isNew || isEnabled(p, draft)}
                  visible={providerModelCount(p, draft)}
                  latency={testUrl(p) ? latency[testUrl(p)!] : undefined}
                  readonly={st.readonly}
                  onSelect={() => props.onSelectProvider(p.id)}
                  onModels={() => { setRailSel(p.id); setTab("models"); }}
                  onAction={() => props.onProviderAction(p)}
                  onTest={() => { const u = testUrl(p); if (u) props.onTestOne(u); }}
                />
              ))}
              <button className="pcard-add" disabled={st.readonly} onClick={props.onAddProvider}><Icon.plus size={18} />{t("common.addProvider")}</button>
              {props.onCopyProvider && (
                <button className="pcard-add" disabled={st.readonly} onClick={props.onCopyProvider}><Icon.copy size={18} />{t("agentPage.copyProvider")}<span className="tiny hint">{t("agentPage.copyProviderHint")}</span></button>
              )}
            </div>
          </div>
        )}

        {tab === "models" && (
          st.catalog ? (
            <div className="stack12">
            <ModelTable
              st={st}
              title={t("agentPage.catalogTitle", { file: st.catalogFile ?? "" })}
              note={t("agentPage.catalogNote")}
              pid={CATALOG}
              fetchFrom={cur && cur !== "openai" ? cur : null}
              models={viewModels(CATALOG, st.catalog, draft)}
              base={st.catalog}
              draft={draft}
              setDraft={setDraft}
              readonly={st.readonly}
              onToggle={toggleModel}
              flash={props.flash}
            />
            {official}
            </div>
          ) : st.id === "codex" ? (
            <div className="stack12">
              <div className="empty stack12">
                <span>{t("agentPage.noCatalog")}</span>
                {!st.readonly && <CreateCatalog onCreate={props.onCreateCatalog} flash={props.flash} />}
              </div>
              {official}
            </div>
          ) : (
            (() => {
              const list = providers.filter((p) => !p.isNew && !p.isDeleted);
              const sel = list.find((p) => p.id === railSel) ?? list[0];
              if (!sel) return <div className="empty">{t("agentPage.noProviders")}</div>;
              const enabled = isEnabled(sel, draft);
              const inherited = project && !sel.editable && !sel.builtin;
              return (
                <div className="models-split">
                  <div className="rail">
                    <span className="side-label">{t("common.providers")}</span>
                    {list.map((p) => (
                      <button key={p.id} className={`rail-item${p.id === sel.id ? " on" : ""}`} onClick={() => setRailSel(p.id)}>
                        <span className="dot" style={{ background: isEnabled(p, draft) ? "var(--ok-dot)" : "var(--faint2)" }} />
                        <span className="ellipsis grow">{p.name}</span>
                        <span className="mono muted tiny">{visibleModelCount(p.id, p.models, draft)}/{p.models.length}</span>
                      </button>
                    ))}
                  </div>
                  <ModelTable
                    // One table per provider: a model list still being fetched for the last
                    // one must not land here (and be added under this provider).
                    key={sel.id}
                    st={st}
                    title={sel.name}
                    note={sel.builtin ? t("agentPage.builtinNote", { name: st.name }) : inherited ? t("agentPage.inheritedNote") : enabled ? t("agentPage.enabledNote") : t("agentPage.disabledNote")}
                    pid={sel.id}
                    fetchFrom={sel.builtin || inherited || !sel.baseUrl ? null : sel.id}
                    models={viewModels(sel.id, sel.models, draft)}
                    base={sel.models}
                    draft={draft}
                    setDraft={setDraft}
                    readonly={st.readonly || !enabled || sel.builtin || inherited}
                    onToggle={toggleModel}
                    flash={props.flash}
                  />
                </div>
              );
            })()
          )
        )}

        {/* Tabs that load their own data: a refresh remounts them so they read it again. */}
        {tab === "sessions" && <SessionsTab key={reads} target={sessionTarget} flash={props.flash} initialQuery={props.sessionQuery} />}
        {tab === "maint" && <MaintenanceTab key={reads} flash={props.flash} />}
        {tab === "projects" && <Fragment key={reads}>{props.projectsTab?.body}</Fragment>}
        {tab === "mcp" && <Fragment key={reads}>{props.mcpTab}</Fragment>}
        {tab === "skills" && <Fragment key={reads}>{props.skillsTab}</Fragment>}
        {tab === "plugins" && plugins && (
          <div className="stack12">
            <span className="muted small hint">{t("agentPage.pluginsHint", { name: st.name })}</span>
            {plugins.length === 0 && <div className="empty">{t("agentPage.pluginsEmpty", { name: st.name })}</div>}
            {plugins.length > 0 && (
              <div className="stable">
                {plugins.map((p) => {
                  const on = pluginOn(p, draft);
                  const pending = on !== p.enabled;
                  return (
                    <div key={p.id} className={`hrow mcp-row${on ? "" : " off"}`}>
                      <div className="minw0">
                        <div className="row gap6">
                          <span className="strong small">{p.name}</span>
                          {p.version && <span className="tiny muted mono">{p.version}</span>}
                          {p.source && <span className="ptag tag-soft">{p.source}</span>}
                          {pending && <span className="ptag tag-new">{t("agentPage.pluginPending")}</span>}
                        </div>
                        <div className="tiny muted ellipsis" title={p.description ?? undefined}>{p.description || (p.name === p.id ? "" : p.id)}</div>
                      </div>
                      <span title={p.locked ?? undefined}>
                        <Switch on={on} disabled={st.readonly || !!p.locked} onChange={(v) => setDraft(setPluginEnabled(draft, p, v))} label={t("agentPage.pluginToggle", { name: p.name })} />
                      </span>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        )}

        {tab === "set" && hasSettings && (() => {
          // Fixed id is pre-enabled: shown as on, written together with the next provider switch.
          const fixedPre = st.fixedPrompt && !draft[keys.setting("fixed_id")];
          const shown = fixedPre ? st.settings.map((s) => (s.key === "fixed_id" ? { ...s, value: true } : s)) : st.settings;
          return (
            <Settings settings={shown} draft={draft} readonly={st.readonly}
              onChange={async (s, v) => {
                if (fixedPre && s.key === "fixed_id") return props.onDeclineFixed();
                // Mutually exclusive switches: confirm only when the other one is on.
                if (v === true) {
                  for (const o of excludedOn(draft, st.settings, s)) {
                    const ok = await ask({ title: t("agentPage.exclusiveConfirm", { other: o.label }), message: t("agentPage.exclusiveConfirmMsg", { name: s.label, other: o.label }), confirmText: t("agentPage.exclusiveTurnOn") });
                    if (!ok) return;
                  }
                }
                setDraft((d) => setSettingIn(d, st.settings, s, v));
              }}
              notes={fixedPre ? {
                fixed_id: <div className="set-note">{t("agentPage.fixedPreNote")}</div>,
              } : undefined}
              extra={props.settingsExtra} />
          );
        })()}
      </div>
    </main>
  );
}

/** The agent's icon; hovering shows a faint refresh mark, and a click re-reads its config. */
function RefreshIcon({ id, name, onRefresh }: { id: AgentId; name: string; onRefresh: () => Promise<void> }) {
  const [busy, setBusy] = useState(false);
  const click = async () => {
    if (busy) return;
    setBusy(true);
    // Reading is quick: keep the mark turning long enough to be seen.
    await Promise.all([onRefresh().catch(() => undefined), new Promise((r) => window.setTimeout(r, 450))]);
    setBusy(false);
  };
  const label = t("agentPage.reloadTitle", { name });
  return (
    <button className={`agent-reload${busy ? " busy" : ""}`} onClick={click} title={label} aria-label={label} aria-busy={busy}>
      <AgentIcon id={id} size={46} />
      <span className="agent-reload-mark"><Icon.refresh size={20} sw={2.4} /></span>
    </button>
  );
}

/** What in the agent's config differs from how AgentPlus writes it, and what fixes each. */
function ConfigIssues({ st, draft, setDraft, providers, onEditProvider }: {
  st: AgentState; draft: Draft; setDraft: (d: Draft) => void; providers: ViewProvider[]; onEditProvider: (p: ViewProvider) => void;
}) {
  const [open, setOpen] = useState(true);
  const issues = st.issues ?? [];
  if (!issues.length) return null;
  const movable = issues.filter((i) => i.kind === "key-elsewhere");
  const synced = movable.every((i) => issueStaged(draft, i));
  const action = (i: Issue) => {
    if (issueStaged(draft, i)) return <span className="ptag tag-new">{t("agentPage.issuePending")}</span>;
    const p = providers.find((x) => x.id === i.provider);
    const base = st.providers.find((x) => x.id === i.provider);
    if (!p || !base || st.readonly) return null;
    if (i.kind === "key-missing") {
      return <button className="btn small" onClick={() => onEditProvider(p)}><Icon.key size={12} />{t("agentPage.issueAddKey")}</button>;
    }
    if (i.kind === "api-key-sign-in") {
      return <button className="btn small" onClick={() => setDraft(editProvider(draft, base, { officialAuth: false }))}>{t("agentPage.issueMixOff")}</button>;
    }
    return null;
  };
  return (
    <section className="issues">
      <div className="issues-head">
        <Icon.warn size={14} />
        <span className="strong small minw0">{tn("agentPage.issuesHead", issues.length, { name: st.name })}</span>
        {movable.length > 0 && (
          <button className="btn small primary" disabled={st.readonly || synced} title={t("agentPage.issuesSyncTitle")} onClick={() => setDraft(syncKeys(draft, st))}>
            {synced ? t("agentPage.issuesSynced") : t("agentPage.issuesSync")}
          </button>
        )}
        <button className="link tiny" aria-expanded={open} onClick={() => setOpen(!open)}>{open ? t("agentPage.collapse") : t("agentPage.issuesExpand")}</button>
      </div>
      {open && (
        <ul className="issues-list">
          {issues.map((i, n) => (
            <li key={`${i.kind}:${i.provider ?? ""}:${n}`}>
              <span className="small minw0">{scrub(i.text)}</span>
              {action(i)}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

/** Creates Codex's catalog from the model list built into Codex. */
function CreateCatalog({ onCreate, flash }: { onCreate: () => Promise<void>; flash: Flash }) {
  const [busy, setBusy] = useState(false);
  const create = async () => {
    setBusy(true);
    try {
      await onCreate();
    } catch (e) {
      flash(errText(e), true);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div>
      <button className="btn primary" disabled={busy} onClick={create}>
        <Icon.layers size={13} />{busy ? t("agentPage.creatingCatalog") : t("agentPage.useBuiltinCatalog")}
      </button>
    </div>
  );
}

// ---------------------------------------------------------------- model table

interface ModelTableProps {
  st: AgentState;
  title: string;
  note: string;
  pid: string;
  /** Provider id to fetch the model list from (null = can't fetch). */
  fetchFrom: string | null;
  models: ViewModel[];
  /** The same models as read from disk (to tell a real change from an undo). */
  base: Model[];
  draft: Draft;
  setDraft: (d: Draft | ((cur: Draft) => Draft)) => void;
  readonly: boolean;
  onToggle: (pid: string, m: Model) => void;
  flash: Flash;
}

/** Backend capability tags (`cap:*`), replaced by the ones below (they would lag behind pending edits). */
const isCapTag = (g: ModelTag) => g.id.startsWith("cap:");

/** A raw config value as a tag ("image_x" → "Image_x"). */
const rawTag = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);

/** Short tags for what a model can read / do, from its field values (pending edits included). */
function capTags(fields: ModelField[], extra: Model["extra"]): string[] {
  const out: string[] = [];
  for (const f of fields) {
    const v = extra?.[f.key];
    if (f.kind === "chips" && Array.isArray(v)) {
      for (const o of v) {
        if (o === "text") continue;
        const i = f.options.indexOf(o);
        out.push((i >= 0 && (f.caps[i] || f.hints[i])) || rawTag(o));
      }
    } else if (f.kind === "bool" && v === true) {
      if (/reasoning$/i.test(f.key)) out.push(t("agentPage.capReasoning"));
      else if (f.gid === "io" && f.caps[0]) out.push(f.caps[0]);
    }
  }
  return [...new Set(out)];
}

const sameVal = (a: ModelFieldValue | undefined, b: ModelFieldValue | undefined) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);

function ModelTable({ st, title, note, pid, fetchFrom, models, base, draft, setDraft, readonly, onToggle, flash }: ModelTableProps) {
  const hasNames = st.id !== "zcode"; // ZCode has no per-model display name
  const hasContext = st.id !== "droid"; // Droid has no context-window field
  const fields = st.modelFields ?? [];
  const [editing, setEditing] = useState<string | null>(null); // model id, or "__new"
  const [fetched, setFetched] = useState<string[] | null>(null);
  const [fetching, setFetching] = useState(false);
  const [pick, setPick] = useState<Set<string>>(new Set());
  const [filter, setFilter] = useState("");

  useEffect(() => { setEditing(null); setFetched(null); setFilter(""); }, [pid]);
  // Where a fetch was started from vs. where the table is now (the provider can change meanwhile).
  const source = useRef({ pid, fetchFrom });
  source.current = { pid, fetchFrom };

  const existing = new Set(models.map((m) => m.id));
  const doFetch = async () => {
    if (!fetchFrom) return;
    const from = { pid, fetchFrom };
    setFetching(true);
    try {
      const list = await api.fetchModels(st.id, fetchFrom);
      if (source.current.pid !== from.pid || source.current.fetchFrom !== from.fetchFrom) return;
      const fresh = list.filter((m) => !existing.has(m));
      setFetched(fresh);
      setPick(new Set());
      if (fresh.length === 0) flash(tn("agentPage.allListed", list.length));
    } catch (e) {
      flash(t("common.fetchFailed", { err: errText(e) }), true);
    } finally {
      setFetching(false);
    }
  };

  // The draft as of now, for code that continues after an await.
  const latest = useRef(draft);
  latest.current = draft;
  const [adding, setAdding] = useState(false);
  const addPicked = async () => {
    const ids = [...pick];
    setAdding(true);
    // Context window and settings from the model catalogs; unknown models are added bare.
    const guesses = await api.guessModels(st.id, ids).catch(() => ({}) as Record<string, ModelGuess>);
    setAdding(false);
    if (source.current.pid !== pid) return;
    let d = latest.current;
    for (const id of ids) d = upsertModel(d, pid, guessedModel(id, guesses[id]));
    setDraft(d);
    setFetched(null);
    flash(tn("agentPage.addedModels", ids.length));
  };

  /** Puts a dialog save into the draft; an edit that ends up where it started drops the op. */
  const saveModel = (input: ModelInput, m?: ViewModel): boolean => {
    if (!m) {
      if (existing.has(input.id)) { flash(t("agentPage.alreadyListed", { id: input.id }), true); return false; }
      setDraft(upsertModel(draft, pid, { ...input, extra: mergeExtra({}, input.extra) }));
      return true;
    }
    const after = mergeExtra(m.extra, input.extra);
    if (m.isNew) {
      setDraft(upsertModel(draft, pid, { id: m.id, name: input.name, context: input.context, extra: after }));
      return true;
    }
    const orig = base.find((x) => x.id === m.id);
    const was = orig?.extra ?? {};
    const extra: Record<string, ModelFieldValue | null> = {};
    for (const k of new Set([...Object.keys(was), ...Object.keys(after)])) if (!sameVal(was[k], after[k])) extra[k] = after[k] ?? null;
    const name = input.name !== null && input.name !== (orig?.name ?? null) ? input.name : null;
    const context = input.context !== null && input.context !== (orig?.context ?? null) ? input.context : null;
    const changed = name !== null || context !== null || Object.keys(extra).length > 0;
    setDraft(changed ? upsertModel(draft, pid, { id: m.id, name, context, extra }) : withOp(draft, keys.upsertModel(pid, m.id), null));
    return true;
  };

  const editingModel = editing && editing !== "__new" ? models.find((m) => m.id === editing) : undefined;

  // Hermes: the default model's tag follows pending changes (set from the right-click menu).
  const hasDefault = hasDefaultModel(st.id);
  const dflt = defaultModel(draft, pid, base);

  const shown = models.filter((m) => !filter || m.id.toLowerCase().includes(filter.toLowerCase()) || (m.name ?? "").toLowerCase().includes(filter.toLowerCase()));

  return (
    <div className="mtable">
      <div className="mtable-head">
        <div className="grow minw0">
          <div className="strong ellipsis">{title}</div>
          <div className={`muted small${readonly ? "" : " hint"}`}>{note}</div>
        </div>
        <input className="search-input slim" placeholder={t("common.filter")} value={filter} onChange={(e) => setFilter(e.target.value)} />
        <button className="btn small" disabled={readonly || !fetchFrom || fetching} onClick={doFetch} title={fetchFrom ? "" : t("agentPage.noFetchUrl")}>
          <Icon.refresh size={12} />{fetching ? t("common.fetching") : t("agentPage.fetchModels")}
        </button>
        <button className="btn small" disabled={readonly} onClick={() => setEditing("__new")}><Icon.plus size={12} />{t("agentPage.addModel")}</button>
      </div>

      {fetched && fetched.length > 0 && (
        <div className="fetched">
          <div className="row between">
            <span className="small strong">{tn("agentPage.fetchedHead", fetched.length)}</span>
            <span className="row gap6">
              <button className="link tiny" onClick={() => setPick(new Set(pick.size === fetched.length ? [] : fetched))}>{pick.size === fetched.length ? t("common.selectNone") : t("common.selectAll")}</button>
              <button className="btn small" onClick={() => setFetched(null)}>{t("agentPage.collapse")}</button>
              <button className="btn small primary" disabled={pick.size === 0 || adding} onClick={addPicked}>{tn("agentPage.addPicked", pick.size)}</button>
            </span>
          </div>
          <div className="pick-list wide">
            {fetched.map((m) => (
              <label key={m} className="pick" title={m}>
                <input type="checkbox" checked={pick.has(m)} onChange={() => setPick((s) => toggled(s, m))} />
                <span className="mono small">{m}</span>
              </label>
            ))}
          </div>
        </div>
      )}

      <div className="mrow mhead"><span /><span>{t("common.model")}</span><span>{t("agentPage.colContext")}</span><span className="right">{t("agentPage.colShown")}</span></div>
      {(editing === "__new" || editingModel) && (
        <ModelDialog agent={st.id} agentName={st.name} hasNames={hasNames} nameIsUpstream={st.id === "kimi"} hasContext={hasContext} fields={fields} initial={editingModel}
          onClose={() => setEditing(null)}
          onSave={(input) => { if (saveModel(input, editingModel)) setEditing(null); }} />
      )}
      {shown.map((m) => {
        const on = !m.isDeleted && isVisible(pid, m, draft);
        const dirty = m.isNew || m.isEdited || m.isDeleted || on !== m.visible;
        const editable = !readonly && !m.readonly && !m.isDeleted;
        const caps = capTags(fields, m.extra);
        const tags = (fields.length ? m.tags.filter((g) => !isCapTag(g)) : m.tags).filter((g) => !hasDefault || g.id !== "role:default");
        const isDefault = hasDefault && m.id === dflt.id;
        return (
          <div key={m.id} className={`mrow${m.isDeleted ? " deleted" : ""}`} data-ctx="model" data-pid={pid} data-mid={m.id}
            title={editable ? t("agentPage.dblClickEdit") : undefined}
            onDoubleClick={(e) => { if (editable && !(e.target as HTMLElement).closest("button")) setEditing(m.id); }}>
            <Icon.grip />
            <span className="minw0">
              <span className="row gap6 minw0">
                <span className={`mono ellipsis${on ? "" : " faint"}${dirty ? " dirty" : ""}`}>{m.id}</span>
                {m.isNew && <span className="mtag new">{t("common.tagNew")}</span>}
                {m.isDeleted && <span className="mtag">{t("common.tagDeleting")}</span>}
                {isDefault && <span className={`mtag${dflt.pending ? " new" : ""}`}>{t("agentPage.tagDefault")}</span>}
                {tags.map((g) => <span key={g.id} className={`mtag${g.id === "fast" ? " fast" : ""}`}>{g.label}</span>)}
              </span>
              {((hasNames && m.name && m.name !== m.id) || caps.length > 0) && (
                <span className="row gap6 minw0 msub">
                  {hasNames && m.name && m.name !== m.id && <span className="tiny muted ellipsis">{m.name}</span>}
                  {caps.map((c) => <span key={c} className="mtag cap">{c}</span>)}
                </span>
              )}
            </span>
            <span className="mono muted small">{m.ctx ?? "—"}</span>
            <span className="row gap6 right">
              {m.readonly ? (
                <span className="muted small">{t("agentPage.builtinReadonly")}</span>
              ) : m.isDeleted ? (
                <button className="link tiny" onClick={() => setDraft(withOp(draft, keys.deleteModel(pid, m.id), null))}>{t("common.undoDelete")}</button>
              ) : (
                <>
                  <button className="icon-btn sm" aria-label={t("agentPage.editModel", { id: m.id })} title={t("common.edit")} disabled={readonly} onClick={() => setEditing(m.id)}>
                    <Icon.edit size={12} />
                  </button>
                  {(m.deletable || m.isNew) && (
                    <button className="icon-btn sm" aria-label={t("agentPage.deleteModel", { id: m.id })} title={t("common.delete")} disabled={readonly}
                      onClick={async () => {
                        if (!(await ask({ title: t("agentPage.deleteConfirm", { id: m.id }), message: t(m.isNew ? "agentPage.deleteNewConfirmMsg" : "agentPage.deleteConfirmMsg"), danger: true }))) return;
                        setDraft((d) => (m.isNew ? discardNewModel(d, pid, m.id) : deleteModel(d, pid, m.id)));
                      }}>
                      <Icon.trash size={12} />
                    </button>
                  )}
                  {!m.isNew && <Switch on={on} disabled={readonly} label={t(on ? "agentPage.hideModel" : "agentPage.showModel", { id: m.id })} onChange={() => onToggle(pid, m)} />}
                </>
              )}
            </span>
          </div>
        );
      })}
      <div className="mtable-foot muted small">{filter
        ? tn("agentPage.totalFiltered", models.filter((m) => !m.isDeleted).length, { shown: shown.length })
        : tn("agentPage.total", models.filter((m) => !m.isDeleted).length)}</div>
    </div>
  );
}

function Settings({ settings, draft, readonly, onChange, notes, extra }: {
  settings: Setting[]; draft: Draft; readonly: boolean; onChange: (s: Setting, v: SettingValue) => void;
  /** Extra line under a setting's description, by key. */
  notes?: Record<string, ReactNode>;
  /** Sections appended after the groups. */
  extra?: ReactNode;
}) {
  const groups = [...new Set(settings.map((s) => s.group))];
  return (
    <div className="settings">
      {groups.map((g) => (
        <section key={g} className="sgroup">
          <h2>{g}</h2>
          {settings.filter((s) => s.group === g).map((s) => {
            const v = settingValue(s, draft);
            const dirty = JSON.stringify(v) !== JSON.stringify(s.value);
            const head = (
              <div className="grow minw0">
                <div className="slabel">{s.label}{dirty && <span className="unsaved">{t("agentPage.unapplied")}</span>}</div>
                <div className="muted small hint">{s.desc}</div>
                {notes?.[s.key]}
              </div>
            );
            if (s.kind === "bool") {
              return (
                <div key={s.key} className="srow" id={`setting-${s.key}`}>
                  {head}
                  <Switch on={v === true} disabled={readonly} fast={s.key.startsWith("fast")} label={s.label} onChange={(x) => onChange(s, x)} />
                </div>
              );
            }
            if (s.kind === "select") {
              return (
                <div key={s.key} className="srow" id={`setting-${s.key}`}>
                  {head}
                  <div className="sctl">
                    <Dropdown value={String(v)} label={s.label} disabled={readonly}
                      options={s.options.map((o, i) => ({ value: o, label: s.hints[i] || o || t("common.notSet") }))}
                      onChange={(x) => onChange(s, x)} />
                  </div>
                </div>
              );
            }
            if (s.kind === "text") {
              return (
                <div key={s.key} className="srow" id={`setting-${s.key}`}>
                  {head}
                  <TextSetting value={String(v)} options={s.options} label={s.label} disabled={readonly} onCommit={(x) => onChange(s, x)} />
                </div>
              );
            }
            if (s.kind === "list") {
              return (
                <div key={s.key} className="srow stacked" id={`setting-${s.key}`}>
                  {head}
                  <ListSetting value={v as string[]} label={s.label} disabled={readonly} onCommit={(x) => onChange(s, x)} />
                </div>
              );
            }
            // Multi-select: options as a grid of cards under the title, each with its explanation.
            const arr = v as string[];
            return (
              <div key={s.key} className="srow stacked" id={`setting-${s.key}`}>
                <div className="row between">
                  {head}
                  <span className="tiny muted">{t("agentPage.selectedCount", { n: arr.length, total: s.options.length })}</span>
                </div>
                <div className="opt-grid">
                  {s.options.map((o, i) => {
                    const on = arr.includes(o);
                    return (
                      <button key={o} className={`opt${on ? " on" : ""}`} aria-pressed={on} disabled={readonly}
                        onClick={() => onChange(s, toggledIn(arr, o))}>
                        <OptCheck on={on} />
                        <span className="minw0">
                          <span className="opt-name">{o}</span>
                          {s.hints[i] && <span className="opt-hint hint">{s.hints[i]}</span>}
                        </span>
                      </button>
                    );
                  })}
                </div>
              </div>
            );
          })}
        </section>
      ))}
      {extra}
    </div>
  );
}

/** Free text (with suggestions); goes into the draft when the field loses focus, on Enter, or when a suggestion is picked. */
function TextSetting({ value, options, label, disabled, onCommit }: { value: string; options: string[]; label: string; disabled: boolean; onCommit: (v: string) => void }) {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  const commit = (x = text) => { if (x.trim() !== value) onCommit(x.trim()); };
  return (
    <div className="sctl wide" onBlur={(e) => { if (!e.currentTarget.contains(e.relatedTarget as Node | null)) commit(); }}>
      {options.length ? (
        <ComboBox value={text} options={options} label={label} disabled={disabled} placeholder={t("common.notSet")}
          onChange={(x) => { setText(x); if (options.includes(x)) commit(x); }} onEnter={() => commit()} />
      ) : (
        <input className="input mono" value={text} aria-label={label} disabled={disabled} placeholder={t("common.notSet")}
          onChange={(e) => setText(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") commit(); }} />
      )}
    </div>
  );
}

/** One entry per line; goes into the draft when the field loses focus. */
function ListSetting({ value, label, disabled, onCommit }: { value: string[]; label: string; disabled: boolean; onCommit: (v: string[]) => void }) {
  const joined = value.join("\n");
  const [text, setText] = useState(joined);
  useEffect(() => setText(joined), [joined]);
  const lines = (s: string) => s.split("\n").map((x) => x.trim()).filter(Boolean);
  return (
    <textarea className="input mono slist" rows={Math.min(8, Math.max(2, value.length + 1))} value={text} aria-label={label} disabled={disabled}
      placeholder={t("agentPage.onePerLine")} onChange={(e) => setText(e.target.value)}
      onBlur={() => { const next = lines(text); if (next.join("\n") !== joined) onCommit(next); }} />
  );
}
