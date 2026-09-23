import { invoke } from "@tauri-apps/api/core";

export type AgentId = "codex" | "zcode" | "mimo";

export interface Model {
  id: string;
  visible: boolean;
  readonly: boolean;
  tags: string[];
  ctx: string | null;
  name: string | null;
  context: number | null;
  deletable: boolean;
}

export type ApiKind = "responses" | "chat" | "anthropic";

export interface ProviderInput {
  id: string | null;
  name: string;
  baseUrl: string;
  api: ApiKind;
  apiKey: string | null;
  models: string[];
}

export interface ModelInput {
  id: string;
  name: string | null;
  context: number | null;
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
}

export interface Setting {
  key: string;
  group: string;
  label: string;
  desc: string;
  kind: "bool" | "chips";
  value: boolean | string[];
  options: string[];
  /** One short explanation per option (may be empty). */
  hints: string[];
}

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
  notes: string[];
  readonly: boolean;
  /** Codex: not on the fixed id yet, but could be. */
  fixedPending: boolean;
  /** Codex: prefill "turn on fixed id" as a pending change (not declined before). */
  fixedPrompt: boolean;
}

export type Op =
  | { op: "set_current_provider"; provider: string }
  | { op: "set_provider_enabled"; provider: string; enabled: boolean }
  | { op: "set_model_visible"; provider: string; model: string; visible: boolean }
  | { op: "set_setting"; key: string; value: boolean | string[] }
  | { op: "upsert_provider"; provider: ProviderInput }
  | { op: "delete_provider"; provider: string }
  | { op: "upsert_model"; provider: string; model: ModelInput }
  | { op: "delete_model"; provider: string; model: string }
  | { op: "import_provider"; fromAgent: string; provider: string; api?: ApiKind; name?: string };

/** Where AgentPlus reads and writes configs. */
export interface EnvInfo {
  id: string;
  label: string;
  detail: string;
  current: boolean;
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
}

/** A provider in AgentPlus's shared library (the key stays in the backend). */
export interface LibEntry {
  id: string;
  name: string;
  baseUrl: string;
  api: ApiKind;
  hasKey: boolean;
  keyHint: string | null;
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

export interface ApplyResult {
  state: AgentState;
  files: string[];
  backupDir: string | null;
}

const real = {
  listAgents: () => invoke<AgentState[]>("list_agents"),
  getAgent: (agent: AgentId) => invoke<AgentState>("get_agent", { agent }),
  preview: (agent: AgentId, ops: Op[]) => invoke<DiffGroup[]>("preview", { agent, ops }),
  apply: (agent: AgentId, ops: Op[]) => invoke<ApplyResult>("apply", { agent, ops }),
  testLatency: (url: string) => invoke<number>("test_latency", { url }),
  restart: (agent: AgentId) => invoke<string>("restart_agent", { agent }),
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
  listBackups: () => invoke<BackupEntry[]>("list_backups"),
  restoreBackup: (id: string) => invoke<string>("restore_backup", { id }),
  syncStatus: () => invoke<SyncStatus>("sync_status"),
  syncSetFolder: (path: string) => invoke<void>("sync_set_folder", { path }),
  syncExport: () => invoke<string>("sync_export"),
  syncPreview: () => invoke<SyncSuggestion[]>("sync_preview"),
  openPath: (path: string) => invoke<void>("open_path", { path }),
  dismissFixedPrompt: () => invoke<void>("codex_dismiss_fixed_prompt"),
  listEnvs: () => invoke<EnvInfo[]>("list_envs"),
  setEnv: (id: string) => invoke<void>("set_env", { id }),
  libraryList: () => invoke<LibEntry[]>("library_list"),
  librarySave: (input: LibInput) => invoke<LibEntry>("library_save", { input }),
  libraryDelete: (id: string) => invoke<void>("library_delete", { id }),
  openDataDir: () => invoke<void>("open_data_dir"),
  detectAgents: () => invoke<AgentDetect[]>("detect_agents"),
  setAgentDir: (agent: AgentId, path: string | null) => invoke<void>("set_agent_dir", { agent, path }),
};

// Plain-browser preview (`npm run dev`): serve a static snapshot so the UI can be
// checked without the Tauri backend. Never used inside the app.
const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function fixture(): Promise<AgentState[]> {
  const m = await import("./dev-fixture.json");
  return m.default as unknown as AgentState[];
}

let demoEnv = "windows";
let demoLib: LibEntry[] = [];

const demo: typeof real = {
  listAgents: fixture,
  getAgent: async (agent) => (await fixture()).find((a) => a.id === agent)!,
  preview: async (_agent, ops) => [{ file: "（演示）", lines: ops.map((o) => ({ text: JSON.stringify(o), add: true })) }],
  apply: async (agent) => ({ state: (await fixture()).find((a) => a.id === agent)!, files: [], backupDir: null }),
  testLatency: async () => 120 + Math.round(Math.random() * 300),
  restart: async () => "（演示）已重启",
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
  listBackups: async () => [
    { id: "20260923-140512/codex", stamp: "20260923-140512", agent: "codex", reason: "应用配置", files: [{ name: "config.toml", path: "C:\\Users\\me\\.codex\\config.toml" }], bytes: 10240, restorable: true },
    { id: "20260923-131201/zcode", stamp: "20260923-131201", agent: "zcode", reason: "应用配置", files: [{ name: "provider_config.json", path: "C:\\Users\\me\\.zcode\\v2\\provider_config.json" }], bytes: 19329, restorable: true },
  ],
  restoreBackup: async () => "（演示）已回滚",
  syncStatus: async () => ({ folder: null, fileExists: false, exportedAt: null, machine: null }),
  syncSetFolder: async () => undefined,
  syncExport: async () => "（演示）已导出",
  syncPreview: async () => [],
  openPath: async () => undefined,
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
      models: input.models ?? demoLib.find((x) => x.id === input.id)?.models ?? [],
    };
    demoLib = [...demoLib.filter((x) => x.id !== e.id), e];
    return e;
  },
  libraryDelete: async (id) => { demoLib = demoLib.filter((x) => x.id !== id); },
  openDataDir: async () => undefined,
  detectAgents: async () => (await fixture()).map((a) => ({
    id: a.id, name: a.name, appFound: a.installed, version: a.version, running: a.running,
    defaultDir: a.configDir, customDir: null, configDir: a.configDir, configFound: true, enabled: a.installed, note: null,
  })),
  setAgentDir: async () => undefined,
};

export const api = inTauri ? real : demo;
