// Pending edits per agent, keyed so a toggle back to the original drops the op.
import type { AgentState, ApiKind, McpInput, McpServer, McpSource, Model, ModelFieldValue, ModelGuess, ModelInput, Op, Provider, ProviderInput, Setting, SettingValue } from "./api";
import { t } from "./i18n";

export type Draft = Record<string, Op>;

export const keys = {
  cur: () => "cur",
  enabled: (pid: string) => `pe:${pid}`,
  visible: (pid: string, mid: string) => `mv:${pid}|${mid}`,
  setting: (key: string) => `set:${key}`,
  upsertProvider: (pid: string) => `pu:${pid}`,
  deleteProvider: (pid: string) => `pd:${pid}`,
  upsertModel: (pid: string, mid: string) => `mu:${pid}|${mid}`,
  deleteModel: (pid: string, mid: string) => `md:${pid}|${mid}`,
  providerModels: (pid: string) => `pm:${pid}`,
  roles: (pid: string) => `pr:${pid}`,
  /** A pending copy of provider `pid` from `from` (another agent, or "library"). */
  importProvider: (from: string, pid: string) => `pi:${from}:${pid}`,
  /** A new entry pointing at gateway forward `routeId`. */
  gatewayProvider: (routeId: string) => `pu:gw-${routeId}`,
  mcpUpsert: (name: string) => `mcpu:${name}`,
  mcpDelete: (name: string) => `mcpd:${name}`,
  mcpEnabled: (name: string) => `mcpe:${name}`,
};

/** Codex has one global catalog; its models use provider "*". */
export const CATALOG = "*";

export function withOp(d: Draft, key: string, op: Op | null): Draft {
  const next = { ...d };
  if (op) next[key] = op;
  else delete next[key];
  return next;
}

/**
 * What is left of a draft once `sent` (a snapshot of it) has been written: only the changes
 * made while the write was running (keys added or edited since; ops are replaced on edit).
 */
export function draftAfterWrite(now: Draft, sent: Draft): Draft {
  const out: Draft = {};
  for (const [k, op] of Object.entries(now)) if (sent[k] !== op) out[k] = op;
  return out;
}

export function currentProvider(st: AgentState, d: Draft): string | null {
  const op = d[keys.cur()];
  return op && op.op === "set_current_provider" ? op.provider : st.currentProvider;
}

export function isEnabled(p: Provider, d: Draft): boolean {
  const op = d[keys.enabled(p.id)];
  return op && op.op === "set_provider_enabled" ? op.enabled : p.enabled;
}

export function isVisible(pid: string, m: Model, d: Draft): boolean {
  const op = d[keys.visible(pid, m.id)];
  return op && op.op === "set_model_visible" ? op.visible : m.visible;
}

export function settingValue(s: Setting, d: Draft): SettingValue {
  const op = d[keys.setting(s.key)];
  return op && op.op === "set_setting" ? op.value : s.value;
}

/** An agent's switch setting as applied (not the draft). */
export function settingOn(st: AgentState, key: string): boolean {
  return st.settings.find((s) => s.key === key)?.value === true;
}

/**
 * After writing `ops`: restart the agent (its auto_restart setting), unless it isn't running or
 * the only change was that setting itself (nothing the agent reads changed then).
 */
export function shouldAutoRestart(st: AgentState, ops: Op[]): boolean {
  return settingOn(st, "auto_restart") && st.running && ops.some((o) => !(o.op === "set_setting" && o.key === "auto_restart"));
}

/** A "list" setting (one entry per line) keeps its order; chips compare as sets. */
function sameValue(a: SettingValue, b: SettingValue, ordered: boolean): boolean {
  if (Array.isArray(a) && Array.isArray(b)) {
    if (ordered) return a.length === b.length && a.every((x, i) => x === b[i]);
    return [...a].sort().join("|") === [...b].sort().join("|");
  }
  return a === b;
}

export function setSetting(d: Draft, s: Setting, value: SettingValue): Draft {
  return withOp(d, keys.setting(s.key), sameValue(value, s.value, s.kind === "list") ? null : { op: "set_setting", key: s.key, value });
}

/** The switches `s` excludes that are on in the draft (turning `s` on turns them off). */
export function excludedOn(d: Draft, all: Setting[], s: Setting): Setting[] {
  return all.filter((o) => s.excludes?.includes(o.key) && settingValue(o, d) === true);
}

// Undo information belongs to the enabling op, stays out of the wire payload, and expires
// with the draft. Op identity also prevents undo from overwriting a later manual edit.
const excludedUndo = new WeakMap<Op, { key: string; before: Op | undefined; after: Op | undefined }[]>();

/**
 * Like `setSetting`, for mutually exclusive switches: turning one on turns off the ones it
 * excludes; turning it off again before applying puts those back as they were.
 */
export function setSettingIn(d: Draft, all: Setting[], s: Setting, value: SettingValue): Draft {
  if (settingValue(s, d) === value) return d;
  const previous = d[keys.setting(s.key)];
  let out = setSetting(d, s, value);
  if (value === true) {
    const undo = [];
    for (const o of excludedOn(out, all, s)) {
      const key = keys.setting(o.key);
      const before = out[key];
      out = setSetting(out, o, false);
      undo.push({ key, before, after: out[key] });
    }
    const enabling = out[keys.setting(s.key)];
    if (enabling && undo.length) excludedUndo.set(enabling, undo);
  } else if (value === false && previous) {
    for (const { key, before, after } of excludedUndo.get(previous) ?? []) {
      if (out[key] === after) out = withOp(out, key, before ?? null);
    }
  }
  return out;
}

/** Number of pending changes in a draft. */
export function opCount(d: Draft | undefined): number {
  return d ? Object.keys(d).length : 0;
}

/** Pending changes over all agents. */
export function pendingTotal(drafts: Record<string, Draft>): number {
  return Object.values(drafts).reduce((n, d) => n + opCount(d), 0);
}

/** The agents that have pending changes. */
export function agentsWithOps<T extends { id: string }>(agents: T[], drafts: Record<string, Draft>): T[] {
  return agents.filter((a) => opCount(drafts[a.id]) > 0);
}

// ---------------------------------------------------------------- provider / model edits

export interface ViewProvider extends Provider {
  isNew?: boolean;
  isEdited?: boolean;
  isDeleted?: boolean;
  /** Draft key of a not-yet-applied new provider. */
  draftKey?: string;
}

export interface ViewModel extends Model {
  isNew?: boolean;
  isEdited?: boolean;
  isDeleted?: boolean;
}

// Some agents keep protocols AgentPlus doesn't model (e.g. pi's "bedrock-converse"); show those as-is.
export const API_LABEL: Record<ApiKind, string> = new Proxy(
  { responses: "Responses", chat: "Chat", anthropic: "Anthropic", gemini: "Gemini" } as Record<string, string>,
  { get: (o, k) => (typeof k === "string" ? o[k] ?? k : undefined) },
);

function hostOf(url: string): string {
  try {
    const u = new URL(url);
    return u.host + u.pathname.replace(/\/$/, "");
  } catch {
    return url;
  }
}

/**
 * Providers as they will be after the draft: new ones added, edits overlaid, deletions marked.
 * With `withImports`, pending copies (import_provider) show as new cards too (project pages;
 * the provider hub tracks them on its own).
 */
export function viewProviders(st: AgentState, d: Draft, withImports = false): ViewProvider[] {
  const out: ViewProvider[] = st.providers.map((p) => {
    const up = d[keys.upsertProvider(p.id)];
    const del = !!d[keys.deleteProvider(p.id)];
    if (up && up.op === "upsert_provider") {
      const i = up.provider;
      return {
        ...p, name: i.name, baseUrl: i.baseUrl, host: hostOf(i.baseUrl), api: i.api, apis: [API_LABEL[i.api]],
        hasKey: p.hasKey || !!i.apiKey, isEdited: true, isDeleted: del,
        // A new key is not fingerprinted until applied.
        keyFp: i.apiKey ? null : p.keyFp, keyHint: i.apiKey ? null : p.keyHint,
        officialAuth: i.officialAuth ?? p.officialAuth,
      };
    }
    return { ...p, isDeleted: del };
  });
  for (const [k, op] of Object.entries(d)) {
    if (withImports && op.op === "import_provider") {
      const api = op.api ?? "chat";
      out.push({
        id: k, draftKey: k, name: op.name ?? op.provider, baseUrl: null, host: t("draft.copiedFrom", { from: op.label ?? op.fromAgent }), apis: [API_LABEL[api]],
        builtin: false, enabled: true, compatible: true, reason: null, models: [], details: [], editable: false, api, hasKey: true, keyFp: null, keyHint: null, officialAuth: false, isNew: true,
      });
      continue;
    }
    if (op.op !== "upsert_provider" || op.provider.id !== null) continue;
    const i = op.provider;
    out.push({
      id: k, draftKey: k, name: i.name, baseUrl: i.baseUrl, host: hostOf(i.baseUrl), apis: [API_LABEL[i.api]],
      builtin: false, enabled: true, compatible: st.id !== "codex" || i.api === "responses", reason: null,
      models: i.models.map((id) => ({ id, visible: true, readonly: true, tags: [], ctx: null, name: null, context: null, deletable: false })),
      details: [], editable: true, api: i.api, hasKey: !!i.apiKey, keyFp: null, keyHint: null, officialAuth: !!i.officialAuth, isNew: true,
    });
  }
  return out;
}

export function viewModels(pid: string, models: Model[], d: Draft): ViewModel[] {
  const out: ViewModel[] = models.map((m) => {
    const up = d[keys.upsertModel(pid, m.id)];
    const del = !!d[keys.deleteModel(pid, m.id)];
    if (up && up.op === "upsert_model") {
      const ctx = up.model.context ?? m.context;
      return {
        ...m, name: up.model.name ?? m.name, context: ctx, ctx: ctx ? fmtCtx(ctx) : m.ctx, extra: mergeExtra(m.extra, up.model.extra),
        isEdited: true, isDeleted: del,
      };
    }
    return { ...m, isDeleted: del };
  });
  for (const op of Object.values(d)) {
    if (op.op !== "upsert_model" || op.provider !== pid || models.some((m) => m.id === op.model.id)) continue;
    const c = op.model.context;
    out.push({
      id: op.model.id, name: op.model.name, context: c, ctx: c ? fmtCtx(c) : null, extra: mergeExtra({}, op.model.extra),
      visible: true, readonly: false, tags: [], deletable: true, isNew: true,
    });
  }
  return out;
}

/** Model field values after a pending edit (null in the edit = cleared). */
export function mergeExtra(base: Model["extra"], edit: ModelInput["extra"]): Record<string, ModelFieldValue> {
  const out = { ...(base ?? {}) };
  for (const [k, v] of Object.entries(edit ?? {})) {
    if (v === null) delete out[k];
    else out[k] = v;
  }
  return out;
}

export function upsertProvider(d: Draft, input: ProviderInput, draftKey?: string): Draft {
  const key = input.id ? keys.upsertProvider(input.id) : draftKey ?? `pu:new-${Date.now()}`;
  return withOp(d, key, { op: "upsert_provider", provider: input });
}

export function upsertModel(d: Draft, pid: string, input: ModelInput): Draft {
  return withOp(d, keys.upsertModel(pid, input.id), { op: "upsert_model", provider: pid, model: input });
}

/** A new model with what the catalogs know about it (`api.guessModels`) filled in. */
export function guessedModel(id: string, g: ModelGuess | undefined): ModelInput {
  return { id, name: null, context: g?.context ?? null, ...(g && Object.keys(g.extra).length ? { extra: { ...g.extra } } : {}) };
}

export function deleteProvider(d: Draft, pid: string): Draft {
  return withOp(d, keys.deleteProvider(pid), { op: "delete_provider", provider: pid });
}

/**
 * Removes a provider card: a pending new one (not written yet, its id is a draft key) is just
 * dropped from the draft; an existing one gets a pending delete.
 */
export function removeProvider(d: Draft, p: ViewProvider): Draft {
  return p.isNew && p.draftKey ? withOp(d, p.draftKey, null) : deleteProvider(d, p.id);
}

/** A pending copy of provider `src.provider` from `src.fromAgent` (another agent, or "library"). */
export function importProvider(d: Draft, src: { fromAgent: string; provider: string; api: ApiKind; name: string; label?: string }): Draft {
  return withOp(d, keys.importProvider(src.fromAgent, src.provider), { op: "import_provider", ...src });
}

/** Turns a provider on / off; back to how it is applied = no pending change. */
export function setProviderEnabled(d: Draft, p: Pick<Provider, "id" | "enabled">, enabled: boolean): Draft {
  return withOp(d, keys.enabled(p.id), enabled === p.enabled ? null : { op: "set_provider_enabled", provider: p.id, enabled });
}

export function deleteModel(d: Draft, pid: string, mid: string): Draft {
  return withOp(d, keys.deleteModel(pid, mid), { op: "delete_model", provider: pid, model: mid });
}

/** Shows / hides a model in the agent's picker; back to how it is applied = no pending change. */
export function setModelVisible(d: Draft, pid: string, m: Model, visible: boolean): Draft {
  return withOp(d, keys.visible(pid, m.id), visible === m.visible ? null : { op: "set_model_visible", provider: pid, model: m.id, visible });
}

/**
 * Codex: the built-in catalog models a new list ticks by default: not AgentPlus's custom
 * ones, nor the ones Codex itself hides (internal or retired, tagged `codex-hidden`). A
 * catalog written before AgentPlus noted those falls back to what it shows now.
 */
export function codexDefaultModels(catalog: Model[]): string[] {
  const has = (m: Model, tag: string) => m.tags.some((g) => g.id === tag);
  const own = catalog.filter((m) => !has(m, "custom"));
  const pick = own.some((m) => has(m, "codex-hidden")) ? own.filter((m) => !has(m, "codex-hidden")) : own.filter((m) => m.visible);
  return (pick.length ? pick : own).map((m) => m.id);
}

/** Agents whose providers each have a default model (the backend tags it `role:default`). */
export const hasDefaultModel = (agent: string) => agent === "hermes";

/** A provider's default model, pending changes included (`pending`: a change is queued). */
export function defaultModel(d: Draft, pid: string, models: Model[]): { id: string | null; pending: boolean } {
  const op = d[keys.roles(pid)];
  const queued = op && op.op === "set_model_roles" ? op.roles.default : undefined;
  if (queued) return { id: queued, pending: true };
  return { id: models.find((m) => m.tags.some((g) => g.id === "role:default"))?.id ?? null, pending: false };
}

/** Makes `mid` the provider's default model; picking the saved one again drops the change. */
export function setDefaultModel(d: Draft, pid: string, models: Model[], mid: string): Draft {
  const saved = defaultModel(withOp(d, keys.roles(pid), null), pid, models).id;
  return withOp(d, keys.roles(pid), mid === saved ? null : { op: "set_model_roles", provider: pid, roles: { default: mid } });
}

/** 131072 -> "131K", 1048576 -> "1M" */
export function fmtCtx(n: number): string {
  // From 999.5K up, "K" would round to "1000K".
  if (n >= 999_500) {
    const m = n % 1_048_576 === 0 ? n / 1_048_576 : n / 1_000_000;
    return `${Math.round(m * 10) / 10}M`;
  }
  if (n >= 1000) return `${Math.round(n / 1000)}K`;
  return String(n);
}

/** "128k" -> 128000, "1m" -> 1000000, "200000" -> 200000 */
export function parseCtx(s: string): number | null {
  const v = s.trim().toLowerCase().replace(/,/g, "");
  if (!v) return null;
  const m = v.match(/^(\d+(?:\.\d+)?)\s*([km]?)$/);
  if (!m) return null;
  const n = parseFloat(m[1]) * (m[2] === "k" ? 1000 : m[2] === "m" ? 1_000_000 : 1);
  return Math.round(n);
}

/** Models of `pid` the agent will offer after the draft: not being deleted, not hidden. */
export function visibleModelCount(pid: string, models: Model[], d: Draft): number {
  return viewModels(pid, models, d).filter((m) => !m.isDeleted && isVisible(pid, m, d)).length;
}

/** `visibleModelCount` of a provider card; a new one offers every model it lists. */
export function providerModelCount(p: ViewProvider, d: Draft): number {
  return p.isNew ? p.models.length : visibleModelCount(p.id, p.models, d);
}

export function visibleCount(st: AgentState, d: Draft): number {
  if (st.catalog) return visibleModelCount(CATALOG, st.catalog, d);
  return viewProviders(st, d)
    .filter((p) => !p.isDeleted && (p.isNew || isEnabled(p, d)))
    .reduce((n, p) => n + providerModelCount(p, d), 0);
}

/**
 * What actually gets written for an agent. Codex's fixed provider id is pre-enabled:
 * it is not a pending change of its own, but rides along when the provider is
 * switched (unless the user set it explicitly or turned it off).
 */
export function opsToWrite(st: AgentState | undefined, d: Draft): Op[] {
  const ops = Object.values(d);
  const cur = d[keys.cur()];
  // The official OpenAI login cannot sit behind the fixed id.
  const switching = cur?.op === "set_current_provider" && cur.provider !== "openai";
  if (!st || st.id !== "codex" || !st.fixedPrompt || !switching || d[keys.setting("fixed_id")]) return ops;
  return [...ops, { op: "set_setting", key: "fixed_id", value: true }];
}

// ---------------------------------------------------------------- MCP servers

const mcpKeys = (name: string) => [keys.mcpUpsert(name), keys.mcpDelete(name), keys.mcpEnabled(name)];

function dropMcp(d: Draft, name: string): Draft {
  return mcpKeys(name).reduce((x, k) => withOp(x, k, null), d);
}

function pendingUpsert(d: Draft, name: string): McpInput | null {
  const op = d[keys.mcpUpsert(name)];
  return op?.op === "upsert_mcp" ? op.server : null;
}

/** Adds or replaces a server; a rename also drops the pending changes of the old name. */
export function upsertMcp(d: Draft, input: McpInput): Draft {
  let next = dropMcp(d, input.name);
  if (input.replaces && input.replaces !== input.name) next = dropMcp(next, input.replaces);
  return withOp(next, keys.mcpUpsert(input.name), { op: "upsert_mcp", server: input });
}

/**
 * Removes a server. One that only exists as a pending change just goes away; removing a
 * pending rename removes the server under its old name. `exists`: it is in the config.
 */
export function deleteMcp(d: Draft, name: string, exists: boolean): Draft {
  const old = pendingUpsert(d, name)?.replaces;
  const next = dropMcp(d, name);
  if (old && old !== name) return withOp(dropMcp(next, old), keys.mcpDelete(old), { op: "delete_mcp", name: old });
  return exists ? withOp(next, keys.mcpDelete(name), { op: "delete_mcp", name }) : next;
}

/** Turns a server on or off; back to `actual` (the config's state) drops the change. */
export function setMcpEnabled(d: Draft, name: string, enabled: boolean, actual: boolean | null): Draft {
  const up = pendingUpsert(d, name);
  if (up) return withOp(d, keys.mcpUpsert(name), { op: "upsert_mcp", server: { ...up, enabled } });
  return withOp(d, keys.mcpEnabled(name), enabled === actual ? null : { op: "set_mcp_enabled", name, enabled });
}

/** Drops every pending change to a server (and to the one a pending rename replaces). */
export function undoMcp(d: Draft, name: string): Draft {
  const old = pendingUpsert(d, name)?.replaces;
  return old ? dropMcp(dropMcp(d, name), old) : dropMcp(d, name);
}

/**
 * The order to write agents in: those that remove or rename MCP servers last, so a copy made
 * from one of them still finds the server (and its hidden values) when it is written.
 */
export function writeOrder<T extends { id: string }>(agents: T[], drafts: Record<string, Draft>): T[] {
  const removes = (a: T) => Object.values(drafts[a.id] ?? {}).some((op) => op.op === "delete_mcp" || (op.op === "upsert_mcp" && !!op.server.replaces && op.server.replaces !== op.server.name));
  return [...agents.filter((a) => !removes(a)), ...agents.filter(removes)];
}

export type McpPending = "new" | "edited" | "deleted" | "toggled";

/** A server as it will be once the agent's pending changes are written. */
export interface McpView {
  s: McpServer;
  pending: McpPending | null;
  /** In the config now (not only a pending addition). */
  exists: boolean;
}

/** What runs, compared on the values the page shows (masked the same way on both sides). */
function sameCore(i: McpInput, s: McpServer): boolean {
  const kv = (a: { key: string; value: string }[]) => JSON.stringify(a.map((p) => [p.key, p.value]));
  return i.transport === s.transport && (i.command ?? null) === s.command && JSON.stringify(i.args) === JSON.stringify(s.args)
    && (i.cwd ?? null) === s.cwd && (i.url ?? null) === s.url && kv(i.env) === kv(s.env) && kv(i.headers) === kv(s.headers);
}

/**
 * A pending server, shown like a read one. It keeps the source's `sig` when it runs the same
 * thing as the server it was copied or edited from, so it still groups with it.
 */
export function mcpFromInput(i: McpInput, source: McpServer | undefined): McpServer {
  const kv = (a: { key: string; value: string }[]) => a.map((p) => ({ ...p, secret: p.value.includes("•") }));
  return {
    name: i.name, transport: i.transport, command: i.command, args: i.args, cwd: i.cwd, url: i.url,
    env: kv(i.env), headers: kv(i.headers), enabled: i.enabled, stashed: false, extra: source?.extra ?? {},
    sig: source && sameCore(i, source) ? source.sig : `draft:${JSON.stringify([i.transport, i.command, i.args, i.cwd, i.url, i.env, i.headers])}`,
  };
}

/** An agent's servers with its pending MCP changes; `source` finds the server a copy came from. */
export function mcpView(servers: McpServer[], d: Draft, source: (from: [McpSource, string]) => McpServer | undefined): McpView[] {
  const ups = Object.values(d).flatMap((op) => (op.op === "upsert_mcp" ? [op.server] : []));
  const renamed = new Set(ups.flatMap((i) => (i.replaces && i.replaces !== i.name ? [i.replaces] : [])));
  const out: McpView[] = [];
  for (const s of servers) {
    if (renamed.has(s.name)) continue;
    const up = pendingUpsert(d, s.name);
    const tog = d[keys.mcpEnabled(s.name)];
    if (d[keys.mcpDelete(s.name)]) out.push({ s, pending: "deleted", exists: true });
    else if (up) out.push({ s: mcpFromInput(up, up.from ? source(up.from) ?? s : s), pending: "edited", exists: true });
    else if (tog?.op === "set_mcp_enabled") out.push({ s: { ...s, enabled: tog.enabled }, pending: "toggled", exists: true });
    else out.push({ s, pending: null, exists: true });
  }
  for (const i of ups) {
    if (servers.some((s) => s.name === i.name)) continue;
    const was = i.replaces ? servers.find((s) => s.name === i.replaces) : undefined;
    out.push({ s: mcpFromInput(i, i.from ? source(i.from) : was), pending: was ? "edited" : "new", exists: false });
  }
  return out;
}
