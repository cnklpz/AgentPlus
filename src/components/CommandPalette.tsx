import { useEffect, useMemo, useRef, useState } from "react";
import { type AgentId, type AgentState, type SessionRow, api } from "../api";
import type { Tab } from "./AgentPage";
import type { SettingsTab } from "./SettingsPage";
import type { Page } from "./Sidebar";
import { AgentIcon, Icon } from "./icons";
import { type TKey, t, useLang } from "../i18n";
import { useEscape, useListNav } from "../hooks";
import { scrubHost, usePrivacy } from "../privacy";
import { SYNC_ENABLED } from "../features";

export type Target =
  | { kind: "agent"; agent: AgentId; tab?: Tab; provider?: string; setting?: string; query?: string }
  | { kind: "page"; page: Page; settingsTab?: SettingsTab };

/** Stable group ids (display labels come from GROUP_LABEL); array order = display order. */
const GROUPS = ["agent", "page", "provider", "setting", "model", "session"] as const;
type Group = (typeof GROUPS)[number];
const GROUP_LABEL: Record<Group, TKey> = {
  agent: "commandPalette.groupAgent",
  page: "commandPalette.groupPage",
  provider: "common.provider",
  setting: "commandPalette.groupSetting",
  model: "common.model",
  session: "commandPalette.groupSession",
};

interface Item {
  label: string;
  hint: string;
  group: Group;
  agent?: AgentId;
  /** Icon for items that do not belong to one agent (pages). */
  icon?: JSX.Element;
  target: Target;
  haystack: string;
  /** Shown greyed out; picking it does nothing. */
  disabled?: boolean;
}

interface Props {
  agents: AgentState[];
  onGo: (t: Target) => void;
  onClose: () => void;
}

/** Ctrl+K: search agents, providers, models, settings, pages and Codex sessions. */
export function CommandPalette({ agents, onGo, onClose }: Props) {
  const [q, setQ] = useState("");
  const [sessions, setSessions] = useState<SessionRow[]>([]);
  const input = useRef<HTMLInputElement>(null);
  const lang = useLang();
  const privacy = usePrivacy();

  useEffect(() => {
    input.current?.focus();
    if (agents.some((a) => a.id === "codex" && a.installed)) api.codexSessions().then((s) => setSessions(s.sessions)).catch(() => undefined);
  }, [agents]);

  const items = useMemo(() => {
    // Haystacks are search terms only (never shown): they keep both Chinese and English
    // words so either language finds the page; the translated label is matched too.
    const out: Item[] = [
      { label: t("common.providers"), hint: t("commandPalette.providersHint"), group: "page", icon: <Icon.layers size={14} />, target: { kind: "page", page: "providers" }, haystack: "服务商 供应商 总供应商 模型 providers 添加供应商 add provider library" },
      { label: t("commandPalette.projects"), hint: t("commandPalette.projectsHint"), group: "page", icon: <Icon.folder size={14} />, target: { kind: "agent", agent: "opencode", tab: "projects" }, haystack: "项目 project 文件夹 folder opencode.json 项目级 工作区 workspace" },
      { label: t("commandPalette.gateway"), hint: t("commandPalette.gatewayHint"), group: "page", icon: <Icon.gateway size={14} />, target: { kind: "page", page: "gateway" }, haystack: "网关 gateway 转换 协议 代理 proxy 中转 relay protocol convert" },
      { label: t("commandPalette.history"), hint: t("commandPalette.historyHint"), group: "page", icon: <Icon.history size={14} />, target: { kind: "page", page: "history" }, haystack: "历史 回滚 备份 history backup rollback restore" },
      { label: t("commandPalette.sync"), hint: SYNC_ENABLED ? t("commandPalette.syncHint") : t("common.notAvailable"), group: "page", icon: <Icon.cloud size={14} />, target: { kind: "page", page: "sync" }, haystack: "同步 导出 导入 sync export import device", disabled: !SYNC_ENABLED },
      { label: t("commandPalette.settings"), hint: t("commandPalette.settingsHint"), group: "page", icon: <Icon.gear size={14} />, target: { kind: "page", page: "settings" }, haystack: "设置 settings 环境 wsl windows 动画 测速 environment animation latency language 语言" },
      { label: t("commandPalette.detect"), hint: t("commandPalette.detectHint"), group: "page", icon: <Icon.search size={13} />, target: { kind: "page", page: "settings", settingsTab: "agents" }, haystack: "识别 检测 配置目录 目录 detect detection config folder" },
    ];
    for (const a of agents) {
      out.push({ label: a.name, hint: "Agent", group: "agent", agent: a.id, target: { kind: "agent", agent: a.id }, haystack: a.name });
      const tabs: [Tab, TKey][] = [["prov", "common.providers"], ["models", "commandPalette.tabModels"], ["set", "commandPalette.tabSettings"]];
      if (a.id === "codex") tabs.push(["sessions", "commandPalette.tabSessions"], ["maint", "commandPalette.tabMaint"]);
      for (const [tab, k] of tabs) {
        const l = t(k);
        out.push({ label: `${a.name} · ${l}`, hint: t("commandPalette.groupPage"), group: "page", agent: a.id, target: { kind: "agent", agent: a.id, tab }, haystack: `${a.name} ${l}` });
      }
      for (const p of a.providers) {
        out.push({ label: p.name, hint: `${a.name} · ${scrubHost(p.host)}`, group: "provider", agent: a.id, target: { kind: "agent", agent: a.id, tab: "prov", provider: p.id }, haystack: `${p.name} ${p.host} ${p.id}` });
        for (const m of p.models) {
          out.push({ label: m.id, hint: `${a.name} · ${p.name}`, group: "model", agent: a.id, target: { kind: "agent", agent: a.id, tab: "models", provider: p.id }, haystack: `${m.id} ${m.name ?? ""}` });
        }
      }
      for (const m of a.catalog ?? []) {
        out.push({ label: m.id, hint: t("commandPalette.catalogHint", { agent: a.name }), group: "model", agent: a.id, target: { kind: "agent", agent: a.id, tab: "models" }, haystack: `${m.id} ${m.name ?? ""}` });
      }
      for (const s of a.settings) {
        out.push({ label: s.label, hint: `${a.name} · ${s.group}`, group: "setting", agent: a.id, target: { kind: "agent", agent: a.id, tab: "set", setting: s.key }, haystack: `${s.label} ${s.desc} ${s.key}` });
      }
    }
    for (const s of sessions) {
      out.push({ label: s.title, hint: t("commandPalette.sessionHint", { provider: s.provider, cwd: s.cwd }), group: "session", agent: "codex", target: { kind: "agent", agent: "codex", tab: "sessions", query: s.id }, haystack: `${s.title} ${s.cwd} ${s.id}` });
    }
    return out;
  }, [agents, sessions, lang, privacy]);

  const results = useMemo(() => {
    const words = q.trim().toLowerCase().split(/\s+/).filter(Boolean);
    const hits = words.length === 0
      ? items.filter((i) => i.group === "agent" || i.group === "page").slice(0, 14)
      : items.filter((i) => words.every((w) => i.haystack.toLowerCase().includes(w) || i.label.toLowerCase().includes(w)));
    // Group order, and keep models and sessions from drowning out the rest.
    const cap: Partial<Record<Group, number>> = { model: 12, session: 10 };
    const seen: Partial<Record<Group, number>> = {};
    return hits
      .filter((i) => ((seen[i.group] = (seen[i.group] ?? 0) + 1) <= (cap[i.group] ?? 30)))
      .sort((a, b) => GROUPS.indexOf(a.group) - GROUPS.indexOf(b.group))
      .slice(0, 60);
  }, [items, q]);

  const nav = useListNav(results.length, { selector: ".palette-item.on" });
  useEffect(() => nav.setHi(0), [q]);

  const go = (i: Item | undefined) => { if (i && !i.disabled) { onGo(i.target); onClose(); } };
  // Esc closes only the palette, not a dialog it was opened over.
  useEscape(onClose);

  return (
    <div className="modal-bg top" onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}>
      <div className="palette" role="dialog" aria-modal="true" aria-label={t("common.search")}>
        <input
          ref={input}
          className="palette-input"
          value={q}
          placeholder={t("commandPalette.placeholder")}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (!nav.onKey(e) && e.key === "Enter") go(results[nav.hi]);
          }}
        />
        <div className="palette-list" ref={nav.list}>
          {results.length === 0 && <div className="muted small palette-empty">{t("commandPalette.noResults", { q })}</div>}
          {results.map((r, i) => (
            <button key={`${r.group}-${r.label}-${r.hint}-${i}`} className={`palette-item${i === nav.hi ? " on" : ""}${r.disabled ? " off" : ""}`} aria-disabled={r.disabled}
              onMouseEnter={() => nav.setHi(i)} onClick={() => go(r)}>
              {r.agent ? <AgentIcon id={r.agent} size={18} /> : <span className="palette-dot">{r.icon}</span>}
              <span className="grow minw0">
                <span className={`ellipsis block small strong${r.group === "session" ? " sensitive" : ""}`}>{r.label}</span>
                <span className={`ellipsis block tiny muted${r.group === "session" ? " sensitive" : ""}`}>{r.hint}</span>
              </span>
              <span className="palette-group tiny">{t(GROUP_LABEL[r.group])}</span>
            </button>
          ))}
        </div>
        <div className="palette-foot tiny muted hint">{t("commandPalette.foot")}</div>
      </div>
    </div>
  );
}
