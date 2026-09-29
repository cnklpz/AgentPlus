import { useMemo, useState } from "react";
import { type AgentId, type AgentState, type SkillCopy, type SkillKind, type SkillRoot, type SkillsOverview, api } from "../api";
import { type TKey, t, tn, useLang } from "../i18n";
import { AgentIcon, Icon } from "./icons";
import { ErrorBox, Seg } from "./controls";
import { useLoad } from "../hooks";
import { fmtSize } from "../format";
import { scrub } from "../privacy";
import { agentLabel } from "../services";
import { onActivateKey } from "../util";

/** One copy of a skill: the folder it sits in, and the skill. */
interface Copy {
  root: SkillRoot;
  s: SkillCopy;
}

/** How an agent sees one copy. */
type Seen = "active" | "disabled" | "shadowed";

interface Group {
  name: string;
  copies: Copy[];
  /** Agents that load the skill, and whether it's switched off there. */
  agents: { agent: AgentId; off: boolean }[];
  builtin: boolean;
  differs: boolean;
  problem: boolean;
}

const KIND: Record<SkillKind, TKey> = {
  own: "skillsPage.kindOwn",
  shared: "skillsPage.kindShared",
  compat: "skillsPage.kindCompat",
  extra: "skillsPage.kindExtra",
  builtin: "skillsPage.kindBuiltin",
};

/** The copy of `name` an agent loads: the first of its folders that has one. */
function loaded(o: SkillsOverview, agent: AgentId, name: string): SkillRoot | undefined {
  const a = o.agents.find((x) => x.agent === agent);
  for (const path of a?.roots ?? []) {
    const r = o.roots.find((x) => x.path === path);
    if (r?.skills.some((s) => s.name === name)) return r;
  }
  return undefined;
}

function seenBy(o: SkillsOverview, agent: AgentId, c: Copy): Seen {
  if (loaded(o, agent, c.s.name) !== c.root) return "shadowed";
  return o.agents.find((x) => x.agent === agent)?.disabled.some((d) => d === c.s.name || d === c.s.id) ? "disabled" : "active";
}

function groups(o: SkillsOverview): Group[] {
  const by = new Map<string, Copy[]>();
  for (const root of o.roots) for (const s of root.skills) by.set(s.name, [...(by.get(s.name) ?? []), { root, s }]);
  return [...by.entries()].sort(([a], [b]) => a.localeCompare(b)).map(([name, copies]) => {
    const readers = [...new Set(copies.flatMap((c) => c.root.readers))];
    const agents = readers.filter((a) => loaded(o, a, name)).map((agent) => ({
      agent,
      off: !!o.agents.find((x) => x.agent === agent)?.disabled.some((d) => d === name || copies.some((c) => c.s.id === d)),
    }));
    return {
      name,
      copies,
      agents,
      builtin: copies.every((c) => c.root.kind === "builtin"),
      differs: new Set(copies.map((c) => c.s.sig)).size > 1,
      problem: copies.some((c) => !!c.s.problem),
    };
  });
}

type Filter = "mine" | "builtin" | "all";
const FILTERS: [Filter, TKey][] = [["mine", "skillsPage.filterMine"], ["builtin", "skillsPage.filterBuiltin"], ["all", "skillsPage.filterAll"]];

export function SkillsPage({ agents }: { agents: AgentState[] }) {
  const lang = useLang();
  const ids = useMemo(() => agents.map((a) => a.id).filter((id) => !id.includes("@")), [agents]);
  // Problems come from the backend in the UI language; `agents` changes after every apply.
  const { data, error, reload } = useLoad(() => api.skillsList(ids), [lang, agents]);
  const [sel, setSel] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>("mine");
  const [q, setQ] = useState("");
  const all = useMemo(() => (data ? groups(data) : []), [data]);
  const query = q.trim().toLowerCase();
  const list = all.filter((g) => (filter === "all" || (filter === "builtin") === g.builtin)
    && (!query || g.name.toLowerCase().includes(query) || g.copies.some((c) => c.s.description.toLowerCase().includes(query))));
  const picked = all.find((g) => g.name === sel) ?? null;

  return (
    <>
      <main className="page">
        <div className="page-top">
          <div className="page-head">
            <div className="page-title">
              <h1>{t("skillsPage.title")}</h1>
              <span className="muted small hint">{t("skillsPage.subtitle")}</span>
            </div>
            <button className="btn" onClick={() => void reload()}>{t("common.refresh")}</button>
          </div>
          <div className="row gap6 skills-tools">
            <Seg value={filter} onChange={setFilter} label={t("skillsPage.filter")}
              options={FILTERS.map(([v, k]) => ({ value: v, label: `${t(k)} ${all.filter((g) => v === "all" || (v === "builtin") === g.builtin).length}` }))} />
            <input className="input grow" value={q} onChange={(e) => setQ(e.target.value)} placeholder={t("skillsPage.search")} aria-label={t("skillsPage.search")} />
          </div>
        </div>
        <div className="page-body">
          {error && (data ? <ErrorBox text={error} /> : <div className="empty">{scrub(error)}</div>)}
          {!data && !error && <div className="empty">{t("common.reading")}</div>}
          {data && list.length === 0 && <div className="empty">{t(query ? "skillsPage.noMatch" : filter === "builtin" ? "skillsPage.emptyBuiltin" : "skillsPage.empty")}</div>}
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
                        {g.builtin && <span className="ptag tag-soft">{t("skillsPage.kindBuiltin")}</span>}
                        {g.differs && <span className="ptag tag-warn" title={t("skillsPage.differsTitle")}>{t("skillsPage.differs")}</span>}
                        {g.problem && <span className="ptag tag-warn" title={t("skillsPage.problemTitle")}>{t("skillsPage.problem")}</span>}
                      </div>
                      <div className="tiny muted ellipsis">{g.copies[0].s.description || t("skillsPage.noDescription")}</div>
                    </div>
                    <div className="mcp-agents">
                      {g.agents.map(({ agent, off }) => (
                        <span key={agent} className={`mcp-agent${off ? " off" : ""}`} title={`${agentLabel(agent)} · ${t(off ? "skillsPage.off" : "skillsPage.on")}`}>
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
      <aside className="aside" aria-label={t("skillsPage.detailTitle")}>
        {picked && data ? <SkillDetail key={picked.name} g={picked} o={data} onClose={() => setSel(null)} /> : <Overview o={data} count={all.filter((g) => !g.builtin).length} />}
      </aside>
    </>
  );
}

function Overview({ o, count }: { o: SkillsOverview | null; count: number }) {
  return (
    <section className="aside-cur mcp-scroll skills-overview">
      <h2>{t("skillsPage.overview")}</h2>
      <span className="muted tiny hint">{t("skillsPage.overviewHint")}</span>
      {o && (
        <>
          <div className="hub-stats">
            <div><b>{count}</b><span>{t("skillsPage.statMine")}</span></div>
            <div><b>{o.roots.filter((r) => r.skills.length > 0).length}</b><span>{t("skillsPage.statFolders")}</span></div>
            <div><b>{o.agents.filter((a) => a.supported).length}</b><span>{t("skillsPage.statAgents")}</span></div>
          </div>
          <div className="kv">
            {o.roots.map((r) => (
              <div key={r.path} className={`kv-row${r.exists ? "" : " dim"}`}>
                <span className="tiny muted">{t(KIND[r.kind])}</span>
                <span className="minw0">
                  <span className="mono tiny block ellipsis" title={scrub(r.path)}>{scrub(r.path)}</span>
                  <span className="row gap6 tiny muted">
                    {r.exists ? tn("skillsPage.skillCount", r.skills.length) : t("skillsPage.noFolder")}
                    <span className="skills-readers">{r.readers.map((a) => <AgentIcon key={a} id={a} size={14} />)}</span>
                  </span>
                </span>
              </div>
            ))}
          </div>
        </>
      )}
    </section>
  );
}

function SkillDetail({ g, o, onClose }: { g: Group; o: SkillsOverview; onClose: () => void }) {
  return (
    <section className="aside-cur mcp-scroll">
      <div className="row between">
        <div className="minw0">
          <h2 className="ellipsis">{g.name}</h2>
          <span className="tiny muted">{tn("skillsPage.inAgents", g.agents.length)}</span>
        </div>
        <button className="icon-btn" aria-label={t("common.closeDetails")} onClick={onClose}><Icon.close /></button>
      </div>
      {g.differs && <span className="tiny warn-text">{t("skillsPage.differsHint")}</span>}
      {g.copies.map((c) => (
        <div key={c.s.dir} className="mcp-variant">
          <div className="row gap6">
            <span className="ptag tag-soft">{t(KIND[c.root.kind])}</span>
            {c.root.owner && c.root.kind === "own" && <span className="tiny muted">{agentLabel(c.root.owner)}</span>}
          </div>
          {c.s.description && <span className="small skills-desc">{c.s.description}</span>}
          {c.s.problem && <span className="tiny warn-text">{c.s.problem}</span>}
          <div className="kv">
            <div className="kv-row"><span className="tiny muted">{t("skillsPage.folder")}</span><span className="mono tiny mcp-wrap">{scrub(c.s.dir)}</span></div>
            <div className="kv-row"><span className="tiny muted">{t("skillsPage.content")}</span><span className="tiny">{tn("skillsPage.fileCount", c.s.files, { size: fmtSize(c.s.bytes) })}</span></div>
          </div>
          <div className="mcp-holders">
            {c.root.readers.map((a) => {
              const seen = seenBy(o, a, c);
              const winner = seen === "shadowed" ? loaded(o, a, c.s.name) : undefined;
              return (
                <div key={a} className={`mcp-holder${seen === "active" ? "" : " off"}`}>
                  <AgentIcon id={a} size={16} />
                  <span className="tiny grow minw0 ellipsis">
                    {agentLabel(a)} <span className="muted">· {seen === "shadowed" ? t("skillsPage.shadowed", { path: scrub(winner?.path ?? "") }) : t(seen === "disabled" ? "skillsPage.off" : "skillsPage.on")}</span>
                  </span>
                </div>
              );
            })}
          </div>
        </div>
      ))}
    </section>
  );
}
