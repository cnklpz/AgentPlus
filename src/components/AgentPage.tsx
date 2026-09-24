import { type ReactNode, useEffect, useRef, useState } from "react";
import { type AgentState, type Model, type ModelField, type ModelFieldValue, type ModelInput, type ModelTag, type Setting, type SettingValue, api, isProjectId } from "../api";
import {
  CATALOG, type Draft, type ViewModel, type ViewProvider, currentProvider, deleteModel, isEnabled, isVisible, keys, mergeExtra, opCount,
  providerModelCount, setModelVisible, setSetting, settingValue, upsertModel, viewModels, viewProviders, visibleCount, visibleModelCount, withOp,
} from "../draft";
import { AgentIcon, Icon, OptCheck } from "./icons";
import { MaintenanceTab } from "./MaintenanceTab";
import { type Latency, ProviderCard } from "./ProviderCard";
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

export type Tab = "prov" | "models" | "sessions" | "maint" | "projects" | "set";

interface Props {
  st: AgentState;
  draft: Draft;
  setDraft: (d: Draft) => void;
  latency: Record<string, Latency>;
  onTestAll: () => void;
  onTestOne: (url: string) => void;
  restarting: boolean;
  onRestart: () => void;
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
  /** Replaces the icon / name / buttons row (project pages). */
  head?: ReactNode;
  /** Shows a "copy a provider from elsewhere" card next to "add". */
  onCopyProvider?: () => void;
  /** OpenCode: body of the 项目 tab (per-folder configs) and how many projects there are. */
  projectsTab?: { body: ReactNode; count: number };
}

export function AgentPage(props: Props) {
  const { st, draft, setDraft, latency, onTestAll, restarting, onRestart, onOpenDir, tab, setTab, railSel, setRailSel } = props;
  const cur = currentProvider(st, draft);
  const providers = viewProviders(st, draft, isProjectId(st.id));

  const provCount = providers.filter((p) => p.compatible && !p.isDeleted && (p.isNew || isEnabled(p, draft))).length;
  const tabs: [Tab, string, number | null][] = [
    ["prov", t("common.providers"), st.mode === "single" ? providers.filter((p) => p.compatible && !p.isDeleted).length : provCount],
    ["models", t("agentPage.tabModels"), visibleCount(st, draft)],
    ...(st.id === "codex" ? ([["sessions", t("agentPage.tabSessions"), null], ["maint", t("agentPage.tabMaint"), null]] as [Tab, string, null][]) : []),
    ...(props.projectsTab ? ([["projects", t("agentPage.tabProjects"), props.projectsTab.count || null]] as [Tab, string, number | null][]) : []),
    ["set", t("agentPage.tabSettings"), null],
  ];
  const slide = useSlideDir(tab, tabs.map((x) => x[0]));
  const project = isProjectId(st.id);
  const official = st.id === "codex" && !st.readonly ? (
    <OfficialFetch pending={opCount(draft)} running={st.running} restartable={st.restartable} restarting={restarting} onRestart={onRestart} onReload={props.onReload} flash={props.flash} />
  ) : null;
  // Sessions should belong to the fixed id when that mode is on, else the configured provider.
  const fixedSetting = st.settings.find((s) => s.key === "fixed_id");
  const fixedOn = fixedSetting?.value === true;
  const sessionTarget = fixedOn ? "agentplus" : st.currentProvider ?? "openai";

  const toggleModel = (pid: string, m: Model) => setDraft(setModelVisible(draft, pid, m, !isVisible(pid, m, draft)));

  return (
    <main className="page">
      <div className="page-top">
        {props.head ?? <div className="page-head">
          <AgentIcon id={st.id} size={46} />
          <div className="page-title">
            <div className="row gap10">
              <h1>{st.name}</h1>
              {st.installed ? (
                <span className="chip-ok">{t("agentPage.detected", { version: st.version ?? "?" })}{st.running ? t("agentPage.running") : ""}</span>
              ) : (
                <span className="chip-muted">{t("agentPage.notInstalled")}</span>
              )}
            </div>
            <span className="mono muted small ellipsis">{scrub(st.files.join(" · "))}</span>
          </div>
          <button className="btn" onClick={onOpenDir}><Icon.folder />{t("common.openConfigDir")}</button>
          {st.restartable && (
            <button className="btn strong" onClick={onRestart} disabled={!st.installed || restarting}>
              {st.running ? <Icon.refresh /> : <Icon.play />}
              {restarting
                ? t(st.running ? "agentPage.restarting" : "agentPage.starting")
                : t(st.running ? "common.restartAgent" : "common.startAgent", { name: st.name })}
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
            <div className="row between">
              <span className="muted small">
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
                  latency={p.baseUrl ? latency[p.baseUrl] : undefined}
                  readonly={st.readonly}
                  onSelect={() => props.onSelectProvider(p.id)}
                  onModels={() => { setRailSel(p.id); setTab("models"); }}
                  onAction={() => props.onProviderAction(p)}
                  onTest={() => p.baseUrl && props.onTestOne(p.baseUrl)}
                />
              ))}
              <button className="pcard-add" disabled={st.readonly} onClick={props.onAddProvider}><Icon.plus size={18} />{t("common.addProvider")}</button>
              {props.onCopyProvider && (
                <button className="pcard-add" disabled={st.readonly} onClick={props.onCopyProvider}><Icon.copy size={18} />{t("agentPage.copyProvider")}<span className="tiny">{t("agentPage.copyProviderHint")}</span></button>
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
              <div className="empty">{t("agentPage.noCatalog")}</div>
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

        {tab === "sessions" && <SessionsTab target={sessionTarget} flash={props.flash} initialQuery={props.sessionQuery} />}
        {tab === "maint" && <MaintenanceTab flash={props.flash} />}
        {tab === "projects" && props.projectsTab?.body}

        {tab === "set" && (() => {
          // Fixed id is pre-enabled: shown as on, written together with the next provider switch.
          const fixedPre = st.fixedPrompt && !draft[keys.setting("fixed_id")];
          const shown = fixedPre ? st.settings.map((s) => (s.key === "fixed_id" ? { ...s, value: true } : s)) : st.settings;
          return (
            <Settings settings={shown} draft={draft} readonly={st.readonly}
              onChange={(s, v) => (fixedPre && s.key === "fixed_id" ? props.onDeclineFixed() : setDraft(setSetting(draft, s, v)))}
              notes={fixedPre ? {
                fixed_id: <div className="set-note">{t("agentPage.fixedPreNote")}</div>,
              } : undefined} />
          );
        })()}
      </div>
    </main>
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
  setDraft: (d: Draft) => void;
  readonly: boolean;
  onToggle: (pid: string, m: Model) => void;
  flash: Flash;
}

/** Backend capability tags (`cap:*`), replaced by the ones below (they would lag behind pending edits). */
const isCapTag = (g: ModelTag) => g.id.startsWith("cap:");

/** "读取图片" / "Read images" → "图片" / "Images". */
const capName = (s: string) => {
  const r = s.replace(/^(读取|Read)\s*/i, "");
  return r.charAt(0).toUpperCase() + r.slice(1);
};

/** Short tags for what a model can read / do, from its field values (pending edits included). */
function capTags(fields: ModelField[], extra: Model["extra"]): string[] {
  const out: string[] = [];
  for (const f of fields) {
    const v = extra?.[f.key];
    if (f.kind === "chips" && Array.isArray(v)) {
      for (const o of v) {
        if (o === "text") continue;
        const i = f.options.indexOf(o);
        out.push(capName(i >= 0 ? f.hints[i] || o : o));
      }
    } else if (f.kind === "bool" && v === true) {
      if (/reasoning$/i.test(f.key)) out.push(t("agentPage.capReasoning"));
      else if (f.gid === "io") out.push(capName(f.label));
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

  const addPicked = () => {
    let d = draft;
    for (const id of pick) d = upsertModel(d, pid, { id, name: null, context: null });
    setDraft(d);
    setFetched(null);
    flash(tn("agentPage.addedModels", pick.size));
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

  const shown = models.filter((m) => !filter || m.id.toLowerCase().includes(filter.toLowerCase()) || (m.name ?? "").toLowerCase().includes(filter.toLowerCase()));

  return (
    <div className="mtable">
      <div className="mtable-head">
        <div className="grow minw0">
          <div className="strong ellipsis">{title}</div>
          <div className="muted small">{note}</div>
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
              <button className="btn small primary" disabled={pick.size === 0} onClick={addPicked}>{tn("agentPage.addPicked", pick.size)}</button>
            </span>
          </div>
          <div className="pick-list wide">
            {fetched.map((m) => (
              <label key={m} className="pick">
                <input type="checkbox" checked={pick.has(m)} onChange={() => setPick((s) => toggled(s, m))} />
                <span className="mono small">{m}</span>
              </label>
            ))}
          </div>
        </div>
      )}

      <div className="mrow mhead"><span /><span>{t("common.model")}</span><span>{t("agentPage.colContext")}</span><span className="right">{t("agentPage.colShown")}</span></div>
      {(editing === "__new" || editingModel) && (
        <ModelDialog agentName={st.name} hasNames={hasNames} hasContext={hasContext} fields={fields} initial={editingModel}
          onClose={() => setEditing(null)}
          onSave={(input) => { if (saveModel(input, editingModel)) setEditing(null); }} />
      )}
      {shown.map((m) => {
        const on = !m.isDeleted && isVisible(pid, m, draft);
        const dirty = m.isNew || m.isEdited || m.isDeleted || on !== m.visible;
        const editable = !readonly && !m.readonly && !m.isDeleted;
        const caps = capTags(fields, m.extra);
        const tags = fields.length ? m.tags.filter((g) => !isCapTag(g)) : m.tags;
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
                        if (m.isNew) return setDraft(withOp(draft, keys.upsertModel(pid, m.id), null));
                        if (!(await ask({ title: t("agentPage.deleteConfirm", { id: m.id }), message: t("agentPage.deleteConfirmMsg"), danger: true }))) return;
                        setDraft(deleteModel(draft, pid, m.id));
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

function Settings({ settings, draft, readonly, onChange, notes }: {
  settings: Setting[]; draft: Draft; readonly: boolean; onChange: (s: Setting, v: SettingValue) => void;
  /** Extra line under a setting's description, by key. */
  notes?: Record<string, ReactNode>;
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
                <div className="muted small">{s.desc}</div>
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
                          {s.hints[i] && <span className="opt-hint">{s.hints[i]}</span>}
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
