// The provider hub. A *station* is one service host (e.g. a relay); a relay often has
// several *groups* — different protocols, paths or keys. Each group is edited and added
// to agents on its own. Built from AgentPlus's library plus every agent's entries (as
// they will be after the pending drafts).
import type { AgentId, AgentState, ApiKind, GatewayBreakerView, GatewayRoute, GatewayRouteView, LibEntry, Op, ProviderInput, SyncSuggestion } from "./api";
import { API_LABEL, CATALOG, type Draft, type ViewProvider, currentProvider, isEnabled, keys, providerModelCount, viewProviders, visibleModelCount } from "./draft";
import { type TKey, t } from "./i18n";

export type UseState = "current" | "on" | "off" | "adding" | "removing" | "new";

export interface Use {
  agent: AgentState;
  p: ViewProvider | null;
  /** Draft key of a pending "add to this agent" (import op). */
  importKey?: string;
  state: UseState;
  models: number;
}

/** One way into a station: address + protocol + key. */
export interface Group {
  key: string;
  name: string;
  baseUrl: string;
  api: ApiKind;
  keyFp: string | null;
  keyHint: string | null;
  lib: LibEntry | null;
  uses: Use[];
}

export interface Station {
  key: string;
  name: string;
  host: string;
  /** Address used for latency (first group's). */
  baseUrl: string | null;
  /** Account login / built-in: no address to share. */
  builtin: boolean;
  groups: Group[];
}

export function hostKey(url: string | null): string {
  if (!url) return "";
  try {
    return new URL(url).host.toLowerCase();
  } catch {
    return url.toLowerCase();
  }
}

function normUrl(url: string): string {
  return url.trim().replace(/\/+$/, "").toLowerCase();
}

export function groupKey(baseUrl: string, api: string, keyFp: string | null): string {
  return `${normUrl(baseUrl)}|${api}|${keyFp ?? ""}`;
}

/** "OpenCode Zen — Responses" → "OpenCode Zen" */
function baseName(n: string): string {
  return n.split(/\s+[—–-]\s+/)[0].trim() || n;
}

function mostCommon(names: string[]): string {
  const count = new Map<string, number>();
  for (const n of names) count.set(n, (count.get(n) ?? 0) + 1);
  let best = names[0] ?? "";
  for (const [n, c] of count) if (c > (count.get(best) ?? 0)) best = n;
  return best;
}

function modelCount(a: AgentState, p: ViewProvider, d: Draft): number {
  // Codex shares one model catalog across its providers.
  return a.catalog ? visibleModelCount(CATALOG, a.catalog, d) : providerModelCount(p, d);
}

function stateOfUse(a: AgentState, p: ViewProvider, d: Draft): UseState {
  if (p.isDeleted) return "removing";
  if (p.isNew) return "new";
  if (a.mode === "single") return currentProvider(a, d) === p.id ? "current" : "on";
  return isEnabled(p, d) ? "on" : "off";
}

/** Agents that can receive a provider right now. */
export function writableAgents(agents: AgentState[]): AgentState[] {
  return agents.filter((a) => a.installed && !a.readonly);
}

/** A group's uses that stay (not being removed). */
export function liveUses(g: Group): Use[] {
  return g.uses.filter((u) => u.state !== "removing");
}

/** Stable React key / set member for a use: its agent and provider. */
export function useKey(u: Use): string {
  return `${u.agent.id}:${u.p?.id}`;
}

/** Writable agents the group isn't in yet (or is only being removed from). */
export function freeAgents(agents: AgentState[], g: Group | null): AgentState[] {
  const uses = g ? liveUses(g) : [];
  return writableAgents(agents).filter((a) => !uses.some((u) => u.agent.id === a.id));
}

/** Stations with an address (relays, shown first) and account logins / built-ins. */
export function splitStations(stations: Station[]): { relays: Station[]; accounts: Station[] } {
  return { relays: stations.filter((s) => !s.builtin), accounts: stations.filter((s) => s.builtin) };
}

/** Key for agent entries that point at the gateway; writing swaps in that agent's own gateway key. */
export const GATEWAY_KEY = "agentplus-gateway";

/** The gateway's port until the user picks another one. */
export const DEFAULT_GATEWAY_PORT = 18650;

/** Gateway hosts: the current "127.0.0.1:<port>" first, then earlier ports whose addresses still count as the gateway. */
export type GatewayHosts = string | readonly string[] | null;

/** A new agent entry pointing at a gateway address (Codex keeps its shared catalog: no model list). */
export function gatewayEntry(localBase: string, agent: AgentId, api: ApiKind, name: string, models: string[]): ProviderInput {
  return { id: null, name, baseUrl: localBase, api, apiKey: GATEWAY_KEY, models: agent === "codex" ? [] : models };
}

/** Paused by the error breaker (or waiting for its trial request). */
export function tripped(r: GatewayRouteView): GatewayBreakerView | null {
  return r.breaker && r.breaker.state !== "closed" ? r.breaker : null;
}

/** The forward to a library entry at one upstream protocol. */
export function findRoute<R extends GatewayRoute>(routes: R[], libId: string | null | undefined, upstreamApi: ApiKind): R | undefined {
  return routes.find((r) => r.library === libId && r.upstreamApi === upstreamApi);
}

/** Id for a new forward from its name: a lowercase ASCII slug ("route" when none is left), "-2", "-3"… when taken. */
export function newRouteId(name: string, taken: readonly { id: string }[]): string {
  const base = name.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "") || "route";
  let id = base;
  for (let n = 2; taken.some((x) => x.id === id); n++) id = `${base}-${n}`;
  return id;
}

/** A forward's list of replaced [agent, provider] entries with `add` appended (each pair once). */
export function mergeReplaced(prev: readonly [string, string][] | null | undefined, add: readonly [string, string][]): [string, string][] {
  const out: [string, string][] = [...(prev ?? [])];
  for (const [a, p] of add) if (!out.some(([x, y]) => x === a && y === p)) out.push([a, p]);
  return out;
}

/** Is `host` ("127.0.0.1:18650", or with localhost) one of the gateway's hosts? */
export function isGatewayHost(host: string, hosts: GatewayHosts): boolean {
  const h = host.toLowerCase().replace("localhost", "127.0.0.1");
  return hosts !== null && (typeof hosts === "string" ? h === hosts : hosts.includes(h));
}

/** "http://127.0.0.1:18650/<route>/v1" → route id, when it points at our gateway. */
export function gatewayRouteId(url: string | null, gatewayHost: GatewayHosts): string | null {
  if (!url || !gatewayHost) return null;
  const m = url.match(/^https?:\/\/([^/]+)\/([^/+]+)\/v1\/?$/i);
  return m && isGatewayHost(m[1], gatewayHost) ? m[2] : null;
}

/**
 * The same gateway address at `port`, for an address at one of `fromPorts` (a forward, the
 * unified entry or a combined address); null for anything else.
 */
export function movedGatewayUrl(url: string | null | undefined, fromPorts: readonly number[], port: number): string | null {
  const m = url?.match(/^(https?:\/\/)(?:127\.0\.0\.1|localhost):(\d+)(\/.*)$/i);
  if (!m || !fromPorts.includes(Number(m[2]))) return null;
  const host = `127.0.0.1:${m[2]}`;
  if (gatewayRouteId(url!, host) === null && gatewayPoolIds(url) === null) return null;
  return `${m[1]}127.0.0.1:${port}${m[3]}`;
}

/**
 * Gateway address that spreads requests over several forwards: the unified entry for
 * none, the forward's own address for one, "/<a>+<b>/v1" for several.
 */
export function gatewayPoolBase(port: number, ids: string[]): string {
  const base = `http://127.0.0.1:${port}`;
  return ids.length === 0 ? `${base}/v1` : `${base}/${ids.join("+")}/v1`;
}

/** Forwards a gateway address uses: [] for the unified entry, null when it is not a unified/combined address. */
export function gatewayPoolIds(url: string | null | undefined): string[] | null {
  const m = url?.match(/^https?:\/\/(?:127\.0\.0\.1|localhost):\d+\/(?:([a-z0-9-]+(?:\+[a-z0-9-]+)+)\/)?v1\/?$/i);
  return m ? (m[1] ? m[1].split("+") : []) : null;
}

export function buildStations(agents: AgentState[], drafts: Record<string, Draft>, lib: LibEntry[], gatewayHost: GatewayHosts = null): Station[] {
  const stations = new Map<string, Station>();
  const groups = new Map<string, Group>();

  const station = (key: string, init: () => Station) => {
    let s = stations.get(key);
    if (!s) stations.set(key, (s = init()));
    return s;
  };
  const group = (st: Station, key: string, init: () => Group) => {
    let g = groups.get(key);
    if (!g) {
      groups.set(key, (g = init()));
      st.groups.push(g);
    }
    return g;
  };

  for (const e of lib) {
    const hk = hostKey(e.baseUrl) || `lib:${e.id}`;
    const st = station(hk, () => ({ key: hk, name: "", host: hostKey(e.baseUrl), baseUrl: e.baseUrl, builtin: false, groups: [] }));
    const gk = groupKey(e.baseUrl, e.api, e.keyFp);
    group(st, gk, () => ({ key: gk, name: e.name, baseUrl: e.baseUrl, api: e.api, keyFp: e.keyFp, keyHint: e.keyHint, lib: e, uses: [] })).lib ??= e;
  }

  for (const a of agents) {
    const d = drafts[a.id] ?? {};
    for (const p of viewProviders(a, d)) {
      const use: Use = { agent: a, p, state: stateOfUse(a, p, d), models: modelCount(a, p, d) };
      if (!p.baseUrl) {
        // Account login: its own station with a single group.
        const key = `acct:${a.id}:${p.id}`;
        const st = station(key, () => ({ key, name: p.name, host: p.host, baseUrl: null, builtin: true, groups: [] }));
        group(st, key, () => ({ key, name: p.name, baseUrl: "", api: p.api, keyFp: null, keyHint: null, lib: null, uses: [] })).uses.push(use);
        continue;
      }
      const hk = hostKey(p.baseUrl);
      const gw = isGatewayHost(hk, gatewayHost);
      const st = station(hk, () => ({ key: hk, name: gw ? t("services.gatewayStation") : "", host: hk, baseUrl: p.baseUrl, builtin: false, groups: [] }));
      // Every agent has its own gateway key, so the key doesn't split gateway addresses.
      const fp = gw ? null : p.keyFp;
      const gk = groupKey(p.baseUrl, p.api, fp);
      group(st, gk, () => ({ key: gk, name: "", baseUrl: p.baseUrl!, api: p.api, keyFp: fp, keyHint: fp ? p.keyHint : null, lib: null, uses: [] })).uses.push(use);
    }
  }

  // Pending "add to agent" imports show up under the group they copy from.
  for (const a of agents) {
    for (const [k, op] of Object.entries(drafts[a.id] ?? {}) as [string, Op][]) {
      if (op.op !== "import_provider") continue;
      let g: Group | undefined;
      if (op.fromAgent === "library") g = [...groups.values()].find((x) => x.lib?.id === op.provider);
      else g = [...groups.values()].find((x) => x.uses.some((u) => u.agent.id === op.fromAgent && u.p?.id === op.provider));
      g?.uses.push({ agent: a, p: null, importKey: k, state: "adding", models: 0 });
    }
  }

  for (const st of stations.values()) {
    for (const g of st.groups) {
      if (!g.name) g.name = g.lib?.name ?? mostCommon(g.uses.map((u) => u.p?.name ?? "").filter(Boolean));
    }
    if (!st.name) st.name = mostCommon(st.groups.map((g) => baseName(g.name)).filter(Boolean)) || st.host;
    st.groups.sort((x, y) => live(y) - live(x) || x.name.localeCompare(y.name));
  }

  const score = (s: Station) => s.groups.reduce((n, g) => n + live(g), 0);
  return [...stations.values()].sort((x, y) => Number(x.builtin) - Number(y.builtin) || score(y) - score(x) || x.name.localeCompare(y.name));
}

function live(g: Group): number {
  return g.uses.filter((u) => u.state !== "adding" && u.state !== "removing").length;
}

/** A place the group's address and key can be copied from, for a new agent entry. */
export function importSource(g: Group): { fromAgent: string; provider: string } | null {
  if (g.lib) return { fromAgent: "library", provider: g.lib.id };
  const u = g.uses.find((x) => x.p && !x.p.isNew && x.p.editable && x.p.baseUrl);
  return u ? { fromAgent: u.agent.id, provider: u.p!.id } : null;
}

/** Agents that accept only one protocol; everything else takes all three. */
export const ONLY_API: Partial<Record<AgentId, ApiKind>> = { codex: "responses", claude: "anthropic", codebuddy: "chat", gemini: "gemini" };

/** Can the local gateway serve this agent? (It speaks Chat / Responses / Anthropic, not Gemini.) */
export function gatewayCapable(agent: AgentId): boolean {
  return ONLY_API[agent] !== "gemini";
}

/** Protocol an agent should use to reach a provider speaking `api` (directly or via the gateway). */
export function apiFor(agent: AgentId, api: ApiKind): ApiKind {
  return ONLY_API[agent] ?? api;
}

export const AGENT_NAME: Record<AgentId, string> = {
  codex: "Codex", claude: "Claude Code", opencode: "OpenCode", zcode: "ZCode", mimo: "MiMo Desktop",
  hermes: "Hermes", gemini: "Gemini CLI", pi: "pi", openclaw: "OpenClaw", qwen: "Qwen Code", kimi: "Kimi Code",
  droid: "Droid", codebuddy: "CodeBuddy", kilo: "Kilo Code", trae: "Trae",
};

/** Product name of an agent id; anything else (e.g. "codex@wsl") as it is. */
export function agentLabel(id: string): string {
  return (AGENT_NAME as Record<string, string>)[id] ?? id;
}

/** Why a group cannot be added to an agent (null = it can). */
export function cannotAdd(g: Group, to: AgentId): string | null {
  const only = ONLY_API[to];
  if (only === "gemini" && g.api !== "gemini") return t("services.geminiOnly", { agent: AGENT_NAME[to] });
  if (only && g.api !== only) return t("services.apiOnly", { agent: AGENT_NAME[to], only: API_LABEL[only], api: API_LABEL[g.api] });
  if (!only && g.api === "gemini") return t("services.geminiGroup");
  if (!importSource(g)) return t("services.noSource");
  return null;
}

export function importOp(g: Group, to: AgentId): Op | null {
  const src = importSource(g);
  if (!src || cannotAdd(g, to)) return null;
  return { op: "import_provider", ...src, api: g.api, name: g.name };
}

/** Draft key of `importOp(g, …)` (only meaningful when that is not null). */
export function importKey(g: Group): string {
  const src = importSource(g);
  return src ? keys.importProvider(src.fromAgent, src.provider) : "";
}

export { API_LABEL };

/** Protocols a provider can be set up with, in the order the dialogs offer them. */
export const PROTOCOLS = ["responses", "chat", "anthropic"] as const;
export type Protocol = (typeof PROTOCOLS)[number];

/** A gateway route as saved: the view-only fields the backend adds are left out. */
export function plainRoute(r: GatewayRouteView): GatewayRoute {
  const { localBase: _a, upstreamName: _b, upstreamUrl: _c, upstreamMissing: _d, models: _e, breaker: _f, ...route } = r;
  return route;
}

/** Stable id of a sync suggestion (its agent and draft keys; its text changes with the language). */
export function syncSuggestionId(s: SyncSuggestion): string {
  return `${s.agent}\n${s.lib ? s.lib.key : s.ops.map(([k]) => k).join("\n")}`;
}

/** `syncSuggestionId` for a whole list, unique within it: two suggestions can touch the same
 * draft keys (two remote entries for one relay), so repeats get "#2", "#3"… in list order. */
export function syncSuggestionIds(list: SyncSuggestion[]): string[] {
  const seen = new Map<string, number>();
  return list.map((s) => {
    const id = syncSuggestionId(s);
    const n = (seen.get(id) ?? 0) + 1;
    seen.set(id, n);
    return n === 1 ? id : `${id}#${n}`;
  });
}

const USE_KEY: Record<UseState, TKey> = {
  current: "services.useCurrent",
  on: "services.useOn",
  off: "common.disabled",
  adding: "services.useAdding",
  removing: "services.useRemoving",
  new: "services.useNew",
};

/** Label per use state, translated when read (`USE_LABEL[state]`). */
export const USE_LABEL: Record<UseState, string> = new Proxy(USE_KEY as Record<string, string>, {
  get: (o, k) => (typeof k === "string" && k in o ? t(o[k] as TKey) : undefined),
});
