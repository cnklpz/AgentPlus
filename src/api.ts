import { invoke } from "@tauri-apps/api/core";

export type AgentId = "codex" | "zcode" | "mimo";

export interface Model {
  id: string;
  visible: boolean;
  readonly: boolean;
  tags: string[];
  ctx: string | null;
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
}

export interface Setting {
  key: string;
  group: string;
  label: string;
  desc: string;
  kind: "bool" | "chips";
  value: boolean | string[];
  options: string[];
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
}

export type Op =
  | { op: "set_current_provider"; provider: string }
  | { op: "set_provider_enabled"; provider: string; enabled: boolean }
  | { op: "set_model_visible"; provider: string; model: string; visible: boolean }
  | { op: "set_setting"; key: string; value: boolean | string[] };

export interface DiffGroup {
  file: string;
  lines: { text: string; add: boolean }[];
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
};

// Plain-browser preview (`npm run dev`): serve a static snapshot so the UI can be
// checked without the Tauri backend. Never used inside the app.
const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function fixture(): Promise<AgentState[]> {
  const m = await import("./dev-fixture.json");
  return m.default as unknown as AgentState[];
}

const demo: typeof real = {
  listAgents: fixture,
  getAgent: async (agent) => (await fixture()).find((a) => a.id === agent)!,
  preview: async (_agent, ops) => [{ file: "（演示）", lines: ops.map((o) => ({ text: JSON.stringify(o), add: true })) }],
  apply: async (agent) => ({ state: (await fixture()).find((a) => a.id === agent)!, files: [], backupDir: null }),
  testLatency: async () => 120 + Math.round(Math.random() * 300),
  restart: async () => "（演示）已重启",
  openConfigDir: async () => undefined,
};

export const api = inTauri ? real : demo;
