import type { AgentId, AgentState, GatewayStatus } from "../api";
import { type Draft, currentProvider, isEnabled, visibleCount } from "../draft";
import { AgentIcon, Icon } from "./icons";
import { type TKey, t, tn } from "../i18n";

export type Page = "providers" | "gateway" | "history" | "sync" | "settings";

interface Props {
  agents: AgentState[];
  drafts: Record<string, Draft>;
  /** Selected agent when an agent page is open, else null. */
  selected: AgentId | null;
  page: Page | null;
  onSelect: (id: AgentId) => void;
  onPage: (p: Page) => void;
  gateway: GatewayStatus | null;
}

function subline(a: AgentState, d: Draft): string {
  if (!a.installed) return t("sidebar.notInstalled");
  const n = visibleCount(a, d);
  if (a.mode === "single") return tn("sidebar.singleSub", n, { provider: currentProvider(a, d) ?? "-" });
  const on = a.providers.filter((p) => isEnabled(p, d)).length;
  return `${tn("sidebar.providerCount", on)} · ${tn("sidebar.modelCount", n)}`;
}

export function Sidebar({ agents, drafts, selected, page, onSelect, onPage, gateway }: Props) {
  const detected = agents.filter((a) => a.installed).length;
  const pending = Object.values(drafts).reduce((n, d) => n + Object.keys(d).length, 0);
  const links: [Page, TKey, JSX.Element][] = [
    ["providers", "sidebar.providers", <Icon.layers key="l" />],
    ["gateway", "sidebar.gateway", <Icon.gateway key="g" />],
    ["history", "sidebar.history", <Icon.history key="h" />],
    ["sync", "sidebar.sync", <Icon.cloud key="c" />],
  ];
  return (
    <nav className="sidebar" aria-label={t("sidebar.nav")}>
      <div className="side-label">AGENT</div>
      {agents.map((a) => {
        const d = drafts[a.id] ?? {};
        const fast = a.id === "codex" && a.settings.find((s) => s.key === "fast_inject")?.value === true;
        const dirty = Object.keys(d).length > 0;
        return (
          <button key={a.id} className={`agent-row${a.id === selected ? " active" : ""}`} data-ctx="agent" data-agent={a.id} onClick={() => onSelect(a.id)}>
            <AgentIcon id={a.id} size={32} />
            <span className="agent-row-text">
              <span className="agent-row-name">
                {a.name}
                {fast && <span className="badge-fast">FAST</span>}
              </span>
              <span className="agent-row-sub">{subline(a, d)}</span>
            </span>
            {dirty && <span className="dot-dirty" title={t("sidebar.unapplied")} />}
          </button>
        );
      })}

      <div className="side-label spaced">{t("sidebar.resources")}</div>
      {links.map(([id, label, icon]) => (
        <button key={id} className={`side-link${page === id ? " active" : ""}`} onClick={() => onPage(id)}>{icon}{t(label)}</button>
      ))}

      {gateway?.enabled && (() => {
        const paused = gateway.routes.filter((r) => r.breaker && r.breaker.state !== "closed");
        return (
          <button className={`gw-foot${gateway.running ? (paused.length ? " warn" : " on") : " bad"}`} onClick={() => onPage("gateway")}
            title={gateway.error ?? (paused.length ? paused.map((r) => t("sidebar.pausedRoute", { name: r.name, reason: r.breaker!.reason ?? "" })).join("\n") : t("sidebar.openGateway"))}>
            <span className="gw-dot" />
            <span className="grow minw0">
              <span className="block strong">{gateway.running ? t("sidebar.gatewayOnline") : t("sidebar.gatewayDown")}</span>
              <span className="block ellipsis">
                {!gateway.running ? gateway.error ?? t("sidebar.clickToView")
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
