import type { AgentId, AgentState, GatewayStatus } from "../api";
import { type Draft, currentProvider, isEnabled, opCount, pendingTotal, settingOn, visibleCount } from "../draft";
import { tripped } from "../services";
import { AgentIcon, Icon } from "./icons";
import { type TKey, t, tn } from "../i18n";
import { scrub } from "../privacy";
import { SYNC_ENABLED } from "../features";
import { flushSync } from "react-dom";
import { dragSort, markReordered } from "../dragSort";
import { moveItem } from "../order";

export type Page = "providers" | "mcp" | "skills" | "gateway" | "wechat" | "history" | "sync" | "settings";

interface Props {
  agents: AgentState[];
  drafts: Record<string, Draft>;
  /** Selected agent when an agent page is open, else null. */
  selected: AgentId | null;
  page: Page | null;
  onSelect: (id: AgentId) => void;
  onPage: (p: Page) => void;
  gateway: GatewayStatus | null;
  /** The agents' new order, after one is dragged (or moved with Alt+↑/↓). */
  onReorder: (ids: AgentId[]) => void;
  /** Whether the MCP and skills links are shown (Settings → Interface). */
  showMcp: boolean;
  showSkills: boolean;
  /** Whether the Claw bridge link is shown (Settings → Interface). */
  showClaw: boolean;
}

function subline(a: AgentState, d: Draft): string {
  if (!a.installed) return t("sidebar.notInstalled");
  const n = visibleCount(a, d);
  if (a.mode === "single") return tn("sidebar.singleSub", n, { provider: currentProvider(a, d) ?? "-" });
  const on = a.providers.filter((p) => isEnabled(p, d)).length;
  return `${tn("common.providerCount", on)} · ${tn("common.modelCount", n)}`;
}

export function Sidebar({ agents, drafts, selected, page, onSelect, onPage, gateway, onReorder, showMcp, showSkills, showClaw }: Props) {
  const detected = agents.filter((a) => a.installed).length;
  const ids = agents.map((a) => a.id);
  const onKeyDown = (e: React.KeyboardEvent<HTMLButtonElement>, i: number) => {
    if (!e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
    const to = i + (e.key === "ArrowUp" ? -1 : 1);
    if (to < 0 || to >= ids.length) return;
    e.preventDefault();
    markReordered(e.currentTarget.parentElement!.querySelectorAll<HTMLElement>(":scope > .agent-row"));
    onReorder(moveItem(ids, i, to));
  };
  const pending = pendingTotal(drafts);
  const links: [Page, TKey, JSX.Element][] = [
    ["providers", "common.providers", <Icon.layers key="l" />],
    ...(showMcp ? [["mcp", "sidebar.mcp", <Icon.plug key="m" />] as [Page, TKey, JSX.Element]] : []),
    ...(showSkills ? [["skills", "sidebar.skills", <Icon.book key="s" />] as [Page, TKey, JSX.Element]] : []),
    ["gateway", "sidebar.gateway", <Icon.gateway key="g" />],
    ...(showClaw ? [["wechat", "sidebar.wechat", <Icon.chat key="w" />] as [Page, TKey, JSX.Element]] : []),
    ["history", "sidebar.history", <Icon.history key="h" />],
    ["sync", "sidebar.sync", <Icon.cloud key="c" />],
  ];
  return (
    <nav className="sidebar" aria-label={t("sidebar.nav")}>
      <div className="side-label">{t("sidebar.agents")}</div>
      {agents.map((a, i) => {
        const d = drafts[a.id] ?? {};
        const fast = a.id === "codex" && settingOn(a, "fast_inject");
        const dirty = opCount(d) > 0;
        return (
          <button key={a.id} className={`agent-row${a.id === selected ? " active" : ""}`} data-ctx="agent" data-agent={a.id} onClick={() => onSelect(a.id)}
            onPointerDown={(e) => dragSort(e.nativeEvent, e.currentTarget, ".agent-row", (from, to) => flushSync(() => onReorder(moveItem(ids, from, to))))}
            // An icon is an <img>: the browser's own image drag would start and cancel ours.
            onDragStart={(e) => e.preventDefault()}
            onKeyDown={(e) => onKeyDown(e, i)}>
            <AgentIcon id={a.id} size={32} />
            <span className="agent-row-text">
              <span className="agent-row-name">
                {a.name}
                {fast && <span className="badge-fast" title={t("sidebar.fastBadgeTitle")}>{t("sidebar.fastBadge")}</span>}
              </span>
              <span className="agent-row-sub">{subline(a, d)}</span>
            </span>
            {dirty && <span className="dot-dirty" title={t("sidebar.unapplied")} />}
          </button>
        );
      })}

      <div className="side-label spaced">{t("sidebar.resources")}</div>
      {links.map(([id, label, icon]) => {
        const off = id === "sync" && !SYNC_ENABLED;
        return (
          <button key={id} className={`side-link${page === id ? " active" : ""}`} disabled={off} title={off ? t("common.notAvailable") : undefined} onClick={() => onPage(id)}>{icon}{t(label)}</button>
        );
      })}

      {gateway?.enabled && (() => {
        const paused = gateway.routes.filter((r) => tripped(r));
        return (
          <button className={`gw-foot${gateway.running ? (paused.length ? " warn" : " on") : " bad"}`} onClick={() => onPage("gateway")}
            title={scrub(gateway.error) ?? (paused.length ? paused.map((r) => t("sidebar.pausedRoute", { name: r.name, reason: r.breaker!.reason ?? "" })).join("\n") : t("sidebar.openGateway"))}>
            <span className="gw-dot" />
            <span className="grow minw0">
              <span className="block strong">{gateway.running ? t("sidebar.gatewayOnline") : t("sidebar.gatewayDown")}</span>
              <span className="block ellipsis">
                {!gateway.running ? scrub(gateway.error) ?? t("sidebar.clickToView")
                  : paused.length ? tn("sidebar.tripped", paused.length)
                  : `127.0.0.1:${gateway.port} · ${tn("sidebar.requests", gateway.requests)}${gateway.active ? ` · ${t("sidebar.active", { n: gateway.active })}` : ""}`}
              </span>
            </span>
          </button>
        );
      })()}
      <div className={`side-foot${gateway?.enabled ? " tight" : ""}`}>
        <strong>{tn("sidebar.detected", detected)}</strong>
        <span>{pending ? tn("sidebar.pending", pending) : t("sidebar.backupsAt")}</span>
      </div>
    </nav>
  );
}
