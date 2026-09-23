import type { AgentState, Model, Provider, Setting } from "../api";
import {
  CATALOG, type Draft, currentProvider, isEnabled, isVisible, keys, setSetting, settingValue, visibleCount, withOp,
} from "../draft";
import { AgentIcon, Icon } from "./icons";
import { type Latency, ProviderCard } from "./ProviderCard";

export type Tab = "prov" | "models" | "set";

interface Props {
  st: AgentState;
  draft: Draft;
  setDraft: (d: Draft) => void;
  latency: Record<string, Latency>;
  onTestAll: () => void;
  restarting: boolean;
  onRestart: () => void;
  onOpenDir: () => void;
  tab: Tab;
  setTab: (t: Tab) => void;
  railSel: string | null;
  setRailSel: (id: string | null) => void;
  selectedProvider: string | null;
  onSelectProvider: (id: string) => void;
  onProviderAction: (p: Provider) => void;
}

export function AgentPage(props: Props) {
  const { st, draft, setDraft, latency, onTestAll, restarting, onRestart, onOpenDir, tab, setTab, railSel, setRailSel } = props;
  const cur = currentProvider(st, draft);

  const provCount = st.providers.filter((p) => p.compatible && isEnabled(p, draft)).length;
  const tabs: [Tab, string, number | null][] = [
    ["prov", "供应商", st.mode === "single" ? st.providers.filter((p) => p.compatible).length : provCount],
    ["models", "模型列表", visibleCount(st, draft)],
    ["set", "其他设置", null],
  ];

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
            <span className="mono muted small">{st.files.join(" · ")}</span>
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

      <div className="page-body">
        {tab === "prov" && (
          <div className="stack12">
            <div className="row between">
              <span className="muted small">
                {st.mode === "single"
                  ? `${st.name} 同一时间只用一个供应商。只提供 Chat 接口的供应商不能用于 Codex。`
                  : `${st.name} 可以同时启用多个供应商，它们的模型会一起出现在选择器里。`}
              </span>
              <button className="btn small" onClick={onTestAll}><Icon.pulse />全部测速</button>
            </div>
            <div className="pgrid">
              {st.providers.map((p) => (
                <ProviderCard
                  key={p.id}
                  p={p}
                  mode={st.mode}
                  isCurrent={st.mode === "single" && cur === p.id}
                  selected={props.selectedProvider === p.id}
                  enabled={isEnabled(p, draft)}
                  visible={p.models.filter((m) => isVisible(p.id, m, draft)).length}
                  latency={p.baseUrl ? latency[p.baseUrl] : undefined}
                  readonly={st.readonly}
                  onSelect={() => props.onSelectProvider(p.id)}
                  onModels={() => { setRailSel(p.id); setTab("models"); }}
                  onAction={() => props.onProviderAction(p)}
                />
              ))}
              <button className="pcard-add" disabled title="即将推出"><Icon.plus size={18} />添加供应商</button>
            </div>
          </div>
        )}

        {tab === "models" && (
          st.catalog ? (
            <ModelTable
              title={`模型目录 · ${st.catalogFile}`}
              note="Codex 的模型选择器只读这份目录，切换供应商不会改变它"
              pid={CATALOG}
              models={st.catalog}
              draft={draft}
              readonly={st.readonly}
              onToggle={toggleModel}
            />
          ) : st.mode === "single" ? (
            <div className="empty">没有可编辑的模型目录（config.toml 未设置 model_catalog_json）。</div>
          ) : (
            (() => {
              const list = st.providers.filter((p) => p.models.length > 0);
              const sel = list.find((p) => p.id === railSel) ?? list[0];
              if (!sel) return <div className="empty">没有供应商</div>;
              return (
                <div className="models-split">
                  <div className="rail">
                    <span className="side-label">供应商</span>
                    {list.map((p) => (
                      <button key={p.id} className={`rail-item${p.id === sel.id ? " on" : ""}`} onClick={() => setRailSel(p.id)}>
                        <span className="dot" style={{ background: isEnabled(p, draft) ? "#16A34A" : "#B8BFC9" }} />
                        <span className="ellipsis grow">{p.name}</span>
                        <span className="mono muted tiny">{p.models.filter((m) => isVisible(p.id, m, draft)).length}/{p.models.length}</span>
                      </button>
                    ))}
                  </div>
                  <ModelTable
                    title={sel.name}
                    note={sel.builtin ? `内置供应商的模型由 ${st.name} 自己管理` : isEnabled(sel, draft) ? "打开开关的模型会出现在选择器里" : "该供应商已停用，先在「供应商」里启用"}
                    pid={sel.id}
                    models={sel.models}
                    draft={draft}
                    readonly={st.readonly || !isEnabled(sel, draft)}
                    onToggle={toggleModel}
                  />
                </div>
              );
            })()
          )
        )}

        {tab === "set" && <Settings settings={st.settings} draft={draft} readonly={st.readonly} onChange={(s, v) => setDraft(setSetting(draft, s, v))} />}
      </div>
    </main>
  );
}

function ModelTable(props: {
  title: string; note: string; pid: string; models: Model[]; draft: Draft; readonly: boolean;
  onToggle: (pid: string, m: Model) => void;
}) {
  const { title, note, pid, models, draft, readonly, onToggle } = props;
  return (
    <div className="mtable">
      <div className="mtable-head">
        <div className="grow">
          <div className="strong">{title}</div>
          <div className="muted small">{note}</div>
        </div>
      </div>
      <div className="mrow mhead"><span /><span>模型 ID</span><span>上下文</span><span className="right">在选择器中显示</span></div>
      {models.map((m) => {
        const on = isVisible(pid, m, draft);
        const dirty = on !== m.visible;
        return (
          <div key={m.id} className="mrow">
            <Icon.grip />
            <span className="row gap6 minw0">
              <span className={`mono ellipsis${on ? "" : " faint"}${dirty ? " dirty" : ""}`}>{m.id}</span>
              {m.tags.map((t) => <span key={t} className={`mtag${t === "Fast" ? " fast" : ""}`}>{t}</span>)}
            </span>
            <span className="mono muted small">{m.ctx ?? "—"}</span>
            <span className="right">
              {m.readonly ? (
                <span className="muted small">内置 · 只读</span>
              ) : (
                <Switch on={on} disabled={readonly} label={`${on ? "隐藏" : "显示"} ${m.id}`} onClick={() => onToggle(pid, m)} />
              )}
            </span>
          </div>
        );
      })}
      <div className="mtable-foot muted small">共 {models.length} 个模型</div>
    </div>
  );
}

function Settings({ settings, draft, readonly, onChange }: {
  settings: Setting[]; draft: Draft; readonly: boolean; onChange: (s: Setting, v: boolean | string[]) => void;
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
            return (
              <div key={s.key} className="srow">
                <div className="grow minw0">
                  <div className="slabel">{s.label}{dirty && <span className="unsaved">未应用</span>}</div>
                  <div className="muted small">{s.desc}</div>
                </div>
                {s.kind === "bool" ? (
                  <Switch on={v === true} disabled={readonly} fast={s.key.startsWith("fast")} label={s.label} onClick={() => onChange(s, !(v === true))} />
                ) : (
                  <div className="chips">
                    {s.options.map((o) => {
                      const arr = v as string[];
                      const on = arr.includes(o);
                      return (
                        <button key={o} className={`chip${on ? " on" : ""}`} aria-pressed={on} disabled={readonly}
                          onClick={() => onChange(s, on ? arr.filter((x) => x !== o) : [...arr, o])}>{o}</button>
                      );
                    })}
                  </div>
                )}
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
