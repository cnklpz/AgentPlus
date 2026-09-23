// Pending edits per agent, keyed so a toggle back to the original drops the op.
import type { AgentState, Op, Provider, Model, Setting } from "./api";

export type Draft = Record<string, Op>;

export const keys = {
  cur: () => "cur",
  enabled: (pid: string) => `pe:${pid}`,
  visible: (pid: string, mid: string) => `mv:${pid}|${mid}`,
  setting: (key: string) => `set:${key}`,
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

export function settingValue(s: Setting, d: Draft): boolean | string[] {
  const op = d[keys.setting(s.key)];
  return op && op.op === "set_setting" ? op.value : s.value;
}

function sameValue(a: boolean | string[], b: boolean | string[]): boolean {
  if (Array.isArray(a) && Array.isArray(b)) return [...a].sort().join("|") === [...b].sort().join("|");
  return a === b;
}

export function setSetting(d: Draft, s: Setting, value: boolean | string[]): Draft {
  return withOp(d, keys.setting(s.key), sameValue(value, s.value) ? null : { op: "set_setting", key: s.key, value });
}

export function visibleCount(st: AgentState, d: Draft): number {
  if (st.catalog) return st.catalog.filter((m) => isVisible(CATALOG, m, d)).length;
  return st.providers
    .filter((p) => isEnabled(p, d))
    .reduce((n, p) => n + p.models.filter((m) => isVisible(p.id, m, d)).length, 0);
}
