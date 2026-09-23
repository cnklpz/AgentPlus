import { type ReactNode, useEffect, useState } from "react";
import { type AgentState, type Model, type Setting, api } from "../api";
import {
  CATALOG, type Draft, type ViewModel, type ViewProvider, currentProvider, isEnabled, isVisible, keys, parseCtx,
  setSetting, settingValue, upsertModel, viewModels, viewProviders, visibleCount, withOp,
} from "../draft";
import { AgentIcon, Icon } from "./icons";
import { MaintenanceTab } from "./MaintenanceTab";
import { type Latency, ProviderCard } from "./ProviderCard";
import { SessionsTab } from "./SessionsTab";

export type Tab = "prov" | "models" | "sessions" | "maint" | "set";

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
  flash: (text: string, error?: boolean) => void;
  sessionQuery?: string;
  /** Codex: stop turning the fixed id on by default. */
  onDeclineFixed: () => void;
}

export function AgentPage(props: Props) {
  const { st, draft, setDraft, latency, onTestAll, restarting, onRestart, onOpenDir, tab, setTab, railSel, setRailSel } = props;
  const cur = currentProvider(st, draft);
  const providers = viewProviders(st, draft);

  const provCount = providers.filter((p) => p.compatible && !p.isDeleted && (p.isNew || isEnabled(p, draft))).length;
  const tabs: [Tab, string, number | null][] = [
    ["prov", "供应商", st.mode === "single" ? providers.filter((p) => p.compatible && !p.isDeleted).length : provCount],
    ["models", "模型列表", visibleCount(st, draft)],
    ...(st.id === "codex" ? ([["sessions", "会话", null], ["maint", "维护", null]] as [Tab, string, null][]) : []),
    ["set", "其他设置", null],
  ];
  // Sessions should belong to the fixed id when that mode is on, else the configured provider.
  const fixedSetting = st.settings.find((s) => s.key === "fixed_id");
  const fixedOn = fixedSetting?.value === true;
  const sessionTarget = fixedOn ? "agentplus" : st.currentProvider ?? "openai";

  const toggleModel = (pid: string, m: Model) => {
    const next = !isVisible(pid, m, draft);
    setDraft(withOp(draft, keys.visible(pid, m.id), next === m.visible ? null : { op: "set_model_visible", provider: pid, model: m.id, visible: next }));
  };

  return (
    <main className="page">
      <div className="page-top">
        <div className="page-head">
          <AgentIcon id={st.id} size={46} />
          <div className="page-title">
            <div className="row gap10">
              <h1>{st.name}</h1>
              {st.installed ? (
                <span className="chip-ok">已检测 · {st.version ?? "?"}{st.running ? " · 运行中" : ""}</span>
              ) : (
                <span className="chip-muted">未检测到安装</span>
              )}
            </div>
            <span className="mono muted small ellipsis">{st.files.join(" · ")}</span>
          </div>
          <button className="btn" onClick={onOpenDir}><Icon.folder />打开配置目录</button>
          <button className="btn strong" onClick={onRestart} disabled={!st.installed || restarting}>
            <Icon.refresh />{restarting ? "正在重启…" : `重启 ${st.name}`}
          </button>
        </div>
        {st.notes.length > 0 && (
          <div className="notes">{st.notes.map((n) => <span key={n}>{n}</span>)}</div>
        )}
        <div className="tabs" role="tablist">
          {tabs.map(([id, label, n]) => (
            <button key={id} role="tab" aria-selected={tab === id} className={`tab${tab === id ? " on" : ""}`} onClick={() => setTab(id)}>
              {label}{n !== null && <span className="tab-count">{n}</span>}
            </button>
          ))}
        </div>
      </div>

      <div className="page-body" key={tab}>
        {tab === "prov" && (
          <div className="stack12">
            <div className="row between">
              <span className="muted small">
                {st.mode === "single"
                  ? `${st.name} 同一时间只用一个供应商。只提供 Chat 接口的供应商不能用于 Codex。`
                  : `${st.name} 可以同时启用多个供应商，它们的模型会一起出现在选择器里。`}
                {st.fixedPending && fixedSetting && settingValue(fixedSetting, draft) !== true && (
                  <>
                    {" "}当前没开固定 ID，切换后另一个供应商的会话在 Codex 里可能看不到。
                    <button className="link" onClick={() => setDraft(setSetting(draft, fixedSetting, true))}>开启固定 ID</button>
                  </>
                )}
              </span>
              <button className="btn small" onClick={onTestAll}><Icon.pulse />全部测速</button>
            </div>
            <div className="pgrid">
              {providers.map((p) => (
                <ProviderCard
                  key={p.id}
                  p={p}
                  mode={st.mode}
                  isCurrent={st.mode === "single" && cur === p.id}
                  selected={props.selectedProvider === p.id}
                  enabled={p.isNew || isEnabled(p, draft)}
                  visible={p.isNew ? p.models.length : viewModels(p.id, p.models, draft).filter((m) => !m.isDeleted && isVisible(p.id, m, draft)).length}
                  latency={p.baseUrl ? latency[p.baseUrl] : undefined}
                  readonly={st.readonly}
                  onSelect={() => props.onSelectProvider(p.id)}
                  onModels={() => { setRailSel(p.id); setTab("models"); }}
                  onAction={() => props.onProviderAction(p)}
                  onTest={() => p.baseUrl && props.onTestOne(p.baseUrl)}
                />
              ))}
              <button className="pcard-add" disabled={st.readonly} onClick={props.onAddProvider}><Icon.plus size={18} />添加供应商</button>
            </div>
          </div>
        )}

        {tab === "models" && (
          st.catalog ? (
            <ModelTable
              st={st}
              title={`模型目录 · ${st.catalogFile}`}
              note="Codex 的模型选择器只读这份目录，切换供应商不会改变它。拉取模型会从当前供应商获取。"
              pid={CATALOG}
              fetchFrom={cur && cur !== "openai" ? cur : null}
              models={viewModels(CATALOG, st.catalog, draft)}
              draft={draft}
              setDraft={setDraft}
              readonly={st.readonly}
              onToggle={toggleModel}
              flash={props.flash}
            />
          ) : st.mode === "single" ? (
            <div className="empty">没有可编辑的模型目录（config.toml 未设置 model_catalog_json）。</div>
          ) : (
            (() => {
              const list = providers.filter((p) => !p.isNew && !p.isDeleted);
              const sel = list.find((p) => p.id === railSel) ?? list[0];
              if (!sel) return <div className="empty">没有供应商</div>;
              const enabled = isEnabled(sel, draft);
              return (
                <div className="models-split">
                  <div className="rail">
                    <span className="side-label">供应商</span>
                    {list.map((p) => (
                      <button key={p.id} className={`rail-item${p.id === sel.id ? " on" : ""}`} onClick={() => setRailSel(p.id)}>
                        <span className="dot" style={{ background: isEnabled(p, draft) ? "#16A34A" : "#B8BFC9" }} />
                        <span className="ellipsis grow">{p.name}</span>
                        <span className="mono muted tiny">{viewModels(p.id, p.models, draft).filter((m) => !m.isDeleted && isVisible(p.id, m, draft)).length}/{p.models.length}</span>
                      </button>
                    ))}
                  </div>
                  <ModelTable
                    st={st}
                    title={sel.name}
                    note={sel.builtin ? `内置供应商的模型由 ${st.name} 自己管理` : enabled ? "开关控制是否出现在选择器里；也可以添加、编辑或删除模型" : "该供应商已停用，先在「供应商」里启用"}
                    pid={sel.id}
                    fetchFrom={sel.builtin || !sel.baseUrl ? null : sel.id}
                    models={viewModels(sel.id, sel.models, draft)}
                    draft={draft}
                    setDraft={setDraft}
                    readonly={st.readonly || !enabled || sel.builtin}
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

        {tab === "set" && (() => {
          // Fixed id is pre-enabled: shown as on, written together with the next provider switch.
          const fixedPre = st.fixedPrompt && !draft[keys.setting("fixed_id")];
          const shown = fixedPre ? st.settings.map((s) => (s.key === "fixed_id" ? { ...s, value: true } : s)) : st.settings;
          return (
            <Settings settings={shown} draft={draft} readonly={st.readonly}
              onChange={(s, v) => (fixedPre && s.key === "fixed_id" ? props.onDeclineFixed() : setDraft(setSetting(draft, s, v)))}
              notes={fixedPre ? {
                fixed_id: <div className="set-note">已预开启，还没写入：切换供应商时会一起写入 config.toml，不单独算一项改动。关掉就不再预开启。</div>,
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
  draft: Draft;
  setDraft: (d: Draft) => void;
  readonly: boolean;
  onToggle: (pid: string, m: Model) => void;
  flash: (text: string, error?: boolean) => void;
}

function ModelTable({ st, title, note, pid, fetchFrom, models, draft, setDraft, readonly, onToggle, flash }: ModelTableProps) {
  const hasNames = st.id !== "zcode"; // ZCode has no per-model display name
  const [editing, setEditing] = useState<string | null>(null); // model id, or "__new"
  const [fetched, setFetched] = useState<string[] | null>(null);
  const [fetching, setFetching] = useState(false);
  const [pick, setPick] = useState<Set<string>>(new Set());
  const [filter, setFilter] = useState("");

  useEffect(() => { setEditing(null); setFetched(null); setFilter(""); }, [pid]);

  const existing = new Set(models.map((m) => m.id));
  const doFetch = async () => {
    if (!fetchFrom) return;
    setFetching(true);
    try {
      const list = await api.fetchModels(st.id, fetchFrom);
      const fresh = list.filter((m) => !existing.has(m));
      setFetched(fresh);
      setPick(new Set());
      if (fresh.length === 0) flash(`供应商返回 ${list.length} 个模型，都已在列表里`);
    } catch (e) {
      flash(`拉取失败：${e}`, true);
    } finally {
      setFetching(false);
    }
  };

  const addPicked = () => {
    let d = draft;
    for (const id of pick) d = upsertModel(d, pid, { id, name: null, context: null });
    setDraft(d);
    setFetched(null);
    flash(`已添加 ${pick.size} 个模型，应用后生效`);
  };

  const shown = models.filter((m) => !filter || m.id.toLowerCase().includes(filter.toLowerCase()) || (m.name ?? "").toLowerCase().includes(filter.toLowerCase()));

  return (
    <div className="mtable">
      <div className="mtable-head">
        <div className="grow minw0">
          <div className="strong ellipsis">{title}</div>
          <div className="muted small">{note}</div>
        </div>
        <input className="search-input slim" placeholder="筛选" value={filter} onChange={(e) => setFilter(e.target.value)} />
        <button className="btn small" disabled={readonly || !fetchFrom || fetching} onClick={doFetch} title={fetchFrom ? "" : "这个供应商没有可拉取的地址"}>
          <Icon.refresh size={12} />{fetching ? "拉取中…" : "拉取模型"}
        </button>
        <button className="btn small" disabled={readonly} onClick={() => setEditing("__new")}><Icon.plus size={12} />添加模型</button>
      </div>

      {fetched && fetched.length > 0 && (
        <div className="fetched">
          <div className="row between">
            <span className="small strong">供应商还有 {fetched.length} 个模型不在列表里</span>
            <span className="row gap6">
              <button className="link tiny" onClick={() => setPick(new Set(pick.size === fetched.length ? [] : fetched))}>{pick.size === fetched.length ? "全不选" : "全选"}</button>
              <button className="btn small" onClick={() => setFetched(null)}>收起</button>
              <button className="btn small primary" disabled={pick.size === 0} onClick={addPicked}>添加选中的 {pick.size} 个</button>
            </span>
          </div>
          <div className="pick-list wide">
            {fetched.map((m) => (
              <label key={m} className="pick">
                <input type="checkbox" checked={pick.has(m)} onChange={() => setPick((s) => { const n = new Set(s); if (n.has(m)) n.delete(m); else n.add(m); return n; })} />
                <span className="mono small">{m}</span>
              </label>
            ))}
          </div>
        </div>
      )}

      <div className="mrow mhead"><span /><span>模型</span><span>上下文</span><span className="right">在选择器中显示</span></div>
      {editing === "__new" && (
        <ModelForm hasNames={hasNames} onCancel={() => setEditing(null)}
          onSave={(input) => {
            if (existing.has(input.id)) { flash(`${input.id} 已经在列表里`, true); return; }
            setDraft(upsertModel(draft, pid, input));
            setEditing(null);
          }} />
      )}
      {shown.map((m) => {
        if (editing === m.id) {
          return (
            <ModelForm key={m.id} hasNames={hasNames} initial={m} onCancel={() => setEditing(null)}
              onSave={(input) => {
                const same = input.name === m.name && input.context === m.context;
                const key = keys.upsertModel(pid, m.id);
                if (m.isNew) setDraft(upsertModel(draft, pid, input));
                else setDraft(same ? withOp(draft, key, null) : upsertModel(draft, pid, input));
                setEditing(null);
              }} />
          );
        }
        const on = !m.isDeleted && isVisible(pid, m, draft);
        const dirty = m.isNew || m.isEdited || m.isDeleted || on !== m.visible;
        return (
          <div key={m.id} className={`mrow${m.isDeleted ? " deleted" : ""}`}>
            <Icon.grip />
            <span className="minw0">
              <span className="row gap6 minw0">
                <span className={`mono ellipsis${on ? "" : " faint"}${dirty ? " dirty" : ""}`}>{m.id}</span>
                {m.isNew && <span className="mtag new">新</span>}
                {m.isDeleted && <span className="mtag">将删除</span>}
                {m.tags.map((t) => <span key={t} className={`mtag${t === "Fast" ? " fast" : ""}`}>{t}</span>)}
              </span>
              {hasNames && m.name && m.name !== m.id && <span className="tiny muted ellipsis block">{m.name}</span>}
            </span>
            <span className="mono muted small">{m.ctx ?? "—"}</span>
            <span className="row gap6 right">
              {m.readonly ? (
                <span className="muted small">内置 · 只读</span>
              ) : m.isDeleted ? (
                <button className="link tiny" onClick={() => setDraft(withOp(draft, keys.deleteModel(pid, m.id), null))}>撤销删除</button>
              ) : (
                <>
                  <button className="icon-btn sm" aria-label={`编辑 ${m.id}`} title="编辑" disabled={readonly} onClick={() => setEditing(m.id)}>
                    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M12 20h9" /><path d="M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4Z" /></svg>
                  </button>
                  {(m.deletable || m.isNew) && (
                    <button className="icon-btn sm" aria-label={`删除 ${m.id}`} title="删除" disabled={readonly}
                      onClick={() => {
                        if (m.isNew) setDraft(withOp(draft, keys.upsertModel(pid, m.id), null));
                        else setDraft(withOp(draft, keys.deleteModel(pid, m.id), { op: "delete_model", provider: pid, model: m.id }));
                      }}>
                      <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14" /></svg>
                    </button>
                  )}
                  {!m.isNew && <Switch on={on} disabled={readonly} label={`${on ? "隐藏" : "显示"} ${m.id}`} onClick={() => onToggle(pid, m)} />}
                </>
              )}
            </span>
          </div>
        );
      })}
      <div className="mtable-foot muted small">共 {models.filter((m) => !m.isDeleted).length} 个模型{filter && ` · 筛选出 ${shown.length} 个`}</div>
    </div>
  );
}

function ModelForm({ hasNames, initial, onSave, onCancel }: {
  hasNames: boolean; initial?: ViewModel;
  onSave: (m: { id: string; name: string | null; context: number | null }) => void; onCancel: () => void;
}) {
  const [id, setId] = useState(initial?.id ?? "");
  const [name, setName] = useState(initial?.name ?? "");
  const [ctx, setCtx] = useState(initial?.context ? String(initial.context) : "");
  const ctxVal = parseCtx(ctx);
  const ctxBad = ctx.trim() !== "" && ctxVal === null;
  const submit = () => {
    if (!id.trim() || ctxBad) return;
    onSave({ id: id.trim(), name: hasNames && name.trim() ? name.trim() : null, context: ctxVal });
  };
  return (
    <div className="mform" onKeyDown={(e) => { if (e.key === "Enter") submit(); if (e.key === "Escape") onCancel(); }}>
      <input className="input mono" autoFocus={!initial} value={id} disabled={!!initial} onChange={(e) => setId(e.target.value)} placeholder="模型 ID，如 deepseek-v4-pro" />
      {hasNames && <input className="input" autoFocus={!!initial} value={name} onChange={(e) => setName(e.target.value)} placeholder="显示名（可选）" />}
      <input className={`input mono ctx${ctxBad ? " bad" : ""}`} value={ctx} onChange={(e) => setCtx(e.target.value)} placeholder="上下文，如 128k / 1m" />
      <button className="btn small" onClick={onCancel}>取消</button>
      <button className="btn small primary" disabled={!id.trim() || ctxBad} onClick={submit}>{initial ? "保存" : "添加"}</button>
    </div>
  );
}

function Settings({ settings, draft, readonly, onChange, notes }: {
  settings: Setting[]; draft: Draft; readonly: boolean; onChange: (s: Setting, v: boolean | string[]) => void;
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
                <div className="slabel">{s.label}{dirty && <span className="unsaved">未应用</span>}</div>
                <div className="muted small">{s.desc}</div>
                {notes?.[s.key]}
              </div>
            );
            if (s.kind === "bool") {
              return (
                <div key={s.key} className="srow" id={`setting-${s.key}`}>
                  {head}
                  <Switch on={v === true} disabled={readonly} fast={s.key.startsWith("fast")} label={s.label} onClick={() => onChange(s, !(v === true))} />
                </div>
              );
            }
            // Multi-select: options as a grid of cards under the title, each with its explanation.
            const arr = v as string[];
            return (
              <div key={s.key} className="srow stacked" id={`setting-${s.key}`}>
                <div className="row between">
                  {head}
                  <span className="tiny muted">已选 {arr.length} / {s.options.length}</span>
                </div>
                <div className="opt-grid">
                  {s.options.map((o, i) => {
                    const on = arr.includes(o);
                    return (
                      <button key={o} className={`opt${on ? " on" : ""}`} aria-pressed={on} disabled={readonly}
                        onClick={() => onChange(s, on ? arr.filter((x) => x !== o) : [...arr, o])}>
                        <span className="opt-check" aria-hidden="true">
                          {on && <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="#fff" strokeWidth="3.5" strokeLinecap="round" strokeLinejoin="round"><path d="M20 6 9 17l-5-5" /></svg>}
                        </span>
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

export function Switch({ on, disabled, fast, label, onClick }: { on: boolean; disabled?: boolean; fast?: boolean; label: string; onClick: () => void }) {
  return (
    <button className={`switch${on ? " on" : ""}${fast ? " fast" : ""}`} aria-pressed={on} aria-label={label} disabled={disabled} onClick={onClick}>
      <span />
    </button>
  );
}
