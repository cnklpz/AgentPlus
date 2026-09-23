import { useCallback, useEffect, useMemo, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { type AgentId, type AgentState, type DiffGroup, type Provider, api } from "./api";
import { type Draft, isEnabled, keys, withOp } from "./draft";
import { AgentPage, type Tab } from "./components/AgentPage";
import { ProviderDetail } from "./components/ProviderDetail";
import { Aside } from "./components/Aside";
import { Icon } from "./components/icons";
import type { Latency } from "./components/ProviderCard";
import { Sidebar } from "./components/Sidebar";

/** Windows-style caption buttons; the system title bar is turned off. */
function WindowControls() {
  if (!("__TAURI_INTERNALS__" in window)) return null;
  const win = getCurrentWindow();
  const glyph = (d: string) => (
    <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1" aria-hidden="true"><path d={d} /></svg>
  );
  return (
    <div className="winctl">
      <button aria-label="最小化" onClick={() => win.minimize()}>{glyph("M0 5.5h10")}</button>
      <button aria-label="最大化" onClick={() => win.toggleMaximize()}>{glyph("M0.5 0.5h9v9h-9z")}</button>
      <button aria-label="关闭" className="close" onClick={() => win.close()}>{glyph("M0 0l10 10M10 0L0 10")}</button>
    </div>
  );
}

export default function App() {
  const [agents, setAgents] = useState<AgentState[]>([]);
  const [selected, setSelected] = useState<AgentId>("codex");
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [diff, setDiff] = useState<DiffGroup[]>([]);
  const [diffError, setDiffError] = useState<string | null>(null);
  const [latency, setLatency] = useState<Record<string, Latency>>({});
  const [busy, setBusy] = useState(false);
  const [restarting, setRestarting] = useState<AgentId | null>(null);
  const [toast, setToast] = useState<{ text: string; error?: boolean } | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  // Per-agent page state, kept here so the detail panel can drive it.
  const [tabs, setTabs] = useState<Record<string, Tab>>({});
  const [rails, setRails] = useState<Record<string, string | null>>({});
  const [picked, setPicked] = useState<Record<string, string | null>>({});

  const st = agents.find((a) => a.id === selected);
  const draft = drafts[selected] ?? {};
  const ops = useMemo(() => Object.values(draft), [draft]);

  const flash = (text: string, error = false, ms = 3200) => {
    setToast({ text, error });
    window.setTimeout(() => setToast((t) => (t?.text === text ? null : t)), ms);
  };

  useEffect(() => {
    api.listAgents().then(setAgents).catch((e) => setLoadError(String(e)));
  }, []);

  // Live diff preview for the selected agent.
  useEffect(() => {
    if (!st) return;
    if (ops.length === 0) {
      setDiff([]);
      setDiffError(null);
      return;
    }
    let alive = true;
    api.preview(st.id, ops)
      .then((d) => { if (alive) { setDiff(d); setDiffError(null); } })
      .catch((e) => { if (alive) { setDiff([]); setDiffError(String(e)); } });
    return () => { alive = false; };
  }, [st, ops]);

  const testAll = useCallback((force: boolean) => {
    if (!st) return;
    const urls = [...new Set(st.providers.filter((p) => p.baseUrl && p.compatible).map((p) => p.baseUrl!))];
    for (const u of urls) {
      if (force || latency[u] === undefined) testOne(u);
    }
  }, [st, latency]);

  // Measure latency once per agent when it is first shown.
  useEffect(() => { testAll(false); }, [st?.id, agents.length]);

  const setDraft = (d: Draft) => setDrafts((all) => ({ ...all, [selected]: d }));

  const providerAction = (p: Provider) => {
    if (!st) return;
    if (st.mode === "single") {
      setDraft(withOp(draft, keys.cur(), p.id === st.currentProvider ? null : { op: "set_current_provider", provider: p.id }));
    } else {
      const next = !isEnabled(p, draft);
      setDraft(withOp(draft, keys.enabled(p.id), next === p.enabled ? null : { op: "set_provider_enabled", provider: p.id, enabled: next }));
    }
  };

  const testOne = (url: string) => {
    setLatency((l) => ({ ...l, [url]: "pending" }));
    api.testLatency(url)
      .then((ms) => setLatency((l) => ({ ...l, [url]: ms })))
      .catch((e) => setLatency((l) => ({ ...l, [url]: String(e) })));
  };

  const pickedProvider = st?.providers.find((p) => p.id === picked[selected]) ?? null;

  const replaceAgent = (next: AgentState) => setAgents((list) => list.map((a) => (a.id === next.id ? next : a)));

  const apply = async () => {
    if (!st) return;
    setBusy(true);
    try {
      const r = await api.apply(st.id, ops);
      replaceAgent(r.state);
      setDraft({});
      flash(r.files.length ? `已写入 ${r.files.length} 个文件，原文件已备份` : "已保存");
    } catch (e) {
      flash(String(e), true, 6000);
    } finally {
      setBusy(false);
    }
  };

  const restart = async () => {
    if (!st) return;
    setRestarting(st.id);
    setToast({ text: `正在重启 ${st.name}…` });
    try {
      const msg = await api.restart(st.id);
      flash(`${st.name}：${msg}`, false, 5000);
      replaceAgent(await api.getAgent(st.id));
    } catch (e) {
      flash(`${st.name} 重启失败：${e}`, true, 8000);
    } finally {
      setRestarting(null);
    }
  };

  return (
    <div className="app">
      <header className="topbar" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region><Icon.logo /><span data-tauri-drag-region>AgentPlus</span></div>
        <div className="search"><Icon.search /><span>搜索服务商、模型、设置</span><kbd>Ctrl K</kbd></div>
        <div className="top-right" data-tauri-drag-region>
          <span className="host"><Icon.monitor />本机 · Windows</span>
        </div>
        <WindowControls />
      </header>

      <div className="body">
        <Sidebar agents={agents} drafts={drafts} selected={selected} onSelect={(id) => setSelected(id)} />
        {st ? (
          <AgentPage
            key={st.id}
            st={st}
            draft={draft}
            setDraft={setDraft}
            latency={latency}
            onTestAll={() => testAll(true)}
            restarting={restarting === st.id}
            onRestart={restart}
            onOpenDir={() => api.openConfigDir(st.id).catch((e) => flash(String(e), true))}
            tab={tabs[st.id] ?? "prov"}
            setTab={(t) => setTabs((m) => ({ ...m, [st.id]: t }))}
            railSel={rails[st.id] ?? null}
            setRailSel={(id) => setRails((m) => ({ ...m, [st.id]: id }))}
            selectedProvider={picked[st.id] ?? null}
            onSelectProvider={(id) => setPicked((m) => ({ ...m, [st.id]: m[st.id] === id ? null : id }))}
            onProviderAction={providerAction}
          />
        ) : (
          <main className="page"><div className="empty">{loadError ?? "正在读取配置…"}</div></main>
        )}
        {st && (
          <Aside
            st={st}
            diff={diff}
            pending={ops.length}
            error={diffError}
            busy={busy}
            onDiscard={() => setDraft({})}
            onApply={apply}
            detail={pickedProvider && (
              <ProviderDetail
                st={st}
                p={pickedProvider}
                draft={draft}
                agents={agents}
                latency={pickedProvider.baseUrl ? latency[pickedProvider.baseUrl] : undefined}
                onClose={() => setPicked((m) => ({ ...m, [st.id]: null }))}
                onTest={() => pickedProvider.baseUrl && testOne(pickedProvider.baseUrl)}
                onAction={() => providerAction(pickedProvider)}
                onModels={() => {
                  setRails((m) => ({ ...m, [st.id]: pickedProvider.id }));
                  setTabs((m) => ({ ...m, [st.id]: "models" }));
                }}
                onCopy={(text) => navigator.clipboard.writeText(text).then(() => flash("已复制地址")).catch(() => flash("复制失败", true))}
              />
            )}
          />
        )}
      </div>

      {toast && <div className={`toast${toast.error ? " error" : ""}`} role="status">{toast.text}</div>}
    </div>
  );
}
