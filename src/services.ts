// The provider hub: one "service" per address, merging AgentPlus's library with
// every agent's own provider entries (as they will be after the pending drafts).
import type { AgentId, AgentState, ApiKind, LibEntry, Op } from "./api";
import { CATALOG, type Draft, type ViewProvider, currentProvider, isEnabled, isVisible, viewModels, viewProviders } from "./draft";

export type UseState = "current" | "on" | "off" | "adding" | "removing" | "new";

export interface Use {
  agent: AgentState;
  p: ViewProvider | null;
  /** Draft key of a pending "add to this agent" (import op). */
  importKey?: string;
  state: UseState;
  models: number;
}

export interface Service {
  key: string;
  name: string;
  baseUrl: string | null;
  host: string;
  lib: LibEntry | null;
  uses: Use[];
  /** Account login / built-in services: no address to share. */
  builtin: boolean;
  api: ApiKind;
  apis: ApiKind[];
}

export function hostKey(url: string | null): string {
  if (!url) return "";
  try {
    return new URL(url).host.toLowerCase();
  } catch {
    return url.toLowerCase();
  }
}

/** "OpenCode Zen — Responses" → "OpenCode Zen" */
function baseName(n: string): string {
  return n.split(/\s+[—–-]\s+/)[0].trim() || n;
}

function pickName(names: string[]): string {
  const count = new Map<string, number>();
  for (const n of names.map(baseName)) count.set(n, (count.get(n) ?? 0) + 1);
  let best = names[0] ? baseName(names[0]) : "";
  for (const [n, c] of count) if (c > (count.get(best) ?? 0)) best = n;
  return best;
}

function modelCount(a: AgentState, p: ViewProvider, d: Draft): number {
  // Codex shares one model catalog across its providers.
  if (a.catalog) return viewModels(CATALOG, a.catalog, d).filter((m) => !m.isDeleted && isVisible(CATALOG, m, d)).length;
  if (p.isNew) return p.models.length;
  return viewModels(p.id, p.models, d).filter((m) => !m.isDeleted && isVisible(p.id, m, d)).length;
}

function useState(a: AgentState, p: ViewProvider, d: Draft): UseState {
  if (p.isDeleted) return "removing";
  if (p.isNew) return "new";
  if (a.mode === "single") return currentProvider(a, d) === p.id ? "current" : "on";
  return isEnabled(p, d) ? "on" : "off";
}

/** Agents that can receive a provider right now. */
export function writableAgents(agents: AgentState[]): AgentState[] {
  return agents.filter((a) => a.installed && !a.readonly);
}

export function buildServices(agents: AgentState[], drafts: Record<string, Draft>, lib: LibEntry[]): Service[] {
  const map = new Map<string, Service>();
  const get = (key: string, init: () => Service) => {
    let s = map.get(key);
    if (!s) {
      s = init();
      map.set(key, s);
    }
    return s;
  };

  for (const e of lib) {
    get(hostKey(e.baseUrl) || `lib:${e.id}`, () => ({
      key: hostKey(e.baseUrl) || `lib:${e.id}`, name: e.name, baseUrl: e.baseUrl, host: hostKey(e.baseUrl),
      lib: e, uses: [], builtin: false, api: e.api, apis: [e.api],
    })).lib ??= e;
  }

  for (const a of agents) {
    const d = drafts[a.id] ?? {};
    for (const p of viewProviders(a, d)) {
      const hk = p.baseUrl ? hostKey(p.baseUrl) : "";
      const key = hk || `acct:${a.id}:${p.id}`;
      const s = get(key, () => ({
        key, name: "", baseUrl: p.baseUrl, host: hk || p.host, lib: null, uses: [], builtin: !p.baseUrl, api: p.api, apis: [],
      }));
      s.uses.push({ agent: a, p, state: useState(a, p, d), models: modelCount(a, p, d) });
      if (!s.apis.includes(p.api) && p.baseUrl) s.apis.push(p.api);
    }
  }

  // Pending "add to agent" imports show up as uses too.
  for (const a of agents) {
    for (const [k, op] of Object.entries(drafts[a.id] ?? {}) as [string, Op][]) {
      if (op.op !== "import_provider") continue;
      let s: Service | undefined;
      if (op.fromAgent === "library") s = [...map.values()].find((x) => x.lib?.id === op.provider);
      else {
        const src = agents.find((x) => x.id === op.fromAgent)?.providers.find((p) => p.id === op.provider);
        if (src) s = map.get(hostKey(src.baseUrl) || `acct:${op.fromAgent}:${src.id}`);
      }
      s?.uses.push({ agent: a, p: null, importKey: k, state: "adding", models: 0 });
    }
  }

  for (const s of map.values()) {
    if (!s.name) s.name = s.lib?.name ?? pickName(s.uses.map((u) => u.p?.name ?? "").filter(Boolean));
    if (!s.lib && s.apis.length) s.api = s.apis.includes("responses") ? "responses" : s.apis[0];
  }

  const score = (s: Service) => s.uses.filter((u) => u.state !== "adding").length;
  return [...map.values()].sort((x, y) => Number(x.builtin) - Number(y.builtin) || score(y) - score(x) || x.name.localeCompare(y.name));
}

/** A place the service's address and key can be copied from, for a new agent entry. */
export function importSource(s: Service): { fromAgent: string; provider: string } | null {
  if (s.lib) return { fromAgent: "library", provider: s.lib.id };
  const u = s.uses.find((x) => x.p && !x.p.isNew && x.p.editable && x.p.baseUrl);
  return u ? { fromAgent: u.agent.id, provider: u.p!.id } : null;
}

export function importOp(s: Service, to: AgentId): Op | null {
  const src = importSource(s);
  if (!src) return null;
  return { op: "import_provider", ...src, api: to === "codex" ? "responses" : s.api, name: s.name };
}

export function importKey(s: Service): string {
  const src = importSource(s);
  return `pi:${src?.fromAgent}:${src?.provider}`;
}

export const USE_LABEL: Record<UseState, string> = {
  current: "当前使用",
  on: "已接入",
  off: "已停用",
  adding: "待添加",
  removing: "待移除",
  new: "新 · 未应用",
};
