import { useMemo, useState } from "react";
import { type AgentId, type AgentMcp, type AgentState, type McpKv, type McpServer, type McpTransport, api } from "../api";
import { t, tn, useLang } from "../i18n";
import { AgentIcon, Icon } from "./icons";
import { ErrorBox } from "./controls";
import { useLoad } from "../hooks";
import { scrub } from "../privacy";
import { agentLabel } from "../services";
import { onActivateKey } from "../util";

/** Protocol names: not translated. */
const TRANSPORT: Record<McpTransport, string> = { stdio: "stdio", http: "HTTP", sse: "SSE", ws: "WebSocket", remote: "HTTP / SSE" };

/** One agent's copy of a server. */
interface Entry {
  agent: AgentId;
  s: McpServer;
}

/** A server name across agents; `variants` groups the copies by what they run. */
interface Group {
  name: string;
  entries: Entry[];
  variants: Entry[][];
}

function groups(list: AgentMcp[]): Group[] {
  const by = new Map<string, Entry[]>();
  for (const a of list) for (const s of a.servers) by.set(s.name, [...(by.get(s.name) ?? []), { agent: a.agent, s }]);
  return [...by.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([name, entries]) => {
      const v = new Map<string, Entry[]>();
      for (const e of entries) v.set(e.s.sig, [...(v.get(e.s.sig) ?? []), e]);
      return { name, entries, variants: [...v.values()] };
    });
}

const quote = (a: string) => (/[\s"]/.test(a) ? `"${a.replace(/"/g, '\\"')}"` : a);

/** What runs: the command line, or the URL. */
function summary(s: McpServer): string {
  return s.transport === "stdio" ? [s.command ?? "", ...s.args].map(quote).join(" ") : s.url ?? "";
}

function stateText(s: McpServer): string {
  return t(s.enabled ? "mcpPage.on" : s.stashed ? "mcpPage.stashed" : "mcpPage.off");
}

export function McpPage({ agents }: { agents: AgentState[] }) {
  const lang = useLang();
  const ids = useMemo(() => agents.map((a) => a.id).filter((id) => !id.includes("@")), [agents]);
  // Errors come from the backend in the UI language: reload on switch.
  const { data, error, reload } = useLoad(() => api.mcpList(ids), [lang, ids.join(",")]);
  const [sel, setSel] = useState<string | null>(null);
  const list = useMemo(() => (data ? groups(data) : []), [data]);
  const picked = list.find((g) => g.name === sel) ?? null;
  const differ = list.filter((g) => g.variants.length > 1).length;

  return (
    <>
      <main className="page">
        <div className="page-top">
          <div className="page-head">
            <div className="page-title">
              <h1>{t("mcpPage.title")}</h1>
              <span className="muted small hint">{t("mcpPage.subtitle")}</span>
            </div>
            <button className="btn" onClick={() => void reload()}>{t("common.refresh")}</button>
          </div>
        </div>
        <div className="page-body">
          {error && (data ? <ErrorBox text={error} /> : <div className="empty">{scrub(error)}</div>)}
          {!data && !error && <div className="empty">{t("common.reading")}</div>}
          {data?.filter((a) => a.error).map((a) => <ErrorBox key={a.agent} text={t("mcpPage.agentError", { agent: agentLabel(a.agent), error: a.error! })} />)}
          {data && list.length === 0 && <div className="empty">{t("mcpPage.empty")}</div>}
          {list.length > 0 && (
            <div className="stable">
              {list.map((g) => {
                const on = sel === g.name;
                const toggle = () => setSel(on ? null : g.name);
                return (
                  <div key={g.name} className={`hrow pick${on ? " on" : ""}`} role="button" tabIndex={0} aria-pressed={on} onClick={toggle} onKeyDown={onActivateKey(toggle)}>
                    <div className="minw0">
                      <div className="row gap6">
                        <span className="strong small">{g.name}</span>
                        <span className="ptag tag-soft">{TRANSPORT[g.entries[0].s.transport]}</span>
                        {g.variants.length > 1 && <span className="ptag tag-warn" title={t("mcpPage.differsTitle")}>{t("mcpPage.differs")}</span>}
                      </div>
                      <div className="mono tiny muted ellipsis">{scrub(summary(g.entries[0].s))}</div>
                    </div>
                    <div className="mcp-agents">
                      {g.entries.map(({ agent, s }) => (
                        <span key={agent} className={`mcp-agent${s.enabled ? "" : " off"}`} title={`${agentLabel(agent)} · ${stateText(s)}`}>
                          <AgentIcon id={agent} size={20} />
                        </span>
                      ))}
                    </div>
                  </div>
                );
              })}
            </div>
          )}
        </div>
      </main>
      <aside className="aside" aria-label={t("mcpPage.detailTitle")}>
        {picked ? (
          <McpDetail key={picked.name} g={picked} onClose={() => setSel(null)} />
        ) : (
          <section className="aside-cur mcp-scroll">
            <h2>{t("mcpPage.overview")}</h2>
            <span className="muted tiny hint">{t("mcpPage.overviewHint")}</span>
            {data && (
              <>
                <div className="hub-stats">
                  <div><b>{list.length}</b><span>{t("mcpPage.statServers")}</span></div>
                  <div><b>{data.filter((a) => a.servers.length > 0).length}</b><span>{t("mcpPage.statAgents")}</span></div>
                  <div><b>{differ}</b><span>{t("mcpPage.statDiffer")}</span></div>
                </div>
                <div className="kv">
                  {data.map((a) => (
                    <div key={a.agent} className="kv-row">
                      <span className="tiny row gap6"><AgentIcon id={a.agent} size={16} />{agentLabel(a.agent)}</span>
                      <span className="minw0">
                        <span className="tiny block">
                          {!a.supported ? t("mcpPage.unsupported") : a.error ? t("mcpPage.readFailed") : !a.exists ? t("mcpPage.noFile") : tn("mcpPage.serverCount", a.servers.length)}
                        </span>
                        {a.file && <span className="mono tiny muted block ellipsis" title={scrub(a.file)}>{scrub(a.file)}</span>}
                      </span>
                    </div>
                  ))}
                </div>
              </>
            )}
          </section>
        )}
      </aside>
    </>
  );
}

function Pairs({ items }: { items: McpKv[] }) {
  return (
    <span className="minw0">
      {items.map((p) => (
        <span key={p.key} className="mono tiny block mcp-pair" title={p.secret ? t("mcpPage.masked") : undefined}>
          {p.key}=<span className={p.secret ? "muted" : undefined}>{scrub(p.value)}</span>
          {p.secret && <Icon.key size={11} />}
        </span>
      ))}
    </span>
  );
}

function McpDetail({ g, onClose }: { g: Group; onClose: () => void }) {
  return (
    <section className="aside-cur mcp-scroll">
      <div className="row between">
        <div className="minw0">
          <h2 className="ellipsis">{g.name}</h2>
          <span className="tiny muted">{tn("mcpPage.inAgents", g.entries.length)}</span>
        </div>
        <button className="icon-btn" aria-label={t("common.closeDetails")} onClick={onClose}><Icon.close /></button>
      </div>
      {g.variants.length > 1 && <span className="tiny warn-text">{t("mcpPage.differsHint")}</span>}
      {g.variants.map((v, i) => {
        const s = v[0].s;
        const extras = v.filter((e) => Object.keys(e.s.extra).length > 0);
        return (
          <div key={s.sig} className="mcp-variant">
            {g.variants.length > 1 && <span className="tiny strong">{t("mcpPage.variant", { i: i + 1, n: g.variants.length })}</span>}
            <div className="mcp-states">
              {v.map(({ agent, s: x }) => (
                <span key={agent} className={`mcp-state${x.enabled ? "" : " off"}`}>
                  <AgentIcon id={agent} size={16} />
                  <span className="tiny">{agentLabel(agent)}</span>
                  <span className="tiny muted">{stateText(x)}</span>
                </span>
              ))}
            </div>
            <div className="kv">
              <div className="kv-row"><span className="tiny muted">{t("mcpPage.transport")}</span><span className="tiny">{TRANSPORT[s.transport]}</span></div>
              {s.transport === "stdio" ? (
                <div className="kv-row"><span className="tiny muted">{t("mcpPage.command")}</span><span className="mono tiny mcp-wrap">{scrub(summary(s))}</span></div>
              ) : (
                <div className="kv-row"><span className="tiny muted">{t("mcpPage.url")}</span><span className="mono tiny mcp-wrap">{scrub(s.url ?? "")}</span></div>
              )}
              {s.cwd && <div className="kv-row"><span className="tiny muted">{t("mcpPage.cwd")}</span><span className="mono tiny mcp-wrap">{scrub(s.cwd)}</span></div>}
              {s.env.length > 0 && <div className="kv-row"><span className="tiny muted">{t("mcpPage.env")}</span><Pairs items={s.env} /></div>}
              {s.headers.length > 0 && <div className="kv-row"><span className="tiny muted">{t("mcpPage.headers")}</span><Pairs items={s.headers} /></div>}
              {extras.map(({ agent, s: x }) => (
                <div key={agent} className="kv-row">
                  <span className="tiny muted">{t("mcpPage.otherFields", { agent: agentLabel(agent) })}</span>
                  <span className="minw0">
                    {Object.entries(x.extra).map(([k, val]) => (
                      <span key={k} className="mono tiny block mcp-pair">{k}={scrub(typeof val === "string" ? val : JSON.stringify(val))}</span>
                    ))}
                  </span>
                </div>
              ))}
            </div>
          </div>
        );
      })}
    </section>
  );
}
