import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { type AgentId, type AgentState, type ApiKind, type ApplyResult, type DiffGroup, type EnvInfo, type GatewayRouteView, type GatewayStatus, type LibEntry, type Op, type ProjectEntry, type ProviderInput, type SyncSuggestion, api, isProjectId } from "./api";
import {
  CATALOG, type Draft, type ViewProvider, currentProvider, deleteModel, deleteProvider, draftAfterWrite, isEnabled, isVisible, keys, opCount, opsToWrite,
  pendingTotal, setModelVisible, shouldAutoRestart, upsertModel, upsertProvider, viewModels, viewProviders, withOp,
} from "./draft";
import { AgentPage, type Tab } from "./components/AgentPage";
import { Aside } from "./components/Aside";
import { CommandPalette, type Target } from "./components/CommandPalette";
import { HistoryPage } from "./components/HistoryPage";
import { AgentIcon, Icon } from "./components/icons";
import type { Latency } from "./components/ProviderCard";
import { ProviderDetail } from "./components/ProviderDetail";
import { ProviderDialog, type ProviderSave } from "./components/ProviderDialog";
import { ProvidersHub } from "./components/ProvidersHub";
import { HubAside } from "./components/HubAside";
import { ServiceDetail } from "./components/ServiceDetail";
import { type ServiceSave, ServiceDialog } from "./components/ServiceDialog";
import { EnvSwitch } from "./components/EnvSwitch";
import { checkUpdate, useUpdate } from "./updater";
import { type SettingsTab, SettingsPage } from "./components/SettingsPage";
import { PendingDialog } from "./components/PendingDialog";
import { type CloseChoice, CloseDialog } from "./components/CloseDialog";
import { type Prefs, applyPrefs, loadPrefs, savePrefs } from "./prefs";
import { t, tn, useLang } from "./i18n";
import {
  API_LABEL, GATEWAY_KEY, type Group, type Station, type Use, apiFor, buildStations, cannotAdd, findRoute, gatewayEntry, gatewayPoolIds, gatewayRouteId,
  hostKey, importKey, importOp, mergeReplaced, movedGatewayUrl, newRouteId, plainRoute,
} from "./services";
import { type Page, Sidebar } from "./components/Sidebar";
import { SyncPage } from "./components/SyncPage";
import { SYNC_ENABLED } from "./features";
import { GatewayPage } from "./components/GatewayPage";
import { GatewayAside } from "./components/GatewayAside";
import { ConfirmHost, ask, askCheck } from "./components/Confirm";
import { ContextMenu, type MenuItem, editableOf, insertText, selectedIn } from "./components/ContextMenu";
import { ProjectHead, ProjectList } from "./components/ProjectsPage";
import { type CopyPick, CopyProviderDialog } from "./components/CopyProviderDialog";
import { type RestartRun, RestartDialog, applyProgress, finishRun, newRun } from "./components/RestartDialog";
import { useDismiss } from "./hooks";
import { inTauri } from "./tauri";
import { scrub, usePrivacy } from "./privacy";
import { copyText, errText } from "./util";
import { joinList } from "./format";

/** Windows-style caption buttons; the system title bar is turned off. */
function WindowControls() {
  if (!inTauri) return null;
  const win = getCurrentWindow();
  const glyph = (d: string) => (
    <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1" aria-hidden="true"><path d={d} /></svg>
  );
  return (
    <div className="winctl">
      <button aria-label={t("app.minimize")} onClick={() => win.minimize()}>{glyph("M0 5.5h10")}</button>
      <button aria-label={t("app.maximize")} onClick={() => win.toggleMaximize()}>{glyph("M0.5 0.5h9v9h-9z")}</button>
      <button aria-label={t("common.close")} className="close" onClick={() => win.close()}>{glyph("M0 0l10 10M10 0L0 10")}</button>
    </div>
  );
}

type Dialog = { editing: ViewProvider | null } | null;
/** Hub dialog: undefined = closed; group null = add (optionally inside a station). */
type HubDialog = { group: Group | null; prefill?: { name: string; baseUrl: string; station: string } } | undefined;

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
  /** The restart shown in the progress dialog; null when closed or sent to the background. */
  const [run, setRun] = useState<RestartRun | null>(null);
  /** The running restart isn't shown in the dialog: report its result as a notice. */
  const runHidden = useRef(false);
  /** The restart in progress was cancelled from its dialog. */
  const runCancelled = useRef(false);
  const [toast, setToast] = useState<{ id: number; text: string; error?: boolean } | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [dialog, setDialog] = useState<Dialog>(null);
  const [palette, setPalette] = useState(false);
  const [sessionQuery, setSessionQuery] = useState<string | undefined>(undefined);
  const [lib, setLib] = useState<LibEntry[]>([]);
  const [envs, setEnvs] = useState<EnvInfo[]>([]);
  const [switching, setSwitching] = useState(false);
  // main.tsx applied these before the first render (nothing sensitive is ever painted).
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
  const [gateway, setGateway] = useState<GatewayStatus | null>(null);
  // Project folders (per-project OpenCode configs): history, the open one, and loaded states.
  const [projects, setProjects] = useState<ProjectEntry[]>([]);
  const [projPath, setProjPath] = useState<string | null>(null);
  const [projStates, setProjStates] = useState<Record<string, AgentState>>({});
  const [copyOpen, setCopyOpen] = useState(false);
  const setPrefs = (p: Prefs) => { setPrefsState(p); savePrefs(p); };
  useEffect(() => applyPrefs(prefs), [prefs.motion, prefs.theme, prefs.lang, prefs.privacy]);
  // The window starts hidden (tauri.conf.json) so the WebView's blank white never shows;
  // reveal it once the first frame is committed with theme and styles in place.
  useEffect(() => { if (inTauri) getCurrentWindow().show().catch(() => {}); }, []);
  // Closing the window (× or Alt+F4) hides it in the tray or quits, per 设置 › 界面 › 关闭窗口时.
  const [closeAsk, setCloseAsk] = useState<((c: CloseChoice | null) => void) | null>(null);
  const prefsRef = useRef(prefs);
  prefsRef.current = prefs;
  useEffect(() => {
    if (!inTauri) return;
    const win = getCurrentWindow();
    let asking = false;
    const unlisten = win.onCloseRequested(async (e) => {
      e.preventDefault();
      let action = prefsRef.current.closeAction;
      if (action === "ask") {
        if (asking) return;
        asking = true;
        // Closed from the taskbar while minimized: bring the window up so the question is seen.
        if (await win.isMinimized()) await win.unminimize();
        await win.setFocus();
        const choice = await new Promise<CloseChoice | null>((resolve) => setCloseAsk(() => resolve));
        asking = false;
        setCloseAsk(null);
        if (!choice) return;
        action = choice.action;
        if (choice.remember) setPrefs({ ...prefsRef.current, closeAction: action });
      }
      if (action === "tray") await win.hide();
      else await api.quitApp();
    });
    return () => { unlisten.then((f) => f()); };
  }, []);
  // Re-render on a language switch, and reload what the backend rendered in the old one.
  const lang = useLang();
  // Re-render everything that masks text when 隐私模式 is switched.
  usePrivacy();

  // Per-agent page state, kept here so the detail panel and search can drive it.
  const [tabs, setTabs] = useState<Record<string, Tab>>({});
  const [rails, setRails] = useState<Record<string, string | null>>({});
  const [picked, setPicked] = useState<Record<string, string | null>>({});

  // Agents that were not found in this environment are not shown anywhere.
  const shown = useMemo(() => agents.filter((a) => a.installed), [agents]);
  /** Detected agents minus the ones hidden in 设置 › Agent 识别 (sidebar, search, agent pages). */
  const listed = useMemo(() => shown.filter((a) => !prefs.hiddenAgents.includes(a.id)), [shown, prefs.hiddenAgents]);
  const agentSt = listed.find((a) => a.id === selected) ?? listed[0];
  useEffect(() => { if (agentSt && agentSt.id !== selected) setSelected(agentSt.id); }, [agentSt?.id]);
  const projEntry = projects.find((p) => p.path === projPath);
  /** A project opened from OpenCode's 项目 tab replaces the OpenCode page until "back". */
  const projSt = !page && agentSt?.id === "opencode" && projEntry ? projStates[projEntry.agent] : undefined;
  /** The page's subject: the selected agent, or the open project's OpenCode config. */
  const st = projSt ?? agentSt;
  const sid = st?.id ?? selected;
  const draft = drafts[sid] ?? {};
  const ops = useMemo(() => Object.values(draft), [draft]);

  /** Each notice has its own id: an older one's timer never clears a newer one, even with the same text. */
  const toastSeq = useRef(0);
  /** A notice at the bottom of the window; errors stay up longer. */
  const flash = useCallback((text: string, error = false, ms = error ? 7000 : 3600) => {
    const id = ++toastSeq.current;
    setToast({ id, text, error });
    window.setTimeout(() => setToast((cur) => (cur?.id === id ? null : cur)), ms);
  }, []);
  /** A notice that stays until the next one (a restart running in the background). */
  const showSticky = (text: string) => setToast({ id: ++toastSeq.current, text });

  // A new release: look once shortly after startup (设置 › 关于 › 启动时检查更新).
  const update = useUpdate();
  useEffect(() => {
    if (!inTauri || !prefs.autoUpdate) return;
    const timer = window.setTimeout(() => {
      checkUpdate(true).then((info) => { if (info) flash(t("app.updateToast", { version: info.version }), false, 9000); });
    }, 4000);
    return () => window.clearTimeout(timer);
  }, []);

  const reload = () => api.listAgents().then(setAgents).catch((e) => setLoadError(errText(e)));
  const reloadLib = () => api.libraryList().then(setLib).catch(() => undefined);
  const reloadEnvs = () => api.listEnvs().then(setEnvs).catch(() => undefined);
  const reloadProjects = () => api.projectsList().then(setProjects).catch(() => undefined);
  /** Re-reads the open project configs (a project closed meanwhile stays closed). */
  const reloadProjStates = () => {
    for (const id of Object.keys(projStates) as AgentId[]) {
      api.getAgent(id).then((s) => setProjStates((m) => (m[id] ? { ...m, [id]: s } : m))).catch(() => undefined);
    }
  };
  const reloadGateway = () => api.gatewayStatus().then(setGateway).catch(() => undefined);
  /** Everything read from the agents' config files (after a language switch, F5 or a rollback). */
  const reloadConfigs = () => { reload(); reloadLib(); reloadProjects(); reloadProjStates(); };
  // Backend-rendered text follows the language too.
  useEffect(() => { reloadConfigs(); reloadEnvs(); }, [lang]);

  // Gateway status: poll while it is on or its page is open.
  const gwOn = !!gateway?.enabled;
  useEffect(() => {
    const load = reloadGateway;
    load();
    if (!gwOn && page !== "gateway") return;
    const timer = window.setInterval(load, page === "gateway" ? 2000 : 5000);
    return () => window.clearInterval(timer);
  }, [gwOn, page, lang]);

  // Say so when the gateway's error breaker pauses a forward (once per trip).
  const tripsSeen = useRef<Set<string> | null>(null);
  useEffect(() => {
    if (!gateway) return;
    const open = gateway.routes.filter((r) => r.breaker?.state === "open");
    const keysNow = new Set(open.map((r) => `${r.id}@${r.breaker!.at}`));
    for (const r of open) {
      if (tripsSeen.current && !tripsSeen.current.has(`${r.id}@${r.breaker!.at}`)) {
        flash(t("app.breakerPaused", { name: r.name, secs: r.breaker!.pausedSecs, reason: r.breaker!.reason ?? t("app.upstreamError") }), true, 9000);
      }
    }
    tripsSeen.current = keysNow;
  }, [gateway]);

  /** The current gateway host first, then earlier ports: addresses there still point at the gateway. */
  const formerKey = (gateway?.formerPorts ?? []).join(",");
  const gatewayHosts = useMemo(() => (gateway ? [gateway.port, ...(gateway.formerPorts ?? [])].map((p) => `127.0.0.1:${p}`) : []), [gateway?.port, formerKey]);
  /** Every agent shown here plus the open project configs: what "apply all" / "discard all" act on. */
  const allStates = useMemo(() => [...shown, ...Object.values(projStates)], [shown, projStates]);
  const stations = useMemo(() => buildStations(shown, drafts, lib, gatewayHosts), [shown, drafts, lib, gatewayHosts, lang]);

  /** Drafts with every agent address at one of `from` (gateway ports) moved to `port`, and how many moved. */
  const moveGatewayPort = (all: Record<string, Draft>, from: number[], port: number) => {
    const next = { ...all };
    let n = 0;
    for (const a of allStates) {
      if (a.readonly) continue;
      let d = next[a.id] ?? {};
      const before = d;
      // Pending edits (new entries, switches to the gateway) keep everything but the address.
      for (const [k, op] of Object.entries(d)) {
        if (op.op !== "upsert_provider") continue;
        const url = movedGatewayUrl(op.provider.baseUrl, from, port);
        if (url) {
          d = withOp(d, k, { ...op, provider: { ...op.provider, baseUrl: url } });
          n++;
        }
      }
      for (const p of a.providers) {
        if (!p.editable || d[keys.upsertProvider(p.id)] || d[keys.deleteProvider(p.id)]) continue;
        const url = movedGatewayUrl(p.baseUrl, from, port);
        if (url) {
          d = upsertProvider(d, { id: p.id, name: p.name, baseUrl: url, api: p.api, apiKey: GATEWAY_KEY, models: [] });
          n++;
        }
      }
      if (d !== before) next[a.id] = d;
    }
    return { next, n };
  };

  // When the gateway's port changes (on the gateway page, or in store.json), agent addresses
  // at the old port stop working: queue them to the new port. On first sight, addresses left
  // at an earlier port (e.g. changed while the app was closed) are picked up the same way.
  const portSeen = useRef<number | null>(null);
  const agentsReady = agents.length > 0;
  useEffect(() => {
    if (!gateway || !agentsReady) return;
    const prev = portSeen.current;
    portSeen.current = gateway.port;
    if (prev === gateway.port) return;
    const from = [...(prev === null ? [] : [prev]), ...(gateway.formerPorts ?? [])];
    const { n } = moveGatewayPort(drafts, from, gateway.port);
    if (!n) return;
    setDrafts((all) => moveGatewayPort(all, from, gateway.port).next);
    flash(tn("app.gatewayPortMoved", n, { port: gateway.port }), false, 8000);
  }, [gateway?.port, agentsReady]);
  const hubStation = stations.find((s) => s.key === hubSel) ?? null;
  const totalPending = pendingTotal(drafts);
  const curEnv = envs.find((e) => e.current);

  // Ctrl+K opens search from anywhere; Ctrl+Shift+H switches 隐私模式.
  const togglePrivacy = () => {
    const next = { ...prefsRef.current, privacy: !prefsRef.current.privacy };
    setPrefs(next);
    flash(t(next.privacy ? "app.privacyOnToast" : "app.privacyOffToast"));
  };
  // F5 / Ctrl+R would reload the webview and lose pending changes: re-read configs instead.
  const refreshRef = useRef(() => {});
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "k") {
        e.preventDefault();
        setPalette(true);
      } else if ((e.ctrlKey || e.metaKey) && e.shiftKey && e.key.toLowerCase() === "h") {
        e.preventDefault();
        togglePrivacy();
      } else if (e.key === "F5" || ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "r")) {
        e.preventDefault();
        refreshRef.current();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  // Live diff preview for the selected agent.
  useEffect(() => {
    if (!st) return;
    if (ops.length === 0) {
      // `ops` is a fresh [] on every render while there is no draft; keep the same array so this doesn't loop.
      setDiff((d) => (d.length ? [] : d));
      setDiffError(null);
      return;
    }
    let alive = true;
    api.preview(st.id, opsToWrite(st, draft))
      .then((d) => { if (alive) { setDiff(d); setDiffError(null); } })
      .catch((e) => { if (alive) { setDiff([]); setDiffError(errText(e)); } });
    return () => { alive = false; };
  }, [st, ops]);

  const testOne = (url: string) => {
    setLatency((l) => ({ ...l, [url]: "pending" }));
    api.testLatency(url)
      .then((ms) => setLatency((l) => ({ ...l, [url]: ms })))
      .catch((e) => setLatency((l) => ({ ...l, [url]: errText(e) })));
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
    for (const u of new Set(stations.filter((s) => s.baseUrl).map((s) => s.baseUrl!))) {
      if (force || latency[u] === undefined) testOne(u);
    }
  }, [stations, latency]);
  useEffect(() => { if (prefs.autoLatency && page === "providers") testHub(false); }, [page, stations.length]);

  /** Drafts being written right now, per agent. */
  const inFlight = useRef<Record<string, Draft>>({});
  const setDraftFor = (agent: string, d: Draft) => {
    // Undoing a change that is being written would be lost: the file gets it anyway, and
    // afterwards no pending op would be left to show or revert it.
    const sending = inFlight.current[agent];
    const cur = drafts[agent] ?? {};
    if (sending && Object.keys(sending).some((k) => k in cur && cur[k] === sending[k] && !(k in d))) {
      flash(t("app.undoWhileWriting"), true);
      return;
    }
    setDrafts((all) => ({ ...all, [agent]: d }));
  };
  const setDraft = (d: Draft) => setDraftFor(sid, d);

  // Codex's fixed id is on by default but never a pending change of its own
  // (see opsToWrite). Declining it is remembered by the backend.
  const declineFixed = async () => {
    try {
      await api.dismissFixedPrompt();
      replaceAgent(await api.getAgent("codex"));
    } catch (e) {
      flash(errText(e), true);
      return;
    }
    flash(t("app.fixedDeclined"));
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

  const shownProviders = st ? viewProviders(st, draft, isProjectId(st.id)) : [];
  const pickedProvider = shownProviders.find((p) => p.id === picked[sid]) ?? null;
  const closeDetail = () => setPicked((m) => ({ ...m, [sid]: null }));

  // Clicking outside the cards / detail panel, or pressing Esc, closes the details.
  useDismiss(!!pickedProvider && !dialog && !palette && !copyOpen, ".pcard, .pdetail, .toast, .modal-bg", closeDetail);

  const askDeleteProvider = async (p: ViewProvider) => {
    if (!st) return;
    if (!(await ask({ title: t("app.deleteProviderTitle", { name: p.name }), message: t("app.deleteProviderMsg", { agent: st.name }), danger: true }))) return;
    setDraft(deleteProvider(draft, p.id));
  };

  const replaceAgent = (next: AgentState) => (isProjectId(next.id)
    ? setProjStates((m) => ({ ...m, [next.id]: next }))
    : setAgents((list) => list.map((a) => (a.id === next.id ? next : a))));

  /** Restarts an agent, or starts it when it isn't running; progress as chosen in 设置 › 界面. */
  const restartAgent = async (a: AgentState) => {
    const starting = !a.running;
    const showDialog = prefs.restartProgress === "dialog";
    setRestarting(a.id);
    runHidden.current = !showDialog;
    runCancelled.current = false;
    if (showDialog) setRun(newRun(a.id, a.name, starting));
    else showSticky(t(starting ? "app.starting" : "app.restarting", { name: a.name }));
    const mine = (f: (r: RestartRun) => RestartRun) => setRun((r) => (r && r.agent === a.id && !r.result ? f(r) : r));
    try {
      const msg = await api.restart(a.id, showDialog ? (p) => mine((r) => applyProgress(r, p)) : undefined);
      mine((r) => finishRun(r, true, msg));
      if (runHidden.current) flash(t("app.nameMsg", { name: a.name, msg }), false, 5000);
      replaceAgent(await api.getAgent(a.id));
    } catch (e) {
      mine((r) => finishRun(r, false, errText(e)));
      if (runCancelled.current) flash(t(starting ? "app.startCancelled" : "app.restartCancelled", { name: a.name }));
      else if (runHidden.current) flash(t(starting ? "app.startFailed" : "app.restartFailed", { name: a.name, err: errText(e) }), true, 8000);
    } finally {
      setRestarting(null);
    }
  };
  /** Restart / Start clicked: unapplied changes won't be read, so offer to apply them first. */
  const restartAsked = async (a: AgentState) => {
    const n = opCount(drafts[a.id]);
    if (n) {
      const starting = !a.running;
      const applyFirst = await askCheck({
        title: tn("app.restartPendingTitle", n, { name: a.name }),
        message: t(starting ? "app.startPendingMsg" : "app.restartPendingMsg", { name: a.name }),
        confirmText: t(starting ? "common.startAgent" : "common.restartAgent", { name: a.name }),
        check: { label: t("app.applyFirst"), hint: t("app.applyFirstHint"), value: true },
      });
      if (applyFirst === null) return;
      if (applyFirst && !(await applyAgents([a.id], false))) return;
    }
    await restartAgent(a);
  };
  const restart = () => { if (st) restartAsked(st); };
  /** Closes the dialog; while the restart still runs it carries on with a notice instead. */
  const closeRun = () => {
    if (run && !run.result) {
      runHidden.current = true;
      showSticky(t(run.starting ? "app.starting" : "app.restarting", { name: run.name }));
    }
    setRun(null);
  };
  /** Cancels the running restart: the dialog closes and a notice follows once it has stopped. */
  const cancelRun = () => {
    if (!run || run.result) return;
    runCancelled.current = true;
    runHidden.current = true;
    showSticky(t("app.cancelling"));
    setRun(null);
    api.cancelRestart().catch(() => undefined);
  };

  // Keep the Start / Restart button honest: the app can be closed or opened outside AgentPlus.
  const pollRunning = !page && st?.restartable && !restarting ? st.id : null;
  useEffect(() => {
    if (!pollRunning) return;
    const check = () => {
      if (document.hidden) return;
      api.agentRunning(pollRunning)
        // Same list when nothing changed, so everything that depends on `agents` stays put.
        .then((running) => setAgents((list) => (list.some((a) => a.id === pollRunning && a.running !== running)
          ? list.map((a) => (a.id === pollRunning ? { ...a, running } : a))
          : list)))
        .catch(() => undefined);
    };
    check();
    const timer = window.setInterval(check, 4000);
    window.addEventListener("focus", check);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", check);
    };
  }, [pollRunning]);

  /**
   * Writes `sent` (a snapshot of the agent's draft; `ops` is what goes to its files). Edits made
   * meanwhile stay pending. Then the optional auto-restart (off by default), when something
   * the agent reads changed.
   */
  const writeAgent = async (a: AgentState, sent: Draft, ops: Op[] = opsToWrite(a, sent), autoRestart = true): Promise<ApplyResult> => {
    inFlight.current[a.id] = sent;
    let r: ApplyResult;
    try {
      r = await api.apply(a.id, ops);
    } finally {
      delete inFlight.current[a.id];
    }
    replaceAgent(r.state);
    setDrafts((all) => ({ ...all, [a.id]: draftAfterWrite(all[a.id] ?? {}, sent) }));
    // The project list shows each project's provider count and whether its config exists.
    if (isProjectId(a.id)) reloadProjects();
    if (autoRestart && shouldAutoRestart(r.state, ops)) await restartAgent(r.state);
    return r;
  };

  /** Notice after writing several agents: all of them, or which ones were written before `err`. */
  const reportBatch = (done: string[], err: string | null = null) => {
    if (err !== null) flash(done.length ? t("app.wrotePartial", { names: joinList(done), err }) : err, true, 8000);
    else if (done.length) flash(t("app.wroteAgents", { names: joinList(done) }));
  };

  const apply = async () => {
    if (!st) return;
    setBusy(true);
    const sentOps = opsToWrite(st, draft);
    try {
      // The notice comes before the restart's own.
      const r = await writeAgent(st, draft, sentOps, false);
      setPicked((m) => ({ ...m, [st.id]: null }));
      flash(r.files.length ? tn("app.wroteFiles", r.files.length) : t("app.saved"));
      if (shouldAutoRestart(r.state, sentOps)) await restartAgent(r.state);
    } catch (e) {
      flash(errText(e), true);
    } finally {
      setBusy(false);
    }
  };

  /** Applies the given agents' pending changes one by one; true when all succeeded. */
  const applyAgents = async (ids: AgentId[], autoRestart = true): Promise<boolean> => {
    setBusy(true);
    const done: string[] = [];
    try {
      for (const a of allStates) {
        if (!ids.includes(a.id) || !opCount(drafts[a.id])) continue;
        await writeAgent(a, drafts[a.id], undefined, autoRestart)
          .catch((e) => { throw new Error(t("app.nameMsg", { name: a.name, msg: errText(e) })); });
        done.push(a.name);
      }
      reportBatch(done);
      return true;
    } catch (e) {
      reportBatch(done, errText(e));
      return false;
    } finally {
      setBusy(false);
    }
  };
  const applyAll = () => applyAgents(allStates.map((a) => a.id));

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
      setProjStates({});
      setProjPath(null);
      reloadProjects();
      setAgents(await api.listAgents());
      const list = await api.listEnvs();
      setEnvs(list);
      flash(t("app.switchedTo", { env: list.find((e) => e.current)?.label ?? id }));
    } catch (e) {
      flash(t("app.switchFailed", { err: errText(e) }), true);
    } finally {
      setSwitching(false);
    }
  };
  const switchEnv = (id: string) => {
    if (totalPending) setEnvAsk(id);
    else doSwitch(id);
  };
  const confirmSwitch = async (apply: AgentId[]) => {
    const id = envAsk!;
    if (apply.length && !(await applyAgents(apply))) return;
    setEnvAsk(null);
    await doSwitch(id);
  };

  // ------------------------------------------------------------ projects
  /** Opens a project folder: records it in the history and loads its OpenCode config. */
  const openProject = async (path: string) => {
    try {
      const e = await api.projectOpen(path);
      const s = await api.getAgent(e.agent);
      setProjStates((m) => ({ ...m, [e.agent]: s }));
      setPage(null);
      setSelected("opencode");
      setProjPath(e.path);
      reloadProjects();
    } catch (err) {
      flash(errText(err), true);
    }
  };
  const pickProject = async () => {
    try {
      const p = await api.pickFolder(projEntry?.path ?? projects[0]?.path ?? null);
      if (p) await openProject(p);
    } catch (err) {
      flash(t("app.pickFolderFailed", { err: errText(err) }), true);
    }
  };
  const forgetProject = async (p: ProjectEntry) => {
    const n = opCount(drafts[p.agent]);
    if (n && !(await ask({ title: t("app.forgetTitle", { name: p.name }), message: tn("app.forgetMsg", n), danger: true, confirmText: t("common.remove") }))) return;
    try {
      await api.projectForget(p.path);
    } catch (e) {
      // Still in the list: keep its pending changes too.
      flash(errText(e), true);
      return;
    }
    setDraftFor(p.agent, {});
    if (p.path === projPath) setProjPath(null);
    reloadProjects();
  };
  /** Copy dialog → pending imports (the backend resolves address and key). */
  const copyToProject = (picks: CopyPick[]) => {
    if (!st) return;
    let d = drafts[st.id] ?? {};
    for (const x of picks) {
      d = withOp(d, keys.importProvider(x.fromAgent, x.provider), { op: "import_provider", fromAgent: x.fromAgent, provider: x.provider, api: x.api, name: x.name, label: x.label });
      if (x.disableInherited) d = withOp(d, keys.enabled(x.provider), { op: "set_provider_enabled", provider: x.provider, enabled: false });
    }
    setDraftFor(st.id, d);
    setCopyOpen(false);
    flash(tn("app.copyQueued", picks.length));
  };
  // ------------------------------------------------------------ hub actions
  const hubAdd = (g: Group, to: AgentId) => {
    const op = importOp(g, to);
    if (!op) {
      flash(cannotAdd(g, to) ?? t("app.cannotAdd"), true);
      return;
    }
    setDraftFor(to, withOp(drafts[to] ?? {}, importKey(g), op));
  };
  const hubRemove = async (u: Use) => {
    if (!u.p) return;
    if (!(await ask({ title: t("app.hubRemoveTitle", { agent: u.agent.name, name: u.p.name }), message: t("app.hubRemoveMsg"), danger: true, confirmText: t("common.remove") }))) return;
    setDraftFor(u.agent.id, deleteProvider(drafts[u.agent.id] ?? {}, u.p.id));
  };
  const hubUndo = (u: Use) => {
    const k = u.importKey ?? (u.state === "new" ? u.p?.draftKey : u.p ? keys.deleteProvider(u.p.id) : undefined);
    if (k) setDraftFor(u.agent.id, withOp(drafts[u.agent.id] ?? {}, k, null));
  };
  const hubDelete = async (s: Group, uses: Use[], fromLib: boolean) => {
    const parts = [
      ...uses.map((u) => t("app.hubDeleteUse", { agent: u.agent.name, name: u.p?.name ?? "" })),
      ...(fromLib && s.lib ? [t("app.hubDeleteLib")] : []),
    ];
    if (!(await ask({ title: t("app.deleteGroupTitle", { name: s.name }), message: <ul className="confirm-list">{parts.map((p) => <li key={p}>{p}</li>)}</ul>, danger: true }))) return;
    setDrafts((all) => {
      const next = { ...all };
      for (const u of uses) {
        if (u.p) next[u.agent.id] = deleteProvider(next[u.agent.id] ?? {}, u.p.id);
      }
      return next;
    });
    if (fromLib && s.lib) {
      try {
        await api.libraryDelete(s.lib.id);
      } catch (e) {
        // The agent deletes are queued; only the library entry is still there.
        flash(uses.length ? tn("app.queuedDeletesLibFailed", uses.length, { err: errText(e) }) : errText(e), true);
        return;
      }
      await reloadLib();
    }
    flash(uses.length ? tn(fromLib ? "app.queuedDeletesAndLib" : "app.queuedDeletes", uses.length) : t("app.removedFromLib"));
  };
  /** Library entries the open hub dialog has already created (a failed save can be retried). */
  const hubSaved = useRef<{ main: string | null; alt: Partial<Record<ApiKind, string>> }>({ main: null, alt: {} });
  useEffect(() => { hubSaved.current = { main: null, alt: {} }; }, [hubDialog]);
  const hubSave = async (v: ServiceSave) => {
    const s = hubDialog?.group ?? null;
    const src = s?.uses.find((u) => u.p && u.p.editable && !u.p.isNew);
    // Entries saved by an earlier attempt of this dialog are updated, not added again.
    const saved = hubSaved.current;
    const entry = await api.librarySave({
      id: s?.lib?.id ?? saved.main, name: v.name, baseUrl: v.baseUrl, api: v.api, apiKey: v.apiKey, models: v.models,
      adoptFrom: src ? [src.agent.id, src.p!.id] : null,
    });
    saved.main = entry.id;
    // Template: agents that need another protocol get the same key at that protocol's address.
    const alts: { e: LibEntry; addTo: AgentId[] }[] = [];
    for (const a of v.alt) {
      const e = await api.librarySave({ id: saved.alt[a.api] ?? null, name: t("app.libAltName", { name: v.name, api: API_LABEL[a.api] }), baseUrl: a.baseUrl, api: a.api, apiKey: v.apiKey, models: v.models, adoptFrom: null });
      saved.alt[a.api] = e.id;
      alts.push({ e, addTo: a.addTo });
    }
    // Through the gateway: one forward for the new entry, and every picked agent points at it.
    let route: GatewayRouteView | null = null;
    if (v.gateway && v.addTo.length) {
      try {
        route = await routeForLib(entry.id, v.api, v.name);
      } catch (e) {
        flash(t("app.savedForwardFailed", { err: errText(e) }), true);
        await reloadLib();
        setHubDialog(undefined);
        return;
      }
    }
    setDrafts((all) => {
      const next = { ...all };
      for (const u of v.sync) {
        // The whole group moves to the new protocol, except in agents that speak only one (ONLY_API): they keep theirs.
        next[u.agent.id] = upsertProvider(next[u.agent.id] ?? {}, { id: u.p!.id, name: u.p!.name, baseUrl: v.baseUrl, api: apiFor(u.agent.id, v.api), apiKey: v.apiKey, models: [] });
      }
      for (const a of v.addTo) {
        next[a] = route
          ? upsertProvider(next[a] ?? {}, gatewayEntry(route.localBase, a, apiFor(a, v.api), t("app.gatewayName", { name: v.name }), v.models), keys.gatewayProvider(route.id))
          : withOp(next[a] ?? {}, keys.importProvider("library", entry.id), { op: "import_provider", fromAgent: "library", provider: entry.id, api: v.api, name: v.name });
      }
      for (const { e, addTo } of alts) {
        for (const a of addTo) next[a] = withOp(next[a] ?? {}, keys.importProvider("library", e.id), { op: "import_provider", fromAgent: "library", provider: e.id, api: e.api, name: v.name });
      }
      return next;
    });
    await reloadLib();
    setHubDialog(undefined);
    setHubSel(hostKey(v.baseUrl));
    const n = v.sync.length + v.addTo.length + alts.reduce((k, x) => k + x.addTo.length, 0);
    flash(n ? (route ? tn("app.savedViaQueued", n, { url: route.localBase }) : tn("app.savedQueued", n)) : t("app.savedToLib"));
  };

  /** Turns the gateway on when it is off; returns its status. */
  const ensureGateway = async () => {
    let s = await api.gatewayStatus();
    if (!s.running) s = await api.gatewaySet(true, null);
    setGateway(s);
    return s;
  };

  /** Library id for a group, adopting it (with its key) when it only lives in an agent. */
  const ensureLibrary = async (g: Group): Promise<string> => {
    if (g.lib) return g.lib.id;
    const src = g.uses.find((u) => u.p && u.p.editable && !u.p.isNew);
    if (!src) throw new Error(t("app.groupNoSource"));
    const e = await api.librarySave({ id: null, name: g.name, baseUrl: g.baseUrl, api: g.api, apiKey: null, models: null, adoptFrom: [src.agent.id, src.p!.id] });
    await reloadLib();
    return e.id;
  };

  /**
   * The gateway route that forwards to a group: reuses one with the same library entry
   * and protocol, or creates it; turns the gateway on when it is off.
   */
  const routeFor = async (g: Group): Promise<GatewayRouteView> => routeForLib(await ensureLibrary(g), g.api, g.name);
  const routeForLib = async (libId: string, upstreamApi: ApiKind, name: string): Promise<GatewayRouteView> => {
    let gs = await api.gatewayStatus();
    let r = findRoute(gs.routes, libId, upstreamApi);
    if (!r) {
      const id = newRouteId(name, gs.routes);
      gs = await api.gatewaySaveRoute({ id, name, library: libId, upstreamApi, modelMap: [], enabled: true, weight: 100 }, null);
      r = gs.routes.find((x) => x.id === id)!;
    } else if (!r.enabled) {
      gs = await api.gatewaySaveRoute({ ...plainRoute(r), enabled: true }, r.id);
    }
    if (!gs.running) gs = await api.gatewaySet(true, null);
    setGateway(gs);
    return gs.routes.find((x) => x.id === r!.id)!;
  };

  /** Hub: add a group to an agent through the gateway (works for any protocol pair). */
  const viaGateway = async (g: Group, agent: AgentId) => {
    try {
      const r = await routeFor(g);
      gatewayToAgent(r, agent, apiFor(agent, g.api));
    } catch (e) {
      flash(t("app.forwardFailed", { err: errText(e) }), true);
    }
  };

  /**
   * Gateway page: forward a group; with `replace`, the agent entries using its address are
   * switched to the forward (pending changes) and remembered on the route for restoring.
   */
  const forwardGroup = async (g: Group, replace: boolean): Promise<GatewayRouteView> => {
    const r = await routeFor(g);
    if (!replace) return r;
    const targets = g.uses.filter((u) => u.p && u.p.editable && !u.p.isNew && !u.p.isDeleted && u.p.baseUrl && gatewayRouteId(u.p.baseUrl, gatewayHosts) === null);
    if (!targets.length) return r;
    setDrafts((all) => {
      const next = { ...all };
      for (const u of targets) {
        const p = u.p!;
        next[u.agent.id] = upsertProvider(next[u.agent.id] ?? {}, {
          id: p.id, name: p.name, baseUrl: r.localBase, api: apiFor(u.agent.id, p.api), apiKey: GATEWAY_KEY, models: [],
        });
      }
      return next;
    });
    const replaced = mergeReplaced(r.replaced, targets.map((u) => [u.agent.id, u.p!.id] as [string, string]));
    const gs = await api.gatewaySaveRoute({ ...plainRoute(r), replaced }, r.id);
    setGateway(gs);
    return gs.routes.find((x) => x.id === r.id) ?? r;
  };

  /** Drafts with the entries a forward replaced pointed back at its upstream, and how many there were. */
  const restoreReplaced = (all: Record<string, Draft>, r: GatewayRouteView) => {
    const e = lib.find((x) => x.id === r.library);
    let n = 0;
    const next = { ...all };
    for (const [aid, pid] of r.replaced ?? []) {
      const a = shown.find((x) => x.id === aid);
      if (!a) continue;
      const d = next[aid] ?? {};
      const p = viewProviders(a, d).find((x) => x.id === pid && !x.isDeleted);
      if (!p || gatewayRouteId(p.baseUrl, gatewayHosts) !== r.id) continue;
      const applied = a.providers.find((x) => x.id === pid);
      if (applied && gatewayRouteId(applied.baseUrl, gatewayHosts) !== r.id) {
        // The switch was never applied: dropping the pending change is enough.
        next[aid] = withOp(d, keys.upsertProvider(pid), null);
      } else if (e) {
        next[aid] = upsertProvider(d, { id: pid, name: p.name, baseUrl: e.baseUrl, api: e.api, apiKey: null, models: [], keyFromLibrary: e.id });
      } else continue;
      n++;
    }
    return { next, n };
  };

  /** Deletes a forward after confirming; offers to restore the addresses it replaced. */
  const deleteRoute = async (r: GatewayRouteView): Promise<boolean> => {
    const title = t("app.deleteRouteTitle", { name: r.name });
    const message = t("app.deleteRouteMsg");
    let restore = false;
    if (r.replaced?.length) {
      const v = await askCheck({
        title, message, danger: true,
        check: { label: t("app.restoreLabel"), hint: tn("app.restoreHint", r.replaced.length, { url: r.upstreamUrl ?? "" }), value: true },
      });
      if (v === null) return false;
      restore = v;
    } else if (!(await ask({ title, message, danger: true }))) return false;
    try {
      setGateway(await api.gatewayDeleteRoute(r.id));
    } catch (e) {
      flash(errText(e), true);
      return false;
    }
    const n = restore ? restoreReplaced(drafts, r).n : 0;
    if (n) setDrafts((all) => restoreReplaced(all, r).next);
    flash(n ? tn("app.routeDeletedRestored", n) : t("app.routeDeleted"));
    return true;
  };

  /** Gateway route an agent's provider points at: undefined = direct; null = route no longer exists. */
  const routeOfProvider = (p: ViewProvider | null): GatewayRouteView | null | undefined => {
    const id = p ? gatewayRouteId(p.baseUrl, gatewayHosts) : null;
    return id === null ? undefined : gateway?.routes.find((r) => r.id === id) ?? null;
  };

  /** Gateway route → a provider in an agent that points at the local address. */
  const gatewayToAgent = (r: GatewayRouteView, agent: AgentId, apiKind: ApiKind) => {
    const models = lib.find((e) => e.id === r.library)?.models ?? [];
    setDraftFor(agent, upsertProvider(drafts[agent] ?? {}, gatewayEntry(r.localBase, agent, apiKind, t("app.gatewayName", { name: r.name }), models), keys.gatewayProvider(r.id)));
    flash(t("app.queuedForAgent", { agent: shown.find((a) => a.id === agent)?.name ?? agent, url: r.localBase }));
  };

  /**
   * Agent entries that point at the gateway without that agent's own key: written before
   * per-agent keys (the shared "agentplus-gateway"), copied from another agent, or keyless.
   */
  const staleGatewayKeys = useMemo(() => {
    if (!gateway) return [];
    const out: { agent: AgentState; p: ViewProvider }[] = [];
    for (const a of allStates) {
      if (a.readonly) continue;
      // Gateway keys are per agent: a project config uses its agent's.
      const own = gateway.keyFps[a.id.split("@")[0]];
      for (const p of viewProviders(a, drafts[a.id] ?? {})) {
        if (p.isDeleted || p.isNew || !p.editable || !p.baseUrl) continue;
        if (gatewayRouteId(p.baseUrl, gatewayHosts) === null && gatewayPoolIds(p.baseUrl) === null) continue;
        // A pending edit with a key has no fingerprint yet (keyFp null): not stale.
        if (!p.hasKey || (p.keyFp !== null && p.keyFp !== own)) out.push({ agent: a, p });
      }
    }
    return out;
  }, [gateway, allStates, drafts, gatewayHosts]);

  /**
   * Lists those entries, then writes each with the gateway key placeholder (the backend fills in
   * that agent's key) right away. Other pending edits of the same agents stay pending; a pending
   * edit of the same provider is written along with the key.
   */
  const updateGatewayKeys = async () => {
    const stale = staleGatewayKeys;
    if (!stale.length) return;
    const ok = await ask({
      title: tn("app.gatewayKeysTitle", stale.length),
      message: (
        <>
          {t("app.gatewayKeysMsg")}
          <ul className="confirm-list">
            {stale.map(({ agent, p }) => <li key={`${agent.id}|${p.id}`}>{t("app.gatewayKeysItem", { agent: agent.name, provider: p.name })}</li>)}
          </ul>
        </>
      ),
      confirmText: t("app.gatewayKeysConfirm"),
    });
    if (!ok) return;
    // Per agent: the ops to write, and the pending ops they include (only those stop being pending).
    const byAgent = new Map<AgentId, { agent: AgentState; ops: Op[]; sent: Draft }>();
    for (const { agent, p } of stale) {
      const key = keys.upsertProvider(p.id);
      const pending = drafts[agent.id]?.[key];
      const input: ProviderInput = pending?.op === "upsert_provider"
        ? { ...pending.provider, apiKey: GATEWAY_KEY }
        : { id: p.id, name: p.name, baseUrl: p.baseUrl!, api: p.api, apiKey: GATEWAY_KEY, models: [] };
      const e = byAgent.get(agent.id) ?? { agent, ops: [], sent: {} };
      e.ops.push({ op: "upsert_provider", provider: input });
      if (pending) e.sent[key] = pending;
      byAgent.set(agent.id, e);
    }
    setBusy(true);
    const done: string[] = [];
    try {
      for (const { agent, ops, sent } of byAgent.values()) {
        await writeAgent(agent, sent, ops)
          .catch((e) => { throw new Error(t("app.nameMsg", { name: agent.name, msg: errText(e) })); });
        done.push(agent.name);
      }
      reportBatch(done);
    } catch (e) {
      reportBatch(done, errText(e));
    } finally {
      setBusy(false);
    }
  };

  // Hub details close on outside click / Esc, like the agent page.
  useDismiss(page === "providers" && !!hubSel && hubDialog === undefined && !palette, ".hcard, .acct, .sdetail, .toast, .modal-bg, .aside-diff, .aside-foot", () => setHubSel(null));

  const openAgent = (id: AgentId) => {
    setPage(null);
    setSelected(id);
    setProjPath(null);
  };

  const goTo = (target: Target) => {
    if (target.kind === "page") {
      if (target.page === "settings") {
        if (target.settingsTab) setSettingsTab(target.settingsTab);
        openSettings();
      } else setPage(target.page);
      return;
    }
    const { agent, tab, provider, setting } = target;
    openAgent(agent);
    if (tab) setTabs((m) => ({ ...m, [agent]: tab }));
    if (provider && tab === "prov") setPicked((m) => ({ ...m, [agent]: provider }));
    if (provider && tab === "models") setRails((m) => ({ ...m, [agent]: provider }));
    setSessionQuery(tab === "sessions" ? target.query : undefined);
    if (setting) {
      window.setTimeout(() => {
        const el = document.getElementById(`setting-${setting}`);
        el?.scrollIntoView({ block: "center", behavior: "smooth" });
        el?.classList.add("flash-row");
        window.setTimeout(() => el?.classList.remove("flash-row"), 1600);
      }, 80);
    }
  };

  /** Library entry the open provider dialog has already created (template through a forward). */
  const dialogSaved = useRef<{ id: string | null }>({ id: null });
  useEffect(() => { dialogSaved.current = { id: null }; }, [dialog]);
  /** Provider dialog on an agent page: edits, connection (direct / gateway) and this agent's model list. */
  const saveProvider = async (sv: ProviderSave) => {
    if (!st) return;
    const editing = dialog?.editing ?? null;
    // This open of the dialog: a save that finishes after it was closed (or reopened) is dropped.
    const slot = dialogSaved.current;
    const stale = () => slot !== dialogSaved.current;
    let input: ProviderInput | null = sv.input;
    try {
      if (sv.viaForward) {
        // Template the agent can't reach directly: library entry + gateway forward.
        const f = sv.viaForward;
        // A retry after the forward failed updates the entry saved the first time.
        const e = await api.librarySave({ id: slot.id, name: f.name, baseUrl: f.baseUrl, api: f.api, apiKey: f.apiKey, models: f.models, adoptFrom: null });
        slot.id = e.id;
        await reloadLib();
        const r = await routeForLib(e.id, e.api, e.name);
        input = { ...gatewayEntry(r.localBase, st.id, apiFor(st.id, e.api), f.name, f.models), officialAuth: f.officialAuth };
      }
      if (sv.connect && editing) {
        const base: ProviderInput = input ?? { id: editing.id, name: editing.name, baseUrl: editing.baseUrl ?? "", api: editing.api, apiKey: null, models: [] };
        if (sv.connect === "gateway") {
          // A project config isn't among the hub's agents: group its own entries.
          const pool = isProjectId(st.id) ? buildStations([st], drafts, lib, gatewayHosts) : stations;
          const g = pool.flatMap((x) => x.groups).find((x) => x.uses.some((u) => u.agent.id === st.id && u.p?.id === editing.id));
          if (!g) throw new Error(t("app.noGroupForProvider"));
          const r = await routeFor(g);
          input = { ...base, baseUrl: r.localBase, api: apiFor(st.id, base.api), apiKey: GATEWAY_KEY };
        } else {
          const r = routeOfProvider(editing);
          const e = r ? lib.find((x) => x.id === r.library) : undefined;
          if (!e) throw new Error(t("app.noUpstream"));
          if (apiFor(st.id, e.api) !== e.api) throw new Error(t("app.upstreamGatewayOnly", { api: API_LABEL[e.api], agent: st.name }));
          input = { ...base, baseUrl: e.baseUrl, api: e.api, apiKey: null, keyFromLibrary: e.id };
        }
      }
    } catch (e) {
      flash(errText(e), true);
      return;
    }
    if (sv.unified) {
      try {
        await ensureGateway();
      } catch (e) {
        flash(t("app.gatewayStartFailed", { err: errText(e) }), true);
        return;
      }
    }
    if (stale()) return;
    const agentId = st.id;
    const build = (d0: Draft): Draft => {
      let d = d0;
      if (input) d = upsertProvider(d, input, sv.draftKey);
      if (editing && sv.roles !== undefined) {
        d = withOp(d, keys.roles(editing.id), sv.roles ? { op: "set_model_roles", provider: editing.id, roles: sv.roles } : null);
      }
      if (editing && sv.codexModels !== undefined) {
        d = withOp(d, keys.providerModels(editing.id), sv.codexModels ? { op: "set_provider_models", provider: editing.id, models: sv.codexModels } : null);
      }
      if (editing && sv.models) {
        for (const m of editing.models) {
          const want = sv.models.visible[m.id];
          if (want !== undefined) d = setModelVisible(d, editing.id, m, want);
        }
        for (const id of sv.models.added) d = upsertModel(d, editing.id, { id, name: null, context: null });
      }
      return d;
    };
    // From the latest drafts: other edits may have landed while this save was waiting.
    setDrafts((all) => ({ ...all, [agentId]: build(all[agentId] ?? {}) }));
    setDialog(null);
    flash(t(sv.connect === "gateway" || sv.viaForward ? "app.queuedViaGateway" : sv.connect === "direct" ? "app.queuedDirect" : "app.queuedApply"));
  };

  const adoptSync = (list: SyncSuggestion[]) => {
    setDrafts((all) => {
      const next = { ...all };
      for (const s of list) next[s.agent] = { ...(next[s.agent] ?? {}), ...Object.fromEntries(s.ops) };
      return next;
    });
    const agentsTouched = [...new Set(list.map((s) => s.agent))];
    if (agentsTouched[0]) openAgent(agentsTouched[0]);
    flash(tn("app.queuedAgents", agentsTouched.length));
  };

  // ------------------------------------------------------------ right-click menu
  const refreshAll = () => {
    reloadConfigs();
    reloadGateway();
    flash(t("app.reloaded"));
  };
  refreshRef.current = refreshAll;
  const copy = (s: string) => copyText(s, flash);
  /** Fire-and-forget action: only a failure is reported. */
  const attempt = (p: Promise<unknown>) => { p.catch((e) => flash(errText(e), true)); };
  /** Opens an agent's model list at one provider. */
  const showModels = (agent: string, pid: string) => {
    setRails((m) => ({ ...m, [agent]: pid }));
    setTabs((m) => ({ ...m, [agent]: "models" }));
  };
  /** Add-group dialog inside a station (its name and first address suggested). */
  const addGroupIn = (s: Station) => setHubDialog({ group: null, prefill: { name: `${s.name} — `, baseUrl: s.groups[0]?.baseUrl ?? "", station: s.name } });

  /** Actions for the thing that was right-clicked (agent, provider, model, station, forward, session). */
  const contextItems = (target: Element): MenuItem[] => {
    const el = target.closest("[data-ctx]");
    if (!el) return [];
    const d = (k: string) => el.getAttribute(`data-${k}`) ?? "";
    switch (el.getAttribute("data-ctx")) {
      case "agent": {
        const a = shown.find((x) => x.id === d("agent"));
        if (!a) return [];
        const n = opCount(drafts[a.id]);
        return [
          { label: t("app.openAgent", { name: a.name }), icon: <AgentIcon id={a.id} size={14} />, action: () => openAgent(a.id) },
          ...(a.restartable ? [{ label: t(a.running ? "common.restartAgent" : "common.startAgent", { name: a.name }), icon: a.running ? <Icon.refresh size={13} /> : <Icon.play size={13} />, disabled: !!restarting, action: () => { restartAsked(a); } }] : []),
          { label: t("common.openConfigDir"), icon: <Icon.folder size={13} />, action: () => attempt(api.openConfigDir(a.id)) },
          ...(n ? [
            "sep" as const,
            { label: tn("app.applyAgentChanges", n, { name: a.name }), icon: <Icon.check size={13} />, disabled: busy, action: () => { applyAgents([a.id]); } },
            { label: t("app.discardAgentChanges", { name: a.name }), icon: <Icon.close size={11} />, danger: true, action: async () => {
              if (await ask({ title: tn("app.discardAgentTitle", n, { name: a.name }), message: t("app.discardMsg"), danger: true, confirmText: t("common.discard") })) setDraftFor(a.id, {});
            } },
          ] : []),
          "sep",
        ];
      }
      case "provider": {
        const p = shownProviders.find((x) => x.id === d("pid"));
        if (!p || !st) return [];
        const isCur = st.mode === "single" && currentProvider(st, draft) === p.id;
        const on = isEnabled(p, draft);
        return [
          { label: t("app.viewDetails"), action: () => setPicked((m) => ({ ...m, [st.id]: p.id })) },
          ...(p.editable ? [{ label: t("app.editMenu"), icon: <Icon.edit size={12} />, disabled: st.readonly, action: () => setDialog({ editing: p }) }] : []),
          ...(p.compatible && !p.isNew && !p.isDeleted && !(st.mode === "multi" && p.builtin)
            ? [st.mode === "single"
              ? { label: t(isCur ? "common.inUse" : "app.setCurrent"), icon: <Icon.check size={13} />, disabled: isCur || st.readonly, action: () => providerAction(p) }
              : { label: t(on ? "app.disable" : "app.enable"), icon: <Icon.check size={13} />, disabled: st.readonly, action: () => providerAction(p) }]
            : []),
          ...(p.models.length ? [{ label: t("common.viewModels"), action: () => showModels(st.id, p.id) }] : []),
          ...(p.editable && !p.isNew && !p.isDeleted ? ["sep" as const, { label: t("app.deleteProviderMenu"), icon: <Icon.trash size={12} />, danger: true, disabled: st.readonly || isCur, action: () => { askDeleteProvider(p); } }] : []),
          "sep",
        ];
      }
      case "model": {
        if (!st) return [];
        const pid = d("pid"), mid = d("mid");
        const base = pid === CATALOG ? st.catalog ?? [] : st.providers.find((p) => p.id === pid)?.models ?? [];
        const m = viewModels(pid, base, draft).find((x) => x.id === mid);
        if (!m) return [];
        const vis = isVisible(pid, m, draft);
        return [
          { label: t("app.copyModelId"), icon: <Icon.copy size={13} />, action: () => copy(mid) },
          ...(!m.isDeleted && !m.readonly ? [{ label: t(vis ? "app.hideInPicker" : "app.showInPicker"), disabled: st.readonly, action: () => setDraft(setModelVisible(draft, pid, m, !vis)) }] : []),
          ...((m.deletable && !m.isDeleted) ? ["sep" as const, { label: t("app.deleteModelMenu"), icon: <Icon.trash size={12} />, danger: true, disabled: st.readonly, action: async () => {
            if (!(await ask({ title: t("app.deleteModelTitle", { id: mid }), message: t("app.deleteModelMsg"), danger: true }))) return;
            setDraft(deleteModel(draft, pid, mid));
          } }] : []),
          "sep",
        ];
      }
      case "station": {
        const stn = stations.find((x) => x.key === d("station"));
        if (!stn) return [];
        return [
          { label: t("app.viewDetails"), action: () => setHubSel(stn.key) },
          ...(!stn.builtin ? [{ label: t("app.addGroupMenu"), icon: <Icon.plus size={12} />, action: () => addGroupIn(stn) }] : []),
          "sep",
        ];
      }
      case "route": {
        const r = gateway?.routes.find((x) => x.id === d("route"));
        if (!r) return [];
        return [
          { label: t(r.enabled ? "common.pauseRoute" : "common.resumeRoute"), action: () => attempt(api.gatewaySaveRoute({ ...plainRoute(r), enabled: !r.enabled }, r.id).then(setGateway)) },
          { label: t("app.deleteForwardMenu"), icon: <Icon.trash size={12} />, danger: true, action: () => { deleteRoute(r); } },
          "sep",
        ];
      }
      case "session": {
        const path = d("path");
        return [
          { label: t("app.copySessionId"), icon: <Icon.copy size={13} />, action: () => copy(d("sid")) },
          ...(d("title") ? [{ label: t("app.copyTitle"), action: () => copy(d("title")) }] : []),
          ...(path ? [{ label: t("app.showInFolder"), icon: <Icon.folder size={13} />, action: () => attempt(api.revealPath(path)) }] : []),
          "sep",
        ];
      }
    }
    return [];
  };

  const buildMenu = (target: Element): MenuItem[] => {
    const items: MenuItem[] = [];
    const ed = editableOf(target);
    const sel = window.getSelection()?.toString() ?? "";
    if (ed) {
      const s = selectedIn(ed);
      const ro = ed.readOnly || ed.disabled;
      const secret = ed instanceof HTMLInputElement && ed.type === "password";
      items.push(
        { label: t("app.cut"), hint: "Ctrl X", disabled: !s || ro || secret, action: () => { navigator.clipboard.writeText(s).then(() => insertText(ed, "")).catch(() => undefined); } },
        { label: t("common.copy"), hint: "Ctrl C", icon: <Icon.copy size={13} />, disabled: !s || secret, action: () => copy(s) },
        { label: t("app.paste"), hint: "Ctrl V", disabled: ro, action: () => { navigator.clipboard.readText().then((x) => insertText(ed, x)).catch(() => flash(t("app.clipboardReadFailed"), true)); } },
        { label: t("common.selectAll"), hint: "Ctrl A", disabled: !ed.value, action: () => { ed.focus(); ed.select(); } },
        "sep",
      );
    } else if (sel.trim()) {
      items.push({ label: t("app.copySelection"), hint: "Ctrl C", icon: <Icon.copy size={13} />, action: () => copy(sel) }, "sep");
    }
    items.push(...contextItems(target));
    const url = target.closest("[data-url]")?.getAttribute("data-url");
    if (url) {
      items.push(
        { label: t("common.copyUrl"), icon: <Icon.copy size={13} />, action: () => copy(url) },
        { label: t("app.retest"), icon: <Icon.pulse size={13} />, action: () => testOne(url) },
        "sep",
      );
    }
    // Right-clicked something specific (text, input, card, URL…): show only its actions,
    // the app-wide items below are for right-clicking empty space.
    if (items.length) return items;
    items.push(
      { label: t("app.searchMenu"), hint: "Ctrl K", icon: <Icon.search size={13} />, action: () => setPalette(true) },
      { label: t("app.reloadConfig"), hint: "F5", icon: <Icon.refresh size={13} />, action: refreshAll },
    );
    if (totalPending) {
      items.push(
        "sep",
        { label: t("app.applyAll", { n: totalPending }), icon: <Icon.check size={13} />, disabled: busy, action: () => { applyAll(); } },
        { label: t("app.discardAll"), icon: <Icon.close size={11} />, danger: true, disabled: busy, action: async () => {
          if (!(await ask({ title: tn("app.discardAllTitle", totalPending), message: t("app.discardAllMsg"), danger: true, confirmText: t("common.discard") }))) return;
          setDrafts({});
          flash(t("app.discardedAll"));
        } },
      );
    }
    items.push("sep");
    if (!page && st) {
      items.push(
        ...(st.restartable ? [{ label: t(st.running ? "common.restartAgent" : "common.startAgent", { name: st.name }), icon: st.running ? <Icon.refresh size={13} /> : <Icon.play size={13} />, disabled: !st.installed || !!restarting, action: restart }] : []),
        { label: t("app.openAgentConfigDir", { name: st.name }), icon: <Icon.folder size={13} />, action: () => attempt(api.openConfigDir(st.id)) },
      );
    }
    items.push({
      label: t(gateway?.running ? "app.gatewayTurnOff" : "app.gatewayTurnOn"),
      icon: <Icon.gateway size={13} />,
      action: () => attempt(api.gatewaySet(!gateway?.running, null).then((g) => { setGateway(g); flash(t(g.running ? "common.gatewayStarted" : "common.gatewayStopped")); })),
    });
    items.push(
      "sep",
      { label: t("common.providers"), icon: <Icon.layers size={13} />, disabled: page === "providers", action: () => setPage("providers") },
      { label: t("app.navGateway"), icon: <Icon.gateway size={13} />, disabled: page === "gateway", action: () => setPage("gateway") },
      { label: t("app.navHistory"), icon: <Icon.history size={13} />, disabled: page === "history", action: () => setPage("history") },
      { label: t("app.navSettings"), icon: <Icon.gear size={13} />, disabled: page === "settings", action: openSettings },
      "sep",
      { label: t("app.openDataDir"), icon: <Icon.folder size={13} />, action: () => attempt(api.openDataDir()) },
    );
    return items;
  };

  return (
    <div className="app">
      <ContextMenu build={buildMenu} />
      <ConfirmHost />
      <header className="topbar" data-tauri-drag-region>
        <div className="brand" data-tauri-drag-region><Icon.logo /><span data-tauri-drag-region>AgentPlus</span></div>
        <button className="search" onClick={() => setPalette(true)}>
          <Icon.search /><span>{t("app.searchPlaceholder")}</span><kbd>Ctrl K</kbd>
        </button>
        <div className="top-end" data-tauri-drag-region>
        <div className="top-right" data-tauri-drag-region>
          <EnvSwitch envs={envs} current={curEnv} switching={switching} onOpen={reloadEnvs} onPick={switchEnv} />
          <button className="gear-btn" aria-label={t("app.navSettings")} aria-pressed={page === "settings"} title={update.kind === "available" ? t("app.updateDot") : undefined}
            onClick={() => (page === "settings" ? closeSettings() : openSettings())}>
            <span className="gear-ico"><Icon.gear />{update.kind === "available" && <span className="gear-dot" />}</span><span className="gear-label">{t("app.settings")}</span>
          </button>
        </div>
        <WindowControls />
        </div>
      </header>

      <div className={`body${page === "settings" ? " solo" : page && !["providers", "history", ...(gateway?.running ? ["gateway"] : [])].includes(page) ? " wide" : ""}`}>
        {page !== "settings" && <Sidebar gateway={gateway} agents={listed} drafts={drafts} selected={page ? null : selected} page={page} onSelect={openAgent} onPage={setPage} />}

        {page === "providers" && (
          <ProvidersHub
            agents={shown}
            stations={stations}
            latency={latency}
            selected={hubSel}
            onSelect={setHubSel}
            onAdd={() => setHubDialog({ group: null })}
            onTestAll={() => testHub(true)}
            onTestOne={testOne}
            envLabel={curEnv?.label ?? t("common.localWindows")}
          />
        )}
        {page === "providers" && (
          <HubAside
            agents={shown}
            pending={allStates}
            drafts={drafts}
            stations={stations}
            busy={busy}
            onDiscard={(a) => (a ? setDraftFor(a, {}) : setDrafts({}))}
            onApplyAll={applyAll}
            detail={hubStation && (
              <ServiceDetail
                key={hubStation.key}
                s={hubStation}
                agents={shown}
                latency={latency}
                onClose={() => setHubSel(null)}
                onTest={testOne}
                onCopy={(text) => copyText(text, flash, t("app.urlCopied"))}
                onAddGroup={() => addGroupIn(hubStation)}
                onEditGroup={(g) => setHubDialog({ group: g })}
                onAddTo={hubAdd}
                onRemove={hubRemove}
                onUndo={hubUndo}
                onModels={(u) => goTo({ kind: "agent", agent: u.agent.id, tab: "models", provider: u.p?.id })}
                onDeleteGroup={hubDelete}
                onViaGateway={viaGateway}
                gatewayHost={gatewayHosts}
              />
            )}
          />
        )}
        {page === "gateway" && (
          <GatewayPage
            status={gateway}
            setStatus={setGateway}
            agents={shown}
            stations={stations}
            gatewayHost={gatewayHosts}
            onForward={forwardGroup}
            onDeleteRoute={deleteRoute}
            onAddToAgent={gatewayToAgent}
            staleKeys={staleGatewayKeys.length}
            onUpdateKeys={updateGatewayKeys}
            flash={flash}
          />
        )}
        {page === "gateway" && gateway?.running && <GatewayAside status={gateway} agents={shown} />}
        {page === "history" && <HistoryPage flash={flash} onChanged={reloadConfigs} />}
        {SYNC_ENABLED && page === "sync" && <SyncPage flash={flash} onAdopt={adoptSync} />}
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
            flash={flash}
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
            onOpenDir={() => attempt(isProjectId(st.id) ? api.openPath(st.configDir) : api.openConfigDir(st.id))}
            tab={tabs[st.id] ?? "prov"}
            setTab={(tab) => setTabs((m) => ({ ...m, [st.id]: tab }))}
            railSel={rails[st.id] ?? null}
            setRailSel={(id) => setRails((m) => ({ ...m, [st.id]: id }))}
            selectedProvider={picked[st.id] ?? null}
            onSelectProvider={(id) => setPicked((m) => ({ ...m, [st.id]: m[st.id] === id ? null : id }))}
            onProviderAction={providerAction}
            onAddProvider={() => setDialog({ editing: null })}
            flash={flash}
            sessionQuery={sessionQuery}
            onDeclineFixed={declineFixed}
            onReload={() => { api.getAgent(st.id).then(replaceAgent).catch(() => undefined); }}
            head={projSt ? (
              <ProjectHead st={projSt} project={projEntry} projects={projects} onBack={() => setProjPath(null)} onSwitch={openProject}
                onOpenDir={() => attempt(api.openPath(projSt.configDir))} />
            ) : undefined}
            onCopyProvider={projSt ? () => setCopyOpen(true) : undefined}
            projectsTab={st.id === "opencode" ? {
              count: projects.length,
              body: (
                <ProjectList
                  projects={projects}
                  busy={busy}
                  pending={Object.fromEntries(Object.entries(drafts).map(([k, d]) => [k, opCount(d)]))}
                  onOpen={openProject}
                  onPick={pickProject}
                  onForget={forgetProject}
                  onReveal={(p) => attempt(api.openPath(p))}
                />
              ),
            } : undefined}
          />
        ) : (
          <main className="page"><div className="empty">{scrub(loadError) ?? t("app.loadingConfig")}</div></main>
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
                onModels={() => showModels(st.id, pickedProvider.id)}
                onCopy={(text) => copyText(text, flash, t("app.urlCopied"))}
                onEdit={() => setDialog({ editing: pickedProvider })}
                onDelete={() => askDeleteProvider(pickedProvider)}
                gatewayRoute={routeOfProvider(pickedProvider)}
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

      {dialog && st && <ProviderDialog st={st} draft={draft} editing={dialog.editing} gatewayRoute={routeOfProvider(dialog.editing)} gateway={gateway} ensureGateway={ensureGateway} onSave={saveProvider} onClose={() => setDialog(null)} />}
      {palette && <CommandPalette agents={listed} onGo={goTo} onClose={() => setPalette(false)} />}
      {copyOpen && st && isProjectId(st.id) && <CopyProviderDialog target={st} agents={shown} lib={lib} onCopy={copyToProject} onClose={() => setCopyOpen(false)} />}
      {hubDialog !== undefined && <ServiceDialog agents={shown} group={hubDialog.group} prefill={hubDialog.prefill} onSave={hubSave} onClose={() => setHubDialog(undefined)} />}
      {envAsk && (
        <PendingDialog
          title={t("app.beforeSwitch", { env: envs.find((e) => e.id === envAsk)?.label ?? envAsk })}
          agents={allStates}
          drafts={drafts}
          busy={busy || switching}
          onConfirm={confirmSwitch}
          onCancel={() => setEnvAsk(null)}
        />
      )}
      {run && <RestartDialog run={run} onClose={closeRun} onCancel={cancelRun} />}
      {closeAsk && <CloseDialog onDone={closeAsk} />}
      {toast && <div className={`toast${toast.error ? " error" : ""}`} role="status">{scrub(toast.text)}</div>}
    </div>
  );
}
