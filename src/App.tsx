import { useCallback, useEffect, useMemo, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { type AgentId, type AgentState, type DiffGroup, type EnvInfo, type LibEntry, type ProviderInput, type SyncSuggestion, api } from "./api";
import { type Draft, type ViewProvider, isEnabled, keys, opsToWrite, upsertProvider, viewProviders, withOp } from "./draft";
import { AgentPage, type Tab } from "./components/AgentPage";
import { Aside } from "./components/Aside";
import { CommandPalette, type Target } from "./components/CommandPalette";
import { HistoryPage } from "./components/HistoryPage";
import { Icon } from "./components/icons";
import type { Latency } from "./components/ProviderCard";
import { ProviderDetail } from "./components/ProviderDetail";
import { ProviderDialog } from "./components/ProviderDialog";
import { ProvidersHub } from "./components/ProvidersHub";
import { HubAside } from "./components/HubAside";
import { ServiceDetail } from "./components/ServiceDetail";
import { type ServiceSave, ServiceDialog } from "./components/ServiceDialog";
import { EnvSwitch } from "./components/EnvSwitch";
import { type SettingsTab, SettingsPage } from "./components/SettingsPage";
import { PendingDialog } from "./components/PendingDialog";
import { type Prefs, applyMotion, loadPrefs, savePrefs } from "./prefs";
import { type Service, type Use, buildServices, hostKey, importKey, importOp } from "./services";
import { type Page, Sidebar } from "./components/Sidebar";
import { SyncPage } from "./components/SyncPage";

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

type Dialog = { editing: ViewProvider | null } | null;
/** Hub dialog: undefined = closed, null = add new, Service = edit. */
type HubDialog = Service | null | undefined;

export default function App() {
  const [agents, setAgents] = useState<AgentState[]>([]);
  const [selected, setSelected] = useState<AgentId>("codex");
  const [page, setPage] = useState<Page | null>(null);
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [diff, setDiff] = useState<DiffGroup[]>([]);
  const [diffError, setDiffError] = useState<string | null>(null);
  const [latency, setLatency] = useState<Record<string, Latency>>({});
  const [busy, setBusy] = useState(false);
  const [restarting, setRestarting] = useState<AgentId | null>(null);
  const [toast, setToast] = useState<{ text: string; error?: boolean } | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [palette, setPalette] = useState(false);
  const [sessionQuery, setSessionQuery] = useState<string | undefined>(undefined);
  const [lib, setLib] = useState<LibEntry[]>([]);
  const [envs, setEnvs] = useState<EnvInfo[]>([]);
  const [switching, setSwitching] = useState(false);
  const [prefs, setPrefsState] = useState<Prefs>(loadPrefs);
  const [settingsTab, setSettingsTab] = useState<SettingsTab>("general");
  // Settings is a full-window page; remember where to go back to.
  const [beforeSettings, setBeforeSettings] = useState<Page | null>(null);
  const openSettings = () => {
    reloadEnvs();
    if (page !== "settings") setBeforeSettings(page);
    setPage("settings");
  };
  const closeSettings = () => setPage(beforeSettings);
  const [hubSel, setHubSel] = useState<string | null>(null);
  const [hubDialog, setHubDialog] = useState<HubDialog>(undefined);
  const setPrefs = (p: Prefs) => { setPrefsState(p); savePrefs(p); };
  useEffect(() => applyMotion(prefs.motion), [prefs.motion]);

  // Per-agent page state, kept here so the detail panel and search can drive it.
  const [tabs, setTabs] = useState<Record<string, Tab>>({});
  const [rails, setRails] = useState<Record<string, string | null>>({});
  const [picked, setPicked] = useState<Record<string, string | null>>({});

  // Agents that were not found in this environment are not shown anywhere.
  const shown = useMemo(() => agents.filter((a) => a.installed), [agents]);
  const st = shown.find((a) => a.id === selected) ?? shown[0];
  useEffect(() => { if (st && st.id !== selected) setSelected(st.id); }, [st?.id]);
  const draft = drafts[selected] ?? {};
  const ops = useMemo(() => Object.values(draft), [draft]);

  const flash = useCallback((text: string, error = false, ms = 3600) => {
    setToast({ text, error });
    window.setTimeout(() => setToast((t) => (t?.text === text ? null : t)), ms);
  }, []);

  const reload = () => api.listAgents().then(setAgents).catch((e) => setLoadError(String(e)));
  const reloadLib = () => api.libraryList().then(setLib).catch(() => undefined);
  const reloadEnvs = () => api.listEnvs().then(setEnvs).catch(() => undefined);
  useEffect(() => { reload(); reloadLib(); reloadEnvs(); }, []);

  const services = useMemo(() => buildServices(agents.filter((a) => a.installed), drafts, lib), [agents, drafts, lib]);
  const hubService = services.find((s) => s.key === hubSel) ?? null;
  const pendingTotal = Object.values(drafts).reduce((n, d) => n + Object.keys(d).length, 0);
  const curEnv = envs.find((e) => e.current);

  // Ctrl+K opens search from anywhere.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPalette(true);
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
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
    api.preview(st.id, opsToWrite(st, draft))
      .then((d) => { if (alive) { setDiff(d); setDiffError(null); } })
      .catch((e) => { if (alive) { setDiff([]); setDiffError(String(e)); } });
    return () => { alive = false; };
  }, [st, ops]);

  const testOne = (url: string) => {
    setLatency((l) => ({ ...l, [url]: "pending" }));
    api.testLatency(url)
      .then((ms) => setLatency((l) => ({ ...l, [url]: ms })))
      .catch((e) => setLatency((l) => ({ ...l, [url]: String(e) })));
  };

  const testAll = useCallback((force: boolean) => {
    if (!st) return;
    const urls = [...new Set(st.providers.filter((p) => p.baseUrl && p.compatible).map((p) => p.baseUrl!))];
    for (const u of urls) {
      if (force || latency[u] === undefined) testOne(u);
    }
  }, [st, latency]);

  // Measure latency once per agent when it is first shown.
  useEffect(() => { if (prefs.autoLatency && !page) testAll(false); }, [st?.id, agents.length, page]);

  const testHub = useCallback((force: boolean) => {
    for (const u of new Set(services.filter((s) => s.baseUrl).map((s) => s.baseUrl!))) {
      if (force || latency[u] === undefined) testOne(u);
    }
  }, [services, latency]);
  useEffect(() => { if (prefs.autoLatency && page === "providers") testHub(false); }, [page, services.length]);

  const setDraftFor = (agent: string, d: Draft) => setDrafts((all) => ({ ...all, [agent]: d }));
  const setDraft = (d: Draft) => setDraftFor(selected, d);

  // Codex's fixed id is on by default but never a pending change of its own
  // (see opsToWrite). Declining it is remembered by the backend.
  const declineFixed = async () => {
    await api.dismissFixedPrompt().catch((e) => flash(String(e), true));
    replaceAgent(await api.getAgent("codex"));
    flash("已关闭固定 ID 的预开启，需要时再手动打开");
  };

  const providerAction = (p: ViewProvider) => {
    if (!st || p.isNew || p.isDeleted) return;
    if (st.mode === "single") {
      setDraft(withOp(draft, keys.cur(), p.id === st.currentProvider ? null : { op: "set_current_provider", provider: p.id }));
    } else {
      const next = !isEnabled(p, draft);
      setDraft(withOp(draft, keys.enabled(p.id), next === p.enabled ? null : { op: "set_provider_enabled", provider: p.id, enabled: next }));
    }
  };

  const shownProviders = st ? viewProviders(st, draft) : [];
  const pickedProvider = shownProviders.find((p) => p.id === picked[selected]) ?? null;
  const closeDetail = () => setPicked((m) => ({ ...m, [selected]: null }));

  // Clicking outside the cards / detail panel, or pressing Esc, closes the details.
  useEffect(() => {
    if (!pickedProvider || dialog || palette) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Element | null;
      if (!t?.closest(".pcard, .pdetail, .toast, .modal-bg")) closeDetail();
    };
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") closeDetail(); };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [pickedProvider?.id, selected, dialog, palette]);

  const replaceAgent = (next: AgentState) => setAgents((list) => list.map((a) => (a.id === next.id ? next : a)));

  const restartAgent = async (a: AgentState) => {
    setRestarting(a.id);
    setToast({ text: `正在重启 ${a.name}…` });
    try {
      const msg = await api.restart(a.id);
      flash(`${a.name}：${msg}`, false, 5000);
      replaceAgent(await api.getAgent(a.id));
    } catch (e) {
      flash(`${a.name} 重启失败：${e}`, true, 8000);
    } finally {
      setRestarting(null);
    }
  };
  const restart = () => { if (st) restartAgent(st); };

  const apply = async () => {
    if (!st) return;
    setBusy(true);
    try {
      const r = await api.apply(st.id, opsToWrite(st, draft));
      replaceAgent(r.state);
      setDraft({});
      setPicked((m) => ({ ...m, [st.id]: null }));
      flash(r.files.length ? `已写入 ${r.files.length} 个文件，原文件已备份` : "已保存");
      // Optional auto-restart (off by default); only when something the agent reads changed.
      const auto = r.state.settings.find((s) => s.key === "auto_restart")?.value === true;
      const touchedAgent = ops.some((o) => !(o.op === "set_setting" && o.key === "auto_restart"));
      if (auto && touchedAgent && r.state.running) await restartAgent(r.state);
    } catch (e) {
      flash(String(e), true, 7000);
    } finally {
      setBusy(false);
    }
  };

  /** Applies the given agents' pending changes one by one; true when all succeeded. */
  const applyAgents = async (ids: AgentId[]): Promise<boolean> => {
    setBusy(true);
    const done: string[] = [];
    try {
      for (const a of agents) {
        if (!ids.includes(a.id) || !Object.keys(drafts[a.id] ?? {}).length) continue;
        const r = await api.apply(a.id, opsToWrite(a, drafts[a.id])).catch((e) => { throw new Error(`${a.name}：${e}`); });
        replaceAgent(r.state);
        setDraftFor(a.id, {});
        done.push(a.name);
        const auto = r.state.settings.find((s) => s.key === "auto_restart")?.value === true;
        if (auto && r.state.running) await restartAgent(r.state);
      }
      if (done.length) flash(`已写入 ${done.join("、")}，原文件已备份`);
      return true;
    } catch (e) {
      flash(`${done.length ? `已写入 ${done.join("、")}；` : ""}${String(e).replace(/^Error: /, "")}`, true, 8000);
      return false;
    } finally {
      setBusy(false);
    }
  };
  const applyAll = () => applyAgents(agents.map((a) => a.id));

  /** Env switch with pending changes: ask which to apply, discard the rest, then switch. */
  const [envAsk, setEnvAsk] = useState<string | null>(null);
  const doSwitch = async (id: string) => {
    setSwitching(true);
    try {
      await api.setEnv(id);
      setDrafts({});
      setPicked({});
      setRails({});
      setHubSel(null);
      setAgents(await api.listAgents());
      const list = await api.listEnvs();
      setEnvs(list);
      flash(`已切换到 ${list.find((e) => e.current)?.label ?? id}`);
    } catch (e) {
      flash(`切换失败：${e}`, true, 7000);
    } finally {
      setSwitching(false);
    }
  };
  const switchEnv = (id: string) => {
    if (pendingTotal) setEnvAsk(id);
    else doSwitch(id);
  };
  const confirmSwitch = async (apply: AgentId[]) => {
    const id = envAsk!;
    if (apply.length && !(await applyAgents(apply))) return;
    setEnvAsk(null);
    await doSwitch(id);
  };

  // ------------------------------------------------------------ hub actions
  const hubAdd = (s: Service, to: AgentId) => {
    const op = importOp(s, to);
    if (!op) {
      flash("这个供应商没有可复制的地址和密钥，先点「编辑」补全", true);
      return;
    }
    setDraftFor(to, withOp(drafts[to] ?? {}, importKey(s), op));
  };
  const hubRemove = (u: Use) => {
    if (!u.p) return;
    setDraftFor(u.agent.id, withOp(drafts[u.agent.id] ?? {}, keys.deleteProvider(u.p.id), { op: "delete_provider", provider: u.p.id }));
  };
  const hubUndo = (u: Use) => {
    const k = u.importKey ?? (u.state === "new" ? u.p?.draftKey : u.p ? keys.deleteProvider(u.p.id) : undefined);
    if (k) setDraftFor(u.agent.id, withOp(drafts[u.agent.id] ?? {}, k, null));
  };
  const hubDelete = async (s: Service, uses: Use[], fromLib: boolean) => {
    setDrafts((all) => {
      const next = { ...all };
      for (const u of uses) {
        if (u.p) next[u.agent.id] = withOp(next[u.agent.id] ?? {}, keys.deleteProvider(u.p.id), { op: "delete_provider", provider: u.p.id });
      }
      return next;
    });
    if (fromLib && s.lib) {
      await api.libraryDelete(s.lib.id).catch((e) => flash(String(e), true));
      await reloadLib();
    }
    flash(uses.length ? `已把 ${uses.length} 处删除加入待写入${fromLib ? "，并从供应商库移除" : ""}` : "已从供应商库移除");
  };
  const hubSave = async (v: ServiceSave) => {
    const s = hubDialog ?? null;
    const src = s?.uses.find((u) => u.p && u.p.editable && !u.p.isNew);
    const entry = await api.librarySave({
      id: s?.lib?.id ?? null, name: v.name, baseUrl: v.baseUrl, api: v.api, apiKey: v.apiKey, models: v.models,
      adoptFrom: src ? [src.agent.id, src.p!.id] : null,
    });
    setDrafts((all) => {
      const next = { ...all };
      for (const u of v.sync) {
        next[u.agent.id] = upsertProvider(next[u.agent.id] ?? {}, { id: u.p!.id, name: u.p!.name, baseUrl: v.baseUrl, api: u.p!.api, apiKey: v.apiKey, models: [] });
      }
      for (const a of v.addTo) {
        next[a] = withOp(next[a] ?? {}, `pi:library:${entry.id}`, { op: "import_provider", fromAgent: "library", provider: entry.id, api: a === "codex" ? "responses" : v.api, name: v.name });
      }
      return next;
    });
    await reloadLib();
    setHubDialog(undefined);
    setHubSel(hostKey(v.baseUrl));
    const n = v.sync.length + v.addTo.length;
    flash(n ? `已保存到供应商库，${n} 处 Agent 改动已加入待写入` : "已保存到供应商库");
  };

  // Hub details close on outside click / Esc, like the agent page.
  useEffect(() => {
    if (page !== "providers" || !hubSel || hubDialog !== undefined || palette) return;
    const onDown = (e: MouseEvent) => {
      const t = e.target as Element | null;
      if (!t?.closest(".hcard, .acct, .sdetail, .toast, .modal-bg, .aside-diff, .aside-foot")) setHubSel(null);
    };
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") setHubSel(null); };
    document.addEventListener("mousedown", onDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [page, hubSel, hubDialog, palette]);

  const openAgent = (id: AgentId) => {
    setPage(null);
    setSelected(id);
  };

  const goTo = (t: Target) => {
    if (t.kind === "page") {
      setPage(t.page);
      return;
    }
    openAgent(t.agent);
    if (t.tab) setTabs((m) => ({ ...m, [t.agent]: t.tab! }));
    if (t.provider && t.tab === "prov") setPicked((m) => ({ ...m, [t.agent]: t.provider! }));
    if (t.provider && t.tab === "models") setRails((m) => ({ ...m, [t.agent]: t.provider! }));
    setSessionQuery(t.tab === "sessions" ? t.query : undefined);
    if (t.setting) {
      window.setTimeout(() => {
        const el = document.getElementById(`setting-${t.setting}`);
        el?.scrollIntoView({ block: "center", behavior: "smooth" });
        el?.classList.add("flash-row");
        window.setTimeout(() => el?.classList.remove("flash-row"), 1600);
      }, 80);
    }
  };

  const saveProvider = (input: ProviderInput, draftKey?: string) => {
    setDraft(upsertProvider(draft, input, draftKey));
    setDialog(null);
    flash(input.id ? "已加入待写入，点「应用」保存" : "新供应商已加入待写入，点「应用」保存");
  };

  const adoptSync = (list: SyncSuggestion[]) => {
    setDrafts((all) => {
      const next = { ...all };
      for (const s of list) next[s.agent] = { ...(next[s.agent] ?? {}), ...Object.fromEntries(s.ops) };
      return next;
    });
    const agentsTouched = [...new Set(list.map((s) => s.agent))];
    if (agentsTouched[0]) openAgent(agentsTouched[0]);
    flash(`已加入 ${agentsTouched.length} 个 Agent 的待写入，逐个确认后应用`);
  };

  return (
    <div className="app">
      <header className="topbar" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region><Icon.logo /><span data-tauri-drag-region>AgentPlus</span></div>
        <button className="search" onClick={() => setPalette(true)}>
          <Icon.search /><span>搜索服务商、模型、设置、会话</span><kbd>Ctrl K</kbd>
        </button>
        <div className="top-right" data-tauri-drag-region>
          <EnvSwitch envs={envs} current={curEnv} switching={switching} onOpen={reloadEnvs} onPick={switchEnv} />
          <button className="icon-btn ghost" aria-label="AgentPlus 设置" title="AgentPlus 设置" aria-pressed={page === "settings"} onClick={() => (page === "settings" ? closeSettings() : openSettings())}><Icon.gear /></button>
        </div>
        <WindowControls />
      </header>

      <div className={`body${page === "settings" ? " solo" : page && page !== "providers" ? " wide" : ""}`}>
        {page !== "settings" && <Sidebar agents={shown} drafts={drafts} selected={page ? null : selected} page={page} onSelect={openAgent} onPage={setPage} />}

        {page === "providers" && (
          <ProvidersHub
            agents={shown}
            services={services}
            latency={latency}
            selected={hubSel}
            onSelect={setHubSel}
            onAdd={() => setHubDialog(null)}
            onTestAll={() => testHub(true)}
            onTestOne={testOne}
            envLabel={curEnv?.label ?? "本机 · Windows"}
          />
        )}
        {page === "providers" && (
          <HubAside
            agents={shown}
            drafts={drafts}
            services={services}
            busy={busy}
            onDiscard={(a) => (a ? setDraftFor(a, {}) : setDrafts({}))}
            onApplyAll={applyAll}
            detail={hubService && (
              <ServiceDetail
                key={hubService.key}
                s={hubService}
                agents={shown}
                latency={hubService.baseUrl ? latency[hubService.baseUrl] : undefined}
                onClose={() => setHubSel(null)}
                onTest={() => { if (hubService.baseUrl) testOne(hubService.baseUrl); }}
                onCopy={(text) => navigator.clipboard.writeText(text).then(() => flash("已复制地址")).catch(() => flash("复制失败", true))}
                onEdit={() => setHubDialog(hubService)}
                onAddTo={(a) => hubAdd(hubService, a)}
                onRemove={hubRemove}
                onUndo={hubUndo}
                onModels={(u) => goTo({ kind: "agent", agent: u.agent.id, tab: "models", provider: u.p?.id })}
                onDelete={(uses, fromLib) => hubDelete(hubService, uses, fromLib)}
              />
            )}
          />
        )}
        {page === "history" && <HistoryPage flash={flash} onChanged={reload} />}
        {page === "sync" && <SyncPage flash={flash} onAdopt={adoptSync} />}
        {page === "settings" && (
          <SettingsPage
            tab={settingsTab}
            setTab={setSettingsTab}
            prefs={prefs}
            setPrefs={setPrefs}
            envs={envs}
            switching={switching}
            onEnv={switchEnv}
            onHistory={() => setPage("history")}
            onClose={closeSettings}
            onAgentsChanged={reload}
            flash={(text, error) => flash(text, error, error ? 7000 : 3600)}
          />
        )}

        {!page && (st ? (
          <AgentPage
            key={st.id}
            st={st}
            draft={draft}
            setDraft={setDraft}
            latency={latency}
            onTestAll={() => testAll(true)}
            onTestOne={testOne}
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
            onAddProvider={() => setDialog({ editing: null })}
            flash={(text, error) => flash(text, error, error ? 7000 : 4000)}
            sessionQuery={sessionQuery}
            onDeclineFixed={declineFixed}
          />
        ) : (
          <main className="page"><div className="empty">{loadError ?? "正在读取配置…"}</div></main>
        ))}

        {!page && st && (
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
                agents={shown}
                latency={pickedProvider.baseUrl ? latency[pickedProvider.baseUrl] : undefined}
                onClose={closeDetail}
                onTest={() => pickedProvider.baseUrl && testOne(pickedProvider.baseUrl)}
                onAction={() => providerAction(pickedProvider)}
                onModels={() => {
                  setRails((m) => ({ ...m, [st.id]: pickedProvider.id }));
                  setTabs((m) => ({ ...m, [st.id]: "models" }));
                }}
                onCopy={(text) => navigator.clipboard.writeText(text).then(() => flash("已复制地址")).catch(() => flash("复制失败", true))}
                onEdit={() => setDialog({ editing: pickedProvider })}
                onDelete={() => setDraft(withOp(draft, keys.deleteProvider(pickedProvider.id), { op: "delete_provider", provider: pickedProvider.id }))}
                onUndo={() => {
                  if (pickedProvider.isNew) {
                    setDraft(withOp(draft, pickedProvider.draftKey!, null));
                    closeDetail();
                  } else {
                    setDraft(withOp(draft, keys.deleteProvider(pickedProvider.id), null));
                  }
                }}
              />
            )}
          />
        )}
      </div>

      {dialog && st && <ProviderDialog st={st} editing={dialog.editing} onSave={saveProvider} onClose={() => setDialog(null)} />}
      {palette && <CommandPalette agents={shown} onGo={goTo} onClose={() => setPalette(false)} />}
      {hubDialog !== undefined && <ServiceDialog agents={shown} service={hubDialog} onSave={hubSave} onClose={() => setHubDialog(undefined)} />}
      {envAsk && (
        <PendingDialog
          title={`切换到 ${envs.find((e) => e.id === envAsk)?.label ?? envAsk} 之前`}
          agents={shown}
          drafts={drafts}
          busy={busy || switching}
          onConfirm={confirmSwitch}
          onCancel={() => setEnvAsk(null)}
        />
      )}
      {toast && <div className={`toast${toast.error ? " error" : ""}`} role="status">{toast.text}</div>}
    </div>
  );
}
