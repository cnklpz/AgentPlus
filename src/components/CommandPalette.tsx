import { useEffect, useMemo, useRef, useState } from "react";
import { type AgentId, type AgentState, type SessionRow, api } from "../api";
import type { Tab } from "./AgentPage";
import { AgentIcon } from "./icons";

export type Target =
  | { kind: "agent"; agent: AgentId; tab?: Tab; provider?: string; setting?: string; query?: string }
  | { kind: "page"; page: "providers" | "history" | "sync" };

interface Item {
  label: string;
  hint: string;
  group: string;
  agent?: AgentId;
  target: Target;
  haystack: string;
}

interface Props {
  agents: AgentState[];
  onGo: (t: Target) => void;
  onClose: () => void;
}

/** Ctrl+K: search agents, providers, models, settings, pages and Codex sessions. */
export function CommandPalette({ agents, onGo, onClose }: Props) {
  const [q, setQ] = useState("");
  const [sel, setSel] = useState(0);
  const [sessions, setSessions] = useState<SessionRow[]>([]);
  const input = useRef<HTMLInputElement>(null);

  useEffect(() => {
    input.current?.focus();
    if (agents.some((a) => a.id === "codex" && a.installed)) api.codexSessions().then((s) => setSessions(s.sessions)).catch(() => undefined);
  }, [agents]);

  const items = useMemo(() => {
    const out: Item[] = [
      { label: "供应商", hint: "所有 Agent 共用的供应商库：添加、编辑、同步到各 Agent", group: "页面", target: { kind: "page", page: "providers" }, haystack: "服务商 供应商 总供应商 模型 providers 添加供应商" },
      { label: "历史与回滚", hint: "备份和回滚", group: "页面", target: { kind: "page", page: "history" }, haystack: "历史 回滚 备份 history backup" },
      { label: "多设备同步", hint: "导出 / 导入", group: "页面", target: { kind: "page", page: "sync" }, haystack: "同步 导出 导入 sync" },
    ];
    for (const a of agents) {
      out.push({ label: a.name, hint: "Agent", group: "Agent", agent: a.id, target: { kind: "agent", agent: a.id }, haystack: a.name });
      const tabs: [Tab, string][] = [["prov", "供应商"], ["models", "模型列表"], ["set", "其他设置"]];
      if (a.id === "codex") tabs.push(["sessions", "会话"], ["maint", "维护"]);
      for (const [t, l] of tabs) out.push({ label: `${a.name} · ${l}`, hint: "页面", group: "页面", agent: a.id, target: { kind: "agent", agent: a.id, tab: t }, haystack: `${a.name} ${l}` });
      for (const p of a.providers) {
        out.push({ label: p.name, hint: `${a.name} · ${p.host}`, group: "供应商", agent: a.id, target: { kind: "agent", agent: a.id, tab: "prov", provider: p.id }, haystack: `${p.name} ${p.host} ${p.id}` });
        for (const m of p.models) {
          out.push({ label: m.id, hint: `${a.name} · ${p.name}`, group: "模型", agent: a.id, target: { kind: "agent", agent: a.id, tab: "models", provider: p.id }, haystack: `${m.id} ${m.name ?? ""}` });
        }
      }
      for (const m of a.catalog ?? []) {
        out.push({ label: m.id, hint: `${a.name} · 模型目录`, group: "模型", agent: a.id, target: { kind: "agent", agent: a.id, tab: "models" }, haystack: `${m.id} ${m.name ?? ""}` });
      }
      for (const s of a.settings) {
        out.push({ label: s.label, hint: `${a.name} · ${s.group}`, group: "设置", agent: a.id, target: { kind: "agent", agent: a.id, tab: "set", setting: s.key }, haystack: `${s.label} ${s.desc} ${s.key}` });
      }
    }
    for (const s of sessions) {
      out.push({ label: s.title, hint: `Codex 会话 · ${s.provider} · ${s.cwd}`, group: "会话", agent: "codex", target: { kind: "agent", agent: "codex", tab: "sessions", query: s.id }, haystack: `${s.title} ${s.cwd} ${s.id}` });
    }
    return out;
  }, [agents, sessions]);

  const results = useMemo(() => {
    const words = q.trim().toLowerCase().split(/\s+/).filter(Boolean);
    const hits = words.length === 0
      ? items.filter((i) => i.group === "Agent" || i.group === "页面").slice(0, 14)
      : items.filter((i) => words.every((w) => i.haystack.toLowerCase().includes(w) || i.label.toLowerCase().includes(w)));
    // Group order, and keep models and sessions from drowning out the rest.
    const order = ["Agent", "页面", "供应商", "设置", "模型", "会话"];
    const cap: Record<string, number> = { 模型: 12, 会话: 10 };
    const seen: Record<string, number> = {};
    return hits
      .filter((i) => ((seen[i.group] = (seen[i.group] ?? 0) + 1) <= (cap[i.group] ?? 30)))
      .sort((a, b) => order.indexOf(a.group) - order.indexOf(b.group))
      .slice(0, 60);
  }, [items, q]);

  useEffect(() => setSel(0), [q]);

  const go = (i: Item | undefined) => { if (i) { onGo(i.target); onClose(); } };

  return (
    <div className="modal-bg top" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="palette" role="dialog" aria-modal="true" aria-label="搜索">
        <input
          ref={input}
          className="palette-input"
          value={q}
          placeholder="搜索 Agent、供应商、模型、设置、会话…"
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") onClose();
            else if (e.key === "ArrowDown") { e.preventDefault(); setSel((s) => Math.min(s + 1, results.length - 1)); }
            else if (e.key === "ArrowUp") { e.preventDefault(); setSel((s) => Math.max(s - 1, 0)); }
            else if (e.key === "Enter") go(results[sel]);
          }}
        />
        <div className="palette-list">
          {results.length === 0 && <div className="muted small palette-empty">没有找到「{q}」</div>}
          {results.map((r, i) => (
            <button key={`${r.group}-${r.label}-${r.hint}-${i}`} className={`palette-item${i === sel ? " on" : ""}`}
              onMouseEnter={() => setSel(i)} onClick={() => go(r)}>
              {r.agent ? <AgentIcon id={r.agent} size={18} /> : <span className="palette-dot" />}
              <span className="grow minw0">
                <span className="ellipsis block small strong">{r.label}</span>
                <span className="ellipsis block tiny muted">{r.hint}</span>
              </span>
              <span className="palette-group tiny">{r.group}</span>
            </button>
          ))}
        </div>
        <div className="palette-foot tiny muted">↑↓ 选择 · Enter 打开 · Esc 关闭</div>
      </div>
    </div>
  );
}
