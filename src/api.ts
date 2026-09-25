import { Channel, invoke as tauriInvoke, type InvokeArgs } from "@tauri-apps/api/core";
import { whenReady } from "./i18n";
import { inTauri } from "./tauri";

/** Every call waits until the backend renders text in the UI language. */
const invoke = <T>(cmd: string, args?: InvokeArgs) => whenReady().then(() => tauriInvoke<T>(cmd, args));

export type AgentId =
  | "codex" | "claude" | "opencode" | "zcode" | "mimo"
  | "hermes" | "gemini" | "pi" | "openclaw" | "qwen" | "kimi" | "droid" | "codebuddy" | "kilo"
  | "trae";

/** A badge on a model: `id` is stable (`fast`, `custom`, `cap:image`, `role:default`…) and is
 * what the UI checks; `label` is display text in the current language. */
export interface ModelTag {
  id: string;
  label: string;
}

export interface Model {
  id: string;
  visible: boolean;
  readonly: boolean;
  tags: ModelTag[];
  ctx: string | null;
  name: string | null;
  context: number | null;
  deletable: boolean;
  /** Values of the agent's model fields that are set, by field key. */
  extra?: Record<string, ModelFieldValue>;
}

export type ModelFieldValue = boolean | number | string | string[];

/** A per-model setting beyond name / context (image input, max output…), declared by the agent. */
export interface ModelField {
  key: string;
  /** Stable group id ("io" | "gen"); `group` is its display label. */
  gid: string;
  group: string;
  label: string;
  desc: string;
  kind: "bool" | "number" | "chips" | "select";
  options: string[];
  /** Label per option. */
  hints: string[];
  /** Short tags for the model table's capability column: one for a bool input field (shown when on); one per option for chips, or none to use `hints`. */
  caps: string[];
}

/** Wire protocol. "gemini" is shown for Gemini-only agents; the gateway converts the other three. */
export type ApiKind = "responses" | "chat" | "anthropic" | "gemini";

export interface ProviderInput {
  id: string | null;
  name: string;
  baseUrl: string;
  api: ApiKind;
  apiKey: string | null;
  models: string[];
  /** Use this library entry's key (the backend fills it in). */
  keyFromLibrary?: string | null;
  /** Codex: keep the ChatGPT sign-in while requests go to this provider. Omitted/null = keep. */
  officialAuth?: boolean | null;
}

export interface ModelInput {
  id: string;
  name: string | null;
  context: number | null;
  /** Fields to change, by key; null clears one (back to the agent's default). */
  extra?: Record<string, ModelFieldValue | null>;
}

export interface Provider {
  id: string;
  name: string;
  baseUrl: string | null;
  host: string;
  apis: string[];
  builtin: boolean;
  enabled: boolean;
  compatible: boolean;
  reason: string | null;
  models: Model[];
  details: Kv[];
  editable: boolean;
  api: ApiKind;
  hasKey: boolean;
  /** One-way fingerprint of the key (groups a relay's entries by key). */
  keyFp: string | null;
  keyHint: string | null;
  /** Codex: official sign-in mix (`requires_openai_auth`). */
  officialAuth: boolean;
}

export interface Setting {
  key: string;
  group: string;
  label: string;
  desc: string;
  /** select: options are values, hints their labels; text: options are suggestions; list: one entry per line. */
  kind: "bool" | "chips" | "select" | "text" | "list";
  value: SettingValue;
  options: string[];
  /** One short explanation per option (may be empty). */
  hints: string[];
}

export type SettingValue = boolean | string | string[];

export interface Kv {
  k: string;
  v: string;
  mono: boolean;
}

export interface AgentState {
  id: AgentId;
  name: string;
  installed: boolean;
  version: string | null;
  running: boolean;
  mode: "single" | "multi";
  configDir: string;
  files: string[];
  currentProvider: string | null;
  providers: Provider[];
  catalog: Model[] | null;
  catalogFile: string | null;
  settings: Setting[];
  current: Kv[];
  /** The model requests go out with, whichever provider is active (Codex, Gemini); null when unset or per provider. */
  currentModel: string | null;
  notes: string[];
  readonly: boolean;
  /** Codex: not on the fixed id yet, but could be. */
  fixedPending: boolean;
  /** Codex: prefill "turn on fixed id" as a pending change (not declined before). */
  fixedPrompt: boolean;
  /** A desktop app AgentPlus can restart; CLIs pick up changes on their next run. */
  restartable: boolean;
  /** Per-model settings this agent's config understands. */
  modelFields?: ModelField[];
}

export type Op =
  | { op: "set_current_provider"; provider: string }
  | { op: "set_provider_enabled"; provider: string; enabled: boolean }
  | { op: "set_model_visible"; provider: string; model: string; visible: boolean }
  | { op: "set_setting"; key: string; value: SettingValue }
  | { op: "upsert_provider"; provider: ProviderInput }
  | { op: "delete_provider"; provider: string }
  | { op: "upsert_model"; provider: string; model: ModelInput }
  | { op: "delete_model"; provider: string; model: string }
  | { op: "set_provider_models"; provider: string; models: string[] }
  | { op: "set_model_roles"; provider: string; roles: Record<string, string> }
  | { op: "import_provider"; fromAgent: string; provider: string; api?: ApiKind; name?: string; label?: string };

/** Where AgentPlus reads and writes configs. */
export interface EnvInfo {
  id: string;
  label: string;
  detail: string;
  current: boolean;
}

export interface GatewayRoute {
  id: string;
  name: string;
  /** Library entry holding the upstream address and key. */
  library: string;
  upstreamApi: ApiKind;
  modelMap: [string, string][];
  enabled: boolean;
  /** Share of the unified entry when several forwards serve the same model. */
  weight: number;
  /** [agent, provider] entries whose address was switched to this forward when it was added. */
  replaced?: [string, string][];
}

export interface GatewayRouteView extends GatewayRoute {
  localBase: string;
  upstreamName: string | null;
  upstreamUrl: string | null;
  upstreamMissing: boolean;
  /** Models this forward is known to serve. */
  models: string[];
  /** Error breaker; null while there is nothing to report. */
  breaker: GatewayBreakerView | null;
}

/** Error breaker settings: pause a forward after `threshold` upstream errors in a row. */
export interface GatewayBreaker {
  enabled: boolean;
  threshold: number;
  /** First pause; doubles on every trip in a row (up to 10 minutes). */
  cooldownSecs: number;
}

export interface GatewayBreakerView {
  /** open = paused; probe = pause over, the next request tests it; closed = errors so far, still working. */
  state: "open" | "probe" | "closed";
  remainingSecs: number;
  pausedSecs: number;
  /** The error that paused it (or the latest one). */
  reason: string | null;
  /** When it paused, HH:MM:SS. */
  at: string | null;
  fails: number;
  trips: number;
}

export interface GatewayLog {
  at: string;
  route: string;
  method: string;
  path: string;
  inbound: string;
  upstream: string;
  model: string;
  status: number;
  ms: number;
  stream: boolean;
  converted: boolean;
  error: string | null;
  /** [input, output] tokens the upstream reported. */
  usage: [number, number] | null;
  /** Agent whose gateway key the request carried ("legacy" = the old shared key, "agentplus" = AgentPlus's own test); null when refused. */
  agent: string | null;
}

/** One agent's share of a minute of gateway traffic. */
export interface GatewayAgentUse {
  requests: number;
  failures: number;
  inputTokens: number;
  outputTokens: number;
}

/** One minute of gateway traffic. */
export interface GatewayMinute {
  /** Unix seconds at the start of the minute. */
  t: number;
  requests: number;
  failures: number;
  msTotal: number;
  msMax: number;
  inputTokens: number;
  outputTokens: number;
  peakActive: number;
  /** Requests by calling agent (see `GatewayLog.agent`). */
  agents: Record<string, GatewayAgentUse>;
}

export interface GatewayStatus {
  enabled: boolean;
  running: boolean;
  port: number;
  error: string | null;
  requests: number;
  failures: number;
  active: number;
  routes: GatewayRouteView[];
  log: GatewayLog[];
  /** Last hour, per minute; quiet minutes are left out. */
  series: GatewayMinute[];
  /** Gateway clock, unix seconds. */
  now: number;
  /** Unified entry (picks a forward by model): http://127.0.0.1:<port>/v1 */
  unifiedBase: string;
  breaker: GatewayBreaker;
  /** Agent → fingerprint of its own gateway key (compare with `Provider.keyFp`). */
  keyFps: Record<string, string>;
  /** Fingerprint of the old shared key "agentplus-gateway". */
  legacyFp: string;
  /** Earlier ports, newest first: agent addresses there still point at this gateway (and get moved). */
  formerPorts: number[];
}

/** Codex: fetching the official model list through a temporary ChatGPT login. */
export interface OfficialFetch {
  active: boolean;
  startedAt: string | null;
  backupDir: string | null;
  cacheReady: boolean;
  cacheModels: number;
  cachePath: string;
  catalogPath: string;
  chatgptLogin: boolean;
}

export interface FetchedModel {
  slug: string;
  name: string;
  visible: boolean;
}

/** One real request sent through a provider. */
export interface TestResult {
  ok: boolean;
  status: number | null;
  ms: number;
  model: string;
  url: string;
  reply: string | null;
  error: string | null;
  usage: [number, number] | null;
}

/** Result of looking for an agent in the current environment. */
export interface AgentDetect {
  id: AgentId;
  name: string;
  appFound: boolean;
  version: string | null;
  running: boolean;
  defaultDir: string;
  customDir: string | null;
  configDir: string;
  configFound: boolean;
  enabled: boolean;
  note: string | null;
  /** Detected only; AgentPlus cannot edit it (the note has manual steps). */
  manual?: boolean;
}

/** A provider in AgentPlus's shared library (the key stays in the backend). */
export interface LibEntry {
  id: string;
  name: string;
  baseUrl: string;
  api: ApiKind;
  hasKey: boolean;
  keyHint: string | null;
  keyFp: string | null;
  models: string[];
}

export interface LibInput {
  id: string | null;
  name: string;
  baseUrl: string;
  api: ApiKind;
  apiKey: string | null;
  models: string[] | null;
  /** [agent, provider] to copy the key from when the library has none yet. */
  adoptFrom: [string, string] | null;
}

export interface BackupEntry {
  id: string;
  stamp: string;
  agent: string;
  reason: string;
  files: { name: string; path: string | null }[];
  bytes: number;
  restorable: boolean;
  /** Why it can't be rolled back automatically (original file gone, database backup…). */
  blocked: string | null;
  /** Blocked because the original file is gone. */
  blockedMissing: boolean;
}

export interface BackupDiffRow {
  /** " " unchanged, "-" only in the backup, "+" only in the current file, "…" folded lines. */
  kind: " " | "-" | "+" | "…";
  text: string;
  old: number | null;
  new: number | null;
}

export interface BackupFileDetail {
  name: string;
  path: string | null;
  backupBytes: number;
  currentBytes: number | null;
  currentModified: string | null;
  same: boolean;
  binary: boolean;
  diff: BackupDiffRow[];
  added: number;
  removed: number;
  truncated: boolean;
}

export interface BackupDetail {
  id: string;
  dir: string;
  time: string | null;
  files: BackupFileDetail[];
}

export interface SyncStatus {
  folder: string | null;
  fileExists: boolean;
  exportedAt: string | null;
  machine: string | null;
}

export interface SyncSuggestion {
  agent: AgentId;
  title: string;
  detail: string;
  ops: [string, Op][];
}

export interface DiffGroup {
  file: string;
  lines: { text: string; add: boolean }[];
}

export interface SessionRow {
  id: string;
  title: string;
  cwd: string;
  provider: string;
  model: string;
  kind: "user" | "automation" | "subagent" | "review" | "exec" | "agent";
  archived: boolean;
  updatedMs: number;
  size: number;
  rolloutPath: string;
  rolloutExists: boolean;
  hidden: string[];
}

export interface SessionList {
  sessions: SessionRow[];
  currentProvider: string;
  providers: [string, number][];
  /** Where sessions can be moved: "openai" plus every provider in config.toml. */
  targets: string[];
  codexRunning: boolean;
  writable: boolean;
  note: string | null;
  lastRepair: { stamp: string; target: string; count: number; undone: boolean } | null;
}

export interface HealthItem {
  key: string;
  title: string;
  status: "ok" | "warn" | "error" | "info";
  detail: string;
}

export interface CleanupPreview {
  tmpCount: number;
  tmpBytes: number;
  logsBytes: number;
  logsRows: number;
  logsOldRows: number;
  logsFreeBytes: number;
  walBytes: number;
  codexRunning: boolean;
}

/** A project folder opened in AgentPlus (per-project agent configs). */
export interface ProjectEntry {
  path: string;
  name: string;
  /** "opencode@<path>": the id to load / preview / apply its OpenCode config with. */
  agent: AgentId;
  lastOpened: string | null;
  exists: boolean;
  /** Its opencode.json(c), when it has one. */
  config: string | null;
  providers: number;
  git: boolean;
}

export const PROJECT_PREFIX = "opencode@";
/** Project configs load and apply like agents, under the id "opencode@<folder>". */
export const isProjectId = (id: string) => id.startsWith(PROJECT_PREFIX);

export interface ApplyResult {
  state: AgentState;
  files: string[];
  backupDir: string | null;
}

/** Steps of a restart, in the order they run ("port" and "patch" only with Codex UI injection). */
export type RestartStep = "stop" | "start" | "port" | "patch";
export type RestartStatus = "active" | "done" | "skip" | "warn";
/** Sent by the backend while a restart runs. `detail` is backend-rendered text. */
export type RestartProgress =
  | { kind: "plan"; steps: RestartStep[] }
  | { kind: "step"; step: RestartStep; status: RestartStatus; detail: string | null };

/** A newer AgentPlus release on GitHub. `notes` is the release's Markdown text. */
export interface UpdateInfo {
  version: string;
  current: string;
  notes: string | null;
  date: string | null;
}

/** Sent by the backend while an update downloads and installs. */
export type UpdateProgress =
  | { kind: "download"; done: number; total: number | null }
  | { kind: "install" };

const real = {
  listAgents: () => invoke<AgentState[]>("list_agents"),
  getAgent: (agent: AgentId) => invoke<AgentState>("get_agent", { agent }),
  preview: (agent: AgentId, ops: Op[]) => invoke<DiffGroup[]>("preview", { agent, ops }),
  apply: (agent: AgentId, ops: Op[]) => invoke<ApplyResult>("apply", { agent, ops }),
  testLatency: (url: string) => invoke<number>("test_latency", { url }),
  /** Restarts the agent, or starts it when it isn't running; resolves with a summary. */
  restart: (agent: AgentId, onProgress?: (p: RestartProgress) => void) => {
    const ch = new Channel<RestartProgress>();
    if (onProgress) ch.onmessage = onProgress;
    return invoke<string>("restart_agent", { agent, onProgress: ch });
  },
  /** Stops the running restart at its next wait (it then rejects); an app already started keeps running. */
  cancelRestart: () => invoke<void>("cancel_restart"),
  agentRunning: (agent: AgentId) => invoke<boolean>("agent_running", { agent }),
  openConfigDir: (agent: AgentId) => invoke<void>("open_config_dir", { agent }),
  codexSessions: () => invoke<SessionList>("codex_sessions"),
  codexHealth: () => invoke<HealthItem[]>("codex_health"),
  codexCleanupPreview: (days: number) => invoke<CleanupPreview>("codex_cleanup_preview", { days }),
  codexCleanup: (tmp: boolean, logsDays: number | null, wal: boolean) => invoke<string>("codex_cleanup", { tmp, logsDays, wal }),
  codexRepair: (ids: string[], target: string) => invoke<string>("codex_repair", { ids, target }),
  codexUndoRepair: (stamp: string) => invoke<string>("codex_undo_repair", { stamp }),
  revealPath: (path: string) => invoke<void>("reveal_path", { path }),
  fetchModels: (agent: AgentId, provider: string) => invoke<string[]>("fetch_models", { agent, provider }),
  fetchModelsUrl: (baseUrl: string, apiKey: string | null, api: ApiKind) => invoke<string[]>("fetch_models_url", { baseUrl, apiKey, api }),
  fetchModelsLib: (id: string) => invoke<string[]>("fetch_models_lib", { id }),
  listBackups: () => invoke<BackupEntry[]>("list_backups"),
  backupDetail: (id: string) => invoke<BackupDetail>("backup_detail", { id }),
  restoreBackup: (id: string) => invoke<string>("restore_backup", { id }),
  syncStatus: () => invoke<SyncStatus>("sync_status"),
  syncSetFolder: (path: string) => invoke<void>("sync_set_folder", { path }),
  syncExport: () => invoke<string>("sync_export"),
  syncPreview: () => invoke<SyncSuggestion[]>("sync_preview"),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  openUrl: (url: string) => invoke<void>("open_url", { url }),
  dismissFixedPrompt: () => invoke<void>("codex_dismiss_fixed_prompt"),
  listEnvs: () => invoke<EnvInfo[]>("list_envs"),
  setEnv: (id: string) => invoke<void>("set_env", { id }),
  libraryList: () => invoke<LibEntry[]>("library_list"),
  librarySave: (input: LibInput) => invoke<LibEntry>("library_save", { input }),
  libraryDelete: (id: string) => invoke<void>("library_delete", { id }),
  openDataDir: () => invoke<void>("open_data_dir"),
  quitApp: () => invoke<void>("quit_app"),
  detectAgents: () => invoke<AgentDetect[]>("detect_agents"),
  testProvider: (agent: string, provider: string, model: string) => invoke<TestResult>("test_provider", { agent, provider, model }),
  officialStatus: () => invoke<OfficialFetch>("codex_official_status"),
  officialStart: () => invoke<OfficialFetch>("codex_official_start"),
  officialFinish: () => invoke<FetchedModel[]>("codex_official_finish"),
  officialCancel: () => invoke<void>("codex_official_cancel"),
  gatewayStatus: () => invoke<GatewayStatus>("gateway_status"),
  gatewaySet: (enabled: boolean, port: number | null) => invoke<GatewayStatus>("gateway_set", { enabled, port }),
  gatewaySaveRoute: (route: GatewayRoute, oldId: string | null) => invoke<GatewayStatus>("gateway_save_route", { route, oldId }),
  gatewayDeleteRoute: (id: string) => invoke<GatewayStatus>("gateway_delete_route", { id }),
  gatewaySetBreaker: (breaker: GatewayBreaker) => invoke<GatewayStatus>("gateway_set_breaker", { breaker }),
  /** Un-pause one forward, or all with null. */
  gatewayResetBreaker: (id: string | null) => invoke<GatewayStatus>("gateway_reset_breaker", { id }),
  gatewayTest: (route: string, api: ApiKind, model: string) => invoke<TestResult>("gateway_test", { route, api, model }),
  setAgentDir: (agent: AgentId, path: string | null) => invoke<void>("set_agent_dir", { agent, path }),
  projectsList: () => invoke<ProjectEntry[]>("projects_list"),
  projectOpen: (path: string) => invoke<ProjectEntry>("project_open", { path }),
  projectForget: (path: string) => invoke<void>("project_forget", { path }),
  pickFolder: (start: string | null) => invoke<string | null>("pick_folder", { start }),
  /** A newer release, or null when this is the latest. */
  updateCheck: () => invoke<UpdateInfo | null>("update_check"),
  /** Downloads, verifies and installs the update found by the last check; the app then restarts. */
  updateInstall: (onProgress: (p: UpdateProgress) => void) => {
    const ch = new Channel<UpdateProgress>();
    ch.onmessage = onProgress;
    return invoke<void>("update_install", { onProgress: ch });
  },
};

// Plain-browser preview (`npm run dev`): serve a static snapshot so the UI can be
// checked without the Tauri backend. Never used inside the app, and left out of
// production builds (see `api` at the bottom).

/** Browser demo: agents started or restarted from the demo count as running. */
const demoRunning: Record<string, boolean> = {};
/** Browser demo: a pause that stands in for real work. */
const sleep = (ms: number) => new Promise<void>((r) => window.setTimeout(r, ms));
let demoCancel = false;

// The snapshot is local-only (.gitignore): a glob resolves to nothing when it is missing, so a
// fresh clone still type-checks and builds, and the demo just starts empty.
const FIXTURE = import.meta.glob<{ default: unknown }>("./dev-fixture.json");

/** Older snapshots have plain-string tags. */
const tagOf = (g: unknown): ModelTag => (typeof g === "string" ? { id: `tag:${g}`, label: g } : (g as ModelTag));
const withTags = (m: Model): Model => ({ ...m, tags: (m.tags ?? []).map(tagOf) });

async function fixture(): Promise<AgentState[]> {
  const load = FIXTURE["./dev-fixture.json"];
  const raw = load ? ((await load()).default as AgentState[]) : [];
  // The snapshot may predate `restartable` (its desktop apps are), `currentModel` and the
  // model fields' `caps`.
  const list = raw.map((a) => ({
    ...a, restartable: a.restartable ?? true, running: demoRunning[a.id] ?? a.running,
    currentModel: a.currentModel ?? null, modelFields: a.modelFields?.map((f) => ({ ...f, caps: f.caps ?? [] })),
    catalog: a.catalog?.map(withTags) ?? null, providers: a.providers.map((p) => ({ ...p, models: p.models.map(withTags) })),
  }));
  // No OpenCode in the snapshot: MiMo runs the same config format, so it stands in.
  const mimo = list.find((a) => a.id === "mimo");
  if (list.some((a) => a.id === "opencode") || !mimo) return list;
  return [...list, { ...mimo, id: "opencode", name: "OpenCode", files: ["~/.config/opencode/opencode.json", "~/.local/share/opencode/auth.json"], restartable: false }];
}

let demoEnv = "windows";
let demoLib: LibEntry[] = [];
const demoOfficial: OfficialFetch = { active: false, startedAt: "15:20:01", backupDir: "C:\\Users\\me\\.agentplus\\backups\\20260923-152001\\codex", cacheReady: false, cacheModels: 0, cachePath: "~/.codex/models_cache.json", catalogPath: "~/.codex/models.json", chatgptLogin: true };
let demoOfficialAt = 0;
const demoGateway: { enabled: boolean; port: number; routes: GatewayRoute[]; breaker: GatewayBreaker; cleared: string[] } = {
  enabled: false, port: 18650, routes: [], breaker: { enabled: true, threshold: 3, cooldownSecs: 60 }, cleared: [],
};
/** Demo: the second forward shows up paused by the breaker until it is reset. */
const demoBreaker = (r: GatewayRoute, i: number): GatewayBreakerView | null =>
  demoGateway.breaker.enabled && i === 1 && !demoGateway.cleared.includes(r.id)
    ? { state: "open", remainingSecs: 42, pausedSecs: 60, reason: "HTTP 401 密钥无效或未授权：invalid api key", at: "15:41:52", fails: 0, trips: 1 }
    : null;
const demoGw = (): GatewayStatus => ({
  enabled: demoGateway.enabled, running: demoGateway.enabled, port: demoGateway.port, error: null,
  requests: demoGateway.enabled ? 12 : 0, failures: 1, active: 0,
  unifiedBase: `http://127.0.0.1:${demoGateway.port}/v1`,
  breaker: demoGateway.breaker,
  routes: demoGateway.routes.map((r, i) => {
    const e = demoLib.find((x) => x.id === r.library);
    return { ...r, models: e?.models ?? [], localBase: `http://127.0.0.1:${demoGateway.port}/${r.id}/v1`, upstreamName: e?.name ?? r.library, upstreamUrl: e?.baseUrl ?? null, upstreamMissing: false, breaker: demoBreaker(r, i) };
  }),
  log: demoGateway.enabled ? [
    { at: "15:42:10", route: "relay", method: "POST", path: "/relay/v1/responses", inbound: "responses", upstream: "chat", model: "glm-5", status: 200, ms: 3120, stream: true, converted: true, error: null, usage: [18230, 412], agent: "codex" },
    { at: "15:41:52", route: "relay", method: "POST", path: "/relay/v1/responses", inbound: "responses", upstream: "chat", model: "glm-5", status: 401, ms: 210, stream: true, converted: true, error: "invalid api key", usage: null, agent: "claude" },
  ] : [],
  now: Math.floor(Date.now() / 1000),
  series: demoGateway.enabled ? demoSeries() : [],
  keyFps: { codex: "fp-codex", claude: "fp-claude" },
  legacyFp: "fp-legacy",
  formerPorts: [],
});

/** Plausible traffic for the charts in browser demo mode (stable per minute). */
function demoSeries() {
  const now = Math.floor(Date.now() / 60000) * 60;
  const out = [];
  for (let k = 59; k >= 0; k--) {
    const t = now - k * 60;
    const r = (n: number) => { const x = Math.sin(t / 60 * 12.9898 + n * 78.233) * 43758.5453; return x - Math.floor(x); };
    if (r(1) < 0.18) continue;
    const requests = Math.round(2 + 6 * (1 + Math.sin(t / 900)) * r(2) + (k < 8 ? 6 : 0));
    const avg = 900 + 2600 * r(3);
    const failures = r(4) < 0.15 ? 1 + Math.floor(r(5) * 2) : 0;
    const inputTokens = Math.round(requests * (9000 + 14000 * r(7)));
    const outputTokens = Math.round(requests * (300 + 900 * r(8)));
    // Split between two agents, Codex taking the larger share.
    const share = 0.55 + 0.3 * r(10);
    const codexReq = Math.round(requests * share);
    const agents: Record<string, GatewayAgentUse> = {
      codex: { requests: codexReq, failures, inputTokens: Math.round(inputTokens * share), outputTokens: Math.round(outputTokens * share) },
    };
    if (requests > codexReq) {
      agents.claude = { requests: requests - codexReq, failures: 0, inputTokens: inputTokens - agents.codex.inputTokens, outputTokens: outputTokens - agents.codex.outputTokens };
    }
    out.push({
      t, requests, failures,
      msTotal: Math.round(avg * requests), msMax: Math.round(avg * (1.4 + r(6))),
      inputTokens, outputTokens,
      peakActive: 1 + Math.floor(r(9) * 3),
      agents,
    });
  }
  return out;
}

let demoProjects: ProjectEntry[] = [
  { path: "D:\\xm\\shop", name: "shop", agent: "opencode@D:\\xm\\shop" as AgentId, lastOpened: new Date(Date.now() - 3600e3).toISOString(), exists: true, config: "D:\\xm\\shop\\opencode.json", providers: 1, git: true },
  { path: "D:\\xm\\demo", name: "demo", agent: "opencode@D:\\xm\\demo" as AgentId, lastOpened: new Date(Date.now() - 3 * 86400e3).toISOString(), exists: true, config: null, providers: 0, git: false },
];

/** Browser demo: a project is the OpenCode fixture with its custom providers inherited. */
async function demoProject(agent: string): Promise<AgentState> {
  const oc = (await fixture()).find((a) => a.id === "opencode")!;
  const path = agent.slice(PROJECT_PREFIX.length);
  return {
    ...oc, id: agent as AgentId, name: path.split(/[\\/]/).pop() ?? path, version: null, running: false, restartable: false,
    configDir: path, files: [`${path}\\opencode.json`, "~/.local/share/opencode/auth.json"],
    providers: oc.providers.map((p) => (p.builtin ? p : { ...p, editable: false, apis: [...p.apis, "全局"], models: p.models.filter((m) => m.visible).map((m) => ({ ...m, readonly: true, deletable: false })) })),
    notes: ["项目配置和全局配置合并生效，同名的键以项目为准。API Key 统一存进 ~/.local/share/opencode/auth.json，不写进项目文件。"],
    settings: [
      { key: "model", group: "模型", label: "默认模型", desc: "provider/model 格式，OpenCode 启动时默认选中它。留空＝继承全局（minew/mimo-v2.5）", kind: "text", value: "", options: ["minew/mimo-v2.5"], hints: [] },
      { key: "enabled_providers", group: "供应商", label: "只加载这些供应商", desc: "都不选＝不限制", kind: "chips", value: [], options: oc.providers.map((p) => p.id), hints: [] },
      { key: "share", group: "行为", label: "会话分享", desc: "share：会话能否分享成公开链接", kind: "select", value: "", options: ["", "manual", "auto", "disabled"], hints: ["继承全局（手动分享）", "手动分享", "自动分享", "禁止分享"] },
      { key: "snapshot", group: "行为", label: "改动快照", desc: "snapshot：记录文件改动，支持 /undo 撤销", kind: "select", value: "", options: ["", "true", "false"], hints: ["继承全局（开）", "开", "关"] },
      { key: "permission.bash", group: "权限", label: "执行命令", desc: "permission.bash：运行 shell 命令", kind: "select", value: "ask", options: ["", "allow", "ask", "deny"], hints: ["继承全局（允许）", "允许", "每次询问", "禁止"] },
      { key: "instructions", group: "指令与文件", label: "额外指令文件", desc: "instructions：每行一个路径或 glob。和全局的合并", kind: "list", value: ["CONTRIBUTING.md"], options: [], hints: [] },
    ],
  };
}

const demo: typeof real = {
  listAgents: fixture,
  getAgent: async (agent) => (isProjectId(agent) ? demoProject(agent) : (await fixture()).find((a) => a.id === agent)!),
  preview: async (_agent, ops) => [{ file: "（演示）", lines: ops.map((o) => ({ text: JSON.stringify(o), add: true })) }],
  apply: async (agent) => ({ state: isProjectId(agent) ? await demoProject(agent) : (await fixture()).find((a) => a.id === agent)!, files: [], backupDir: null }),
  testLatency: async () => 120 + Math.round(Math.random() * 300),
  restart: async (agent, onProgress) => {
    const on = onProgress ?? (() => undefined);
    const inject = agent === "codex";
    const steps: RestartStep[] = inject ? ["stop", "start", "port", "patch"] : ["stop", "start"];
    const running = (await fixture()).find((a) => a.id === agent)?.running ?? false;
    demoCancel = false;
    on({ kind: "plan", steps });
    for (const step of steps) {
      if (step === "stop" && !running) {
        on({ kind: "step", step, status: "skip", detail: null });
        continue;
      }
      on({ kind: "step", step, status: "active", detail: null });
      await sleep(700 + Math.random() * 900);
      if (demoCancel) throw new Error("（演示）已取消");
      on({ kind: "step", step, status: "done", detail: null });
    }
    demoRunning[agent] = true;
    return running ? "（演示）已重启" : "（演示）已启动";
  },
  cancelRestart: async () => { demoCancel = true; },
  agentRunning: async (agent) => (await fixture()).find((a) => a.id === agent)?.running ?? false,
  openConfigDir: async () => undefined,
  codexSessions: async () => ({
    sessions: [
      { id: "demo-1", title: "修复登录页样式", cwd: "D:\\xm\\demo", provider: "klpz", model: "gpt-6-astra", kind: "user", archived: false, updatedMs: Date.now() - 3600e3, size: 2_400_000, rolloutPath: "", rolloutExists: true, hidden: ["属于「klpz」，当前是「work」：最近列表和归档里可能看不到"] },
      { id: "demo-2", title: "接入支付回调", cwd: "D:\\xm\\shop", provider: "work", model: "gpt-5.6-sol", kind: "user", archived: false, updatedMs: Date.now() - 86400e3, size: 640_000, rolloutPath: "", rolloutExists: true, hidden: [] },
      { id: "demo-3", title: "审查：依赖升级", cwd: "D:\\xm\\shop", provider: "klpz", model: "codex-auto-review", kind: "review", archived: false, updatedMs: Date.now() - 2 * 86400e3, size: 120_000, rolloutPath: "", rolloutExists: true, hidden: ["子代理 / 审查 / exec 会话不进侧边栏"] },
    ],
    currentProvider: "work",
    providers: [["klpz", 2], ["work", 1]],
    targets: ["openai", "klpz", "work"],
    codexRunning: false,
    writable: true,
    note: null,
    lastRepair: null,
  }),
  codexHealth: async () => [
    { key: "schema", title: "数据库版本", status: "ok", detail: "state_5 v55，已验证" },
    { key: "provider", title: "会话供应商", status: "warn", detail: "klpz 188 个，切换后在最近列表和归档里可能看不到" },
    { key: "tmp", title: "残留临时文件", status: "warn", detail: "4 个中断写入留下的文件，共 2.6 MB" },
  ],
  codexCleanupPreview: async () => ({ tmpCount: 4, tmpBytes: 2_711_230, logsBytes: 141_946_880, logsRows: 59930, logsOldRows: 38757, logsFreeBytes: 32_911_360, walBytes: 9_109_512, codexRunning: false }),
  codexCleanup: async () => "（演示）已清理",
  codexRepair: async () => "（演示）已修复",
  codexUndoRepair: async () => "（演示）已撤销",
  revealPath: async () => undefined,
  fetchModels: async () => ["gpt-5.6-sol", "gpt-5.6-luna", "deepseek-v4-pro", "kimi-k3", "glm-5.3", "qwen3.8-max"],
  fetchModelsUrl: async () => ["deepseek-v4-pro", "kimi-k3", "glm-5.3"],
  fetchModelsLib: async () => ["glm-5", "glm-5.3", "kimi-k3"],
  listBackups: async () => [
    { id: "20260923-140512/codex", stamp: "20260923-140512", agent: "codex", reason: "应用配置", files: [{ name: "config.toml", path: "C:\\Users\\me\\.codex\\config.toml" }], bytes: 10240, restorable: true, blocked: null, blockedMissing: false },
    { id: "20260923-131201/zcode", stamp: "20260923-131201", agent: "zcode", reason: "应用配置", files: [{ name: "provider_config.json", path: "C:\\Users\\me\\.zcode\\v2\\provider_config.json" }], bytes: 19329, restorable: true, blocked: null, blockedMissing: false },
  ],
  backupDetail: async (id) => ({
    id, dir: `~/.agentplus/backups/${id}`, time: "2026-09-23T14:05:12+08:00",
    files: [{
      name: id.endsWith("zcode") ? "provider_config.json" : "config.toml",
      path: id.endsWith("zcode") ? "C:\\Users\\me\\.zcode\\v2\\provider_config.json" : "C:\\Users\\me\\.codex\\config.toml",
      backupBytes: 10240, currentBytes: 10388, currentModified: "2026-09-23 14:05:12", same: false, binary: false, added: 3, removed: 1, truncated: false,
      diff: [
        { kind: "…", text: "12 行未变", old: null, new: null },
        { kind: " ", text: "[model_providers.work]", old: 13, new: 13 },
        { kind: " ", text: "name = \"work\"", old: 14, new: 14 },
        { kind: "-", text: "base_url = \"https://api.example.com/v1\"", old: 15, new: null },
        { kind: "+", text: "base_url = \"http://127.0.0.1:18650/relay/v1\"", old: null, new: 15 },
        { kind: "+", text: "wire_api = \"responses\"", old: null, new: 16 },
        { kind: "+", text: "env_key = \"sk-a…9f2c\"", old: null, new: 17 },
        { kind: " ", text: "", old: 16, new: 18 },
        { kind: "…", text: "40 行未变", old: null, new: null },
      ],
    }],
  }),
  restoreBackup: async () => "（演示）已回滚",
  syncStatus: async () => ({ folder: null, fileExists: false, exportedAt: null, machine: null }),
  syncSetFolder: async () => undefined,
  syncExport: async () => "（演示）已导出",
  syncPreview: async () => [],
  openPath: async () => undefined,
  openUrl: async (url) => { window.open(url, "_blank"); },
  dismissFixedPrompt: async () => undefined,
  listEnvs: async () => [
    { id: "windows", label: "本机 · Windows", detail: "C:\\Users\\me", current: demoEnv === "windows" },
    { id: "wsl:Ubuntu", label: "WSL · Ubuntu", detail: "Codex CLI 的配置与会话；ZCode、MiMo Desktop 只在 Windows 上", current: demoEnv === "wsl:Ubuntu" },
  ],
  setEnv: async (id) => { demoEnv = id; },
  libraryList: async () => demoLib,
  librarySave: async (input) => {
    const e: LibEntry = {
      id: input.id ?? input.name.toLowerCase().replace(/[^a-z0-9]+/g, "-") + "-" + demoLib.length,
      name: input.name, baseUrl: input.baseUrl, api: input.api,
      hasKey: !!input.apiKey || !!input.adoptFrom || !!demoLib.find((x) => x.id === input.id)?.hasKey,
      keyHint: input.apiKey ? "••••" + input.apiKey.slice(-4) : "••••demo",
      keyFp: input.apiKey ? "fp-" + input.apiKey.slice(-6) : null,
      models: input.models ?? demoLib.find((x) => x.id === input.id)?.models ?? [],
    };
    demoLib = [...demoLib.filter((x) => x.id !== e.id), e];
    return e;
  },
  libraryDelete: async (id) => { demoLib = demoLib.filter((x) => x.id !== id); },
  openDataDir: async () => undefined,
  quitApp: async () => undefined,
  detectAgents: async () => (await fixture()).map((a) => ({
    id: a.id, name: a.name, appFound: a.installed, version: a.version, running: a.running,
    defaultDir: a.configDir, customDir: null, configDir: a.configDir, configFound: true, enabled: a.installed, note: null,
  } as AgentDetect)).concat((["hermes", "gemini", "pi", "openclaw", "droid", "kilo", "codebuddy", "qwen", "kimi"] as AgentId[]).map((id, i) => ({
    id, name: id, appFound: i < 2, version: i < 2 ? "1.0.0" : null, running: false, defaultDir: `~/.${id}`, customDir: null,
    configDir: `~/.${id}`, configFound: i < 2, enabled: i < 2, note: null,
  })), [{
    id: "trae", name: "Trae", appFound: true, version: "3.5.78", running: false, defaultDir: "%APPDATA%\\Trae", customDir: null,
    configDir: "%APPDATA%\\Trae", configFound: true, enabled: false, manual: true,
    note: "Trae 的自定义模型登记在账号云端，AgentPlus 无法代为写入。手动添加：Trae 设置 → 模型 → 添加模型。",
  }]),
  setAgentDir: async () => undefined,
  projectsList: async () => demoProjects,
  projectOpen: async (path) => {
    const p = path.trim().replace(/[\\/]+$/, "");
    const e: ProjectEntry = demoProjects.find((x) => x.path === p) ?? { path: p, name: p.split(/[\\/]/).pop() ?? p, agent: `${PROJECT_PREFIX}${p}` as AgentId, lastOpened: null, exists: true, config: null, providers: 0, git: false };
    const next = { ...e, lastOpened: new Date().toISOString() };
    demoProjects = [next, ...demoProjects.filter((x) => x.path !== p)];
    return next;
  },
  projectForget: async (path) => { demoProjects = demoProjects.filter((x) => x.path !== path); },
  pickFolder: async () => "D:\\xm\\newapp",
  updateCheck: async () => {
    await sleep(800);
    return { version: "0.2.0", current: "0.1.0", notes: "（演示）\n- 新功能：应用内更新\n- 修复若干问题", date: new Date().toISOString() };
  },
  updateInstall: async (onProgress) => {
    const total = 9_400_000;
    for (let done = 0; done < total; done += 700_000) {
      onProgress({ kind: "download", done, total });
      await sleep(120);
    }
    onProgress({ kind: "install" });
    await sleep(1000);
    throw new Error("（演示）浏览器预览里不能安装");
  },
  officialStatus: async () => ({ ...demoOfficial, cacheReady: demoOfficial.active && Date.now() - demoOfficialAt > 4000, cacheModels: 9 }),
  officialStart: async () => { demoOfficial.active = true; demoOfficialAt = Date.now(); return { ...demoOfficial }; },
  officialFinish: async () => { demoOfficial.active = false; return ["gpt-6-astra", "gpt-6-sol", "gpt-6-luna", "gpt-5.6-sol", "gpt-5.5", "codex-auto-review"].map((slug, i) => ({ slug, name: slug.toUpperCase(), visible: i !== 5 })); },
  officialCancel: async () => { demoOfficial.active = false; },
  gatewayStatus: async () => demoGw(),
  gatewaySet: async (enabled, port) => { demoGateway.enabled = enabled; if (port) demoGateway.port = port; return demoGw(); },
  gatewaySaveRoute: async (route, oldId) => { demoGateway.routes = [...demoGateway.routes.filter((r) => r.id !== (oldId ?? route.id)), route]; return demoGw(); },
  gatewayDeleteRoute: async (id) => { demoGateway.routes = demoGateway.routes.filter((r) => r.id !== id); return demoGw(); },
  gatewaySetBreaker: async (b) => { demoGateway.breaker = b; return demoGw(); },
  gatewayResetBreaker: async (id) => { demoGateway.cleared.push(...(id === null ? demoGateway.routes.map((r) => r.id) : [id])); return demoGw(); },
  gatewayTest: async (route, apiKind, model) => {
    await sleep(600);
    return { ok: true, status: 200, ms: 980, model, url: `http://127.0.0.1:${demoGateway.port}/${route}/v1/${apiKind === "chat" ? "chat/completions" : apiKind === "anthropic" ? "messages" : "responses"}`, reply: "pong", error: null, usage: [12, 2] };
  },
  testProvider: async (_a, _p, model) => {
    await sleep(700);
    return model.includes("bad")
      ? { ok: false, status: 404, ms: 412, model, url: "http://demo/v1/responses", reply: null, error: "地址或接口类型不对，或者没有这个模型（HTTP 404）：model not found", usage: null }
      : { ok: true, status: 200, ms: 1234, model, url: "http://demo/v1/responses", reply: "pong", error: null, usage: [14, 3] };
  },
};

// `import.meta.env.DEV` is a build-time constant: production bundles drop the demo.
export const api = inTauri || !import.meta.env.DEV ? real : demo;
