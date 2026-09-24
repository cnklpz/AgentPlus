// Pending edits per agent, keyed so a toggle back to the original drops the op.
import type { AgentState, Model, ModelFieldValue, ModelInput, Op, Provider, ProviderInput, Setting, SettingValue } from "./api";
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
};

/** Codex has one global catalog; its models use provider "*". */
export const CATALOG = "*";

export function withOp(d: Draft, key: string, op: Op | null): Draft {
  const next = { ...d };
  if (op) next[key] = op;
  else delete next[key];
  return next;
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

function sameValue(a: SettingValue, b: SettingValue): boolean {
  if (Array.isArray(a) && Array.isArray(b)) return [...a].sort().join("|") === [...b].sort().join("|");
  return a === b;
}

export function setSetting(d: Draft, s: Setting, value: SettingValue): Draft {
  return withOp(d, keys.setting(s.key), sameValue(value, s.value) ? null : { op: "set_setting", key: s.key, value });
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

const API_LABEL: Record<string, string> = { responses: "Responses", chat: "Chat", anthropic: "Anthropic" };

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
        id: k, draftKey: k, name: op.name ?? op.provider, baseUrl: null, host: t("draft.copiedFrom", { from: op.label ?? op.fromAgent }), apis: [API_LABEL[api] ?? api],
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

/** 131072 -> "131K", 1048576 -> "1M" */
export function fmtCtx(n: number): string {
  if (n >= 1_000_000) {
    const m = n % 1_048_576 === 0 ? n / 1_048_576 : n / 1_000_000;
    return `${Number.isInteger(m) ? m : m.toFixed(1)}M`;
  }
  if (n >= 1000) return `${Math.round(n / 1000)}K`;
  return String(n);
}

/** "128k" -> 128000, "1m" -> 1000000, "200000" -> 200000 */
export function parseCtx(s: string): number | null {
  const t = s.trim().toLowerCase().replace(/,/g, "");
  if (!t) return null;
  const m = t.match(/^(\d+(?:\.\d+)?)\s*([km]?)$/);
  if (!m) return null;
  const n = parseFloat(m[1]) * (m[2] === "k" ? 1000 : m[2] === "m" ? 1_000_000 : 1);
  return Math.round(n);
}

export function visibleCount(st: AgentState, d: Draft): number {
  if (st.catalog) return viewModels(CATALOG, st.catalog, d).filter((m) => !m.isDeleted && isVisible(CATALOG, m, d)).length;
  return viewProviders(st, d)
    .filter((p) => !p.isDeleted && (p.isNew || isEnabled(p, d)))
    .reduce((n, p) => n + (p.isNew ? p.models.length : viewModels(p.id, p.models, d).filter((m) => !m.isDeleted && isVisible(p.id, m, d)).length), 0);
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
