import { useMemo, useState } from "react";
import { type AgentId, type AgentState, type SkillCopy, type SkillImported, type SkillKind, type SkillRoot, type SkillsOverview, api } from "../api";
import { type TKey, t, tn, useLang } from "../i18n";
import { AgentIcon, Icon } from "./icons";
import { ErrorBox, Seg, Switch } from "./controls";
import { ask } from "./Confirm";
import { Modal } from "./Modal";
import { useLoad } from "../hooks";
import { fmtSize, joinList } from "../format";
import { scrub } from "../privacy";
import { agentLabel } from "../services";
import { errText, type Flash, onActivateKey } from "../util";

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
  off: "skillsPage.kindOff",
  library: "skillsPage.kindLibrary",
};

/** Folders AgentPlus can write to (and delete from). */
const writable = (r: SkillRoot) => r.kind === "own" || r.kind === "shared" || r.kind === "extra" || r.kind === "library";

/** The copy of `name` an agent loads: the first of its folders that has one. */
function loaded(o: SkillsOverview, agent: AgentId, name: string): SkillRoot | undefined {
  const a = o.agents.find((x) => x.agent === agent);
  for (const path of a?.roots ?? []) {
    const r = o.roots.find((x) => x.path === path);
    if (r?.skills.some((s) => s.name === name)) return r;
  }
  return undefined;
}

const isOff = (o: SkillsOverview, agent: AgentId, c: Copy) => !!o.agents.find((x) => x.agent === agent)?.disabled.some((d) => d === c.s.name || d === c.s.id);

export function seenBy(o: SkillsOverview, agent: AgentId, c: Copy): Seen {
  if (c.root.kind === "off" && c.root.owner === agent) return "disabled";
  if (loaded(o, agent, c.s.name) !== c.root) return "shadowed";
  return isOff(o, agent, c) ? "disabled" : "active";
}

export function groups(o: SkillsOverview): Group[] {
  const by = new Map<string, Copy[]>();
  for (const root of o.roots) for (const s of root.skills) by.set(s.name, [...(by.get(s.name) ?? []), { root, s }]);
  return [...by.entries()].sort(([a], [b]) => a.localeCompare(b)).map(([name, copies]) => {
    const readers = [...new Set(copies.flatMap((c) => c.root.readers))];
    const agents = readers.filter((a) => loaded(o, a, name)).map((agent) => ({
      agent,
      off: !!o.agents.find((x) => x.agent === agent)?.disabled.some((d) => d === name || copies.some((c) => c.s.id === d)),
    }));
    // Moved out by AgentPlus: the agent that owns the stash, switched off.
    for (const c of copies) if (c.root.kind === "off" && c.root.owner && !agents.some((x) => x.agent === c.root.owner)) agents.push({ agent: c.root.owner, off: true });
    return {
      name,
      copies,
      agents,
      builtin: copies.every((c) => c.root.kind === "builtin"),
      differs: copies.some((c) => !c.s.sig) || new Set(copies.map((c) => c.s.sig)).size > 1,
      problem: copies.some((c) => !!c.s.problem),
    };
  });
}

type Filter = "mine" | "builtin" | "all";
const FILTERS: [Filter, TKey][] = [["mine", "skillsPage.filterMine"], ["builtin", "skillsPage.filterBuiltin"], ["all", "skillsPage.filterAll"]];

/** What the Skills page and an agent's Skills tab share: the folders and their skills, the
 *  actions, and the copy / import dialogs. */
function useSkills(agents: AgentState[], flash: Flash) {
  const lang = useLang();
  const ids = useMemo(() => agents.map((a) => a.id).filter((id) => !id.includes("@")), [agents]);
  // Problems come from the backend in the UI language; `agents` changes after every apply.
  const { data, error, reload } = useLoad(() => api.skillsList(ids), [lang, agents]);
  const [copying, setCopying] = useState<Copy | null>(null);
  const [importing, setImporting] = useState(false);
  const all = useMemo(() => (data ? groups(data) : []), [data]);

  const run = async (p: Promise<unknown>, done?: string) => {
    try {
      await p;
      if (done) flash(done);
    } catch (e) {
      flash(errText(e), true);
    }
    await reload();
  };
  const remove = async (c: Copy) => {
    const users = c.root.readers.filter((a) => data && loaded(data, a, c.s.name) === c.root).map(agentLabel);
    const ok = await ask({
      title: t("skillsPage.deleteTitle", { name: c.s.name }),
      message: `${t("skillsPage.deleteMessage", { dir: scrub(c.s.dir) })}${users.length ? ` ${t("skillsPage.deleteUsers", { agents: joinList(users) })}` : ""}`,
      confirmText: t("common.delete"),
      danger: true,
    });
    if (ok) await run(api.skillsDelete(c.s.dir), t("skillsPage.deleted", { name: c.s.name }));
  };
  const toggle = (agent: AgentId, c: Copy, on: boolean) => run(api.skillsSetEnabled(agent, c.s.name, c.s.dir, on));

  const dialogs = (
    <>
      {copying && data && <CopyDialog c={copying} o={data} flash={flash} onClose={() => setCopying(null)} onDone={() => void reload()} />}
      {importing && <ImportDialog flash={flash} onClose={() => setImporting(false)} onDone={() => void reload()} />}
    </>
  );
  return { data, error, reload, all, remove, toggle, copy: setCopying, openImport: () => setImporting(true), dialogs };
}

export function SkillsPage({ agents, flash }: { agents: AgentState[]; flash: Flash }) {
  const { data, error, reload, all, remove, toggle, copy, openImport, dialogs } = useSkills(agents, flash);
  const [sel, setSel] = useState<string | null>(null);
  const [filter, setFilter] = useState<Filter>("mine");
  const [q, setQ] = useState("");
  const query = q.trim().toLowerCase();
  const list = all.filter((g) => (filter === "all" || (filter === "builtin") === g.builtin)
    && (!query || g.name.toLowerCase().includes(query) || g.copies.some((c) => c.s.description.toLowerCase().includes(query))));
  const picked = all.find((g) => g.name === sel) ?? null;

  return (
    <>
      <main className="page">
        <div className="page-top">
          <div className="page-head">
              <span className="page-icon"><Icon.book size={20} /></span>
            <div className="page-title">
              <h1>{t("skillsPage.title")}</h1>
              <span className="muted small hint">{t("skillsPage.subtitle")}</span>
            </div>
            <div className="row gap6">
              <button className="btn" onClick={() => void reload()}>{t("common.refresh")}</button>
              <button className="btn primary" onClick={openImport}><Icon.download size={12} />{t("skillsPage.import")}</button>
            </div>
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
                const pick = () => setSel(on ? null : g.name);
                return (
                  <div key={g.name} className={`hrow pick${on ? " on" : ""}`} role="button" tabIndex={0} aria-pressed={on} onClick={pick} onKeyDown={onActivateKey(pick)}>
                    <div className="minw0">
                      <div className="row gap6">
                        <span className="strong small">{g.name}</span>
                        {g.builtin && <span className="ptag tag-soft">{t("skillsPage.kindBuiltin")}</span>}
                        {g.copies.some((c) => c.root.kind === "library") && <span className="ptag tag-soft">{t("skillsPage.kindLibrary")}</span>}
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
        {picked && data
          ? <SkillDetail key={picked.name} g={picked} o={data} onClose={() => setSel(null)} onCopy={copy} onDelete={(c) => void remove(c)} onToggle={(a, c, on) => void toggle(a, c, on)} />
          : <Overview o={data} count={all.filter((g) => !g.builtin).length} />}
      </aside>
      {dialogs}
    </>
  );
}

/** An agent's own view (its page's Skills tab): the skills it loads, where each comes from. */
export function AgentSkillsTab({ agent, agents, flash, onOpenPage }: { agent: AgentId; agents: AgentState[]; flash: Flash; onOpenPage: () => void }) {
  const { data, error, all, remove, toggle, copy, openImport, dialogs } = useSkills(agents, flash);
  const [builtin, setBuiltin] = useState(false);
  const name = agentLabel(agent);
  // The copy this agent loads, or the one AgentPlus moved aside for it.
  const rows = data ? all.flatMap((g) => {
    const root = loaded(data, agent, g.name);
    const c = g.copies.find((x) => (root ? x.root === root : x.root.kind === "off" && x.root.owner === agent));
    return c ? [{ g, c }] : [];
  }) : [];
  const shown = rows.filter((r) => builtin || r.c.root.kind !== "builtin");
  const hidden = rows.length - rows.filter((r) => r.c.root.kind !== "builtin").length;
  return (
    <div className="stack12">
      <div className="row between gap6">
        <span className="muted small">{tn("skillsPage.agentCount", rows.filter((r) => r.c.root.kind !== "builtin").length)}{hidden > 0 && ` · ${tn("skillsPage.builtinCount", hidden)}`}</span>
        <div className="row gap6">
          {hidden > 0 && <button className="btn small" onClick={() => setBuiltin((b) => !b)}>{t(builtin ? "skillsPage.hideBuiltin" : "skillsPage.showBuiltin")}</button>}
          <button className="btn small" onClick={onOpenPage}>{t("skillsPage.openPage")}</button>
          <button className="btn small primary" onClick={openImport}><Icon.download size={12} />{t("skillsPage.import")}</button>
        </div>
      </div>
      {error && <ErrorBox text={error} />}
      {!data && !error && <div className="empty">{t("common.reading")}</div>}
      {data && shown.length === 0 && <div className="empty">{t("skillsPage.agentEmpty", { agent: name })}</div>}
      {shown.length > 0 && (
        <div className="stable">
          {shown.map(({ g, c }) => {
            const off = c.root.kind === "off" || isOff(data!, agent, c);
            const why = lockedReason(data!, agent, c);
            return (
              <div key={g.name} className={`hrow mcp-row${off ? " off" : ""}`}>
                <div className="minw0">
                  <div className="row gap6">
                    <span className="strong small">{g.name}</span>
                    <span className="ptag tag-soft" title={scrub(c.s.dir)}>{t(KIND[c.root.kind])}</span>
                    {g.differs && <span className="ptag tag-warn" title={t("skillsPage.differsTitle")}>{t("skillsPage.differs")}</span>}
                    {c.s.problem && <span className="ptag tag-warn" title={c.s.problem}>{t("skillsPage.problem")}</span>}
                  </div>
                  <div className="tiny muted ellipsis">{c.s.description || t("skillsPage.noDescription")}</div>
                </div>
                <div className="row gap6">
                  <span title={why ?? undefined}>
                    <Switch on={!off} disabled={!!why || c.root.kind === "builtin" && !data!.agents.find((a) => a.agent === agent)?.switchable} onChange={(on) => void toggle(agent, c, on)} label={t("skillsPage.toggleIn", { agent: name })} />
                  </span>
                  {c.root.kind !== "off" && <button className="icon-btn sm" aria-label={t("skillsPage.copyTo")} title={t("skillsPage.copyTo")} onClick={() => copy(c)}><Icon.copy size={12} /></button>}
                  {writable(c.root) && <button className="icon-btn sm" aria-label={t("skillsPage.delete")} title={t("skillsPage.delete")} onClick={() => void remove(c)}><Icon.trash size={12} /></button>}
                </div>
              </div>
            );
          })}
        </div>
      )}
      {dialogs}
    </div>
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

/** Why an agent's switch for this copy can't be used (null = it can). */
function lockedReason(o: SkillsOverview, agent: AgentId, c: Copy): string | null {
  const a = o.agents.find((x) => x.agent === agent);
  if (!a || a.switchable || c.root.kind === "off") return null;
  return c.root.kind === "own" && c.root.owner === agent ? null : t("skillsPage.noSwitch", { agent: agentLabel(agent) });
}

function SkillDetail({ g, o, onClose, onCopy, onDelete, onToggle }: {
  g: Group;
  o: SkillsOverview;
  onClose: () => void;
  onCopy: (c: Copy) => void;
  onDelete: (c: Copy) => void;
  onToggle: (agent: AgentId, c: Copy, on: boolean) => void;
}) {
  return (
    <section className="aside-cur mcp-scroll">
      <div className="row between">
        <div className="minw0">
          <h2 className="ellipsis">{g.name}</h2>
          <span className="tiny muted">{tn("skillsPage.inAgents", g.agents.filter((a) => !a.off).length)}</span>
        </div>
        <button className="icon-btn" aria-label={t("common.closeDetails")} onClick={onClose}><Icon.close /></button>
      </div>
      {g.differs && <span className="tiny warn-text">{t("skillsPage.differsHint")}</span>}
      {g.copies.map((c) => {
        // The copy's agents: the ones reading its folder, or the owner of a stash.
        const agents = c.root.kind === "off" && c.root.owner ? [c.root.owner] : c.root.readers;
        return (
          <div key={c.s.dir} className="mcp-variant">
            <div className="row between">
              <div className="row gap6">
                <span className="ptag tag-soft">{t(KIND[c.root.kind])}</span>
                {c.root.owner && c.root.kind === "own" && <span className="tiny muted">{agentLabel(c.root.owner)}</span>}
              </div>
              <div className="row gap6">
                {c.root.kind !== "off" && <button className="btn small" onClick={() => onCopy(c)}><Icon.copy size={12} />{t("skillsPage.copyTo")}</button>}
                {writable(c.root) && (
                  <button className="icon-btn sm" aria-label={t("skillsPage.delete")} title={t("skillsPage.delete")} onClick={() => onDelete(c)}><Icon.trash size={12} /></button>
                )}
              </div>
            </div>
            {c.s.description && <span className="small skills-desc">{c.s.description}</span>}
            {c.s.problem && <span className="tiny warn-text">{c.s.problem}</span>}
            <div className="kv">
              <div className="kv-row"><span className="tiny muted">{t("skillsPage.folder")}</span><span className="mono tiny mcp-wrap">{scrub(c.s.dir)}</span></div>
              <div className="kv-row"><span className="tiny muted">{t("skillsPage.content")}</span><span className="tiny">{tn("skillsPage.fileCount", c.s.files, { size: fmtSize(c.s.bytes) })}</span></div>
            </div>
            {agents.length > 0 && (
              <div className="mcp-holders">
                {agents.map((a) => {
                  const seen = seenBy(o, a, c);
                  const winner = seen === "shadowed" ? loaded(o, a, c.s.name) : undefined;
                  const why = seen === "shadowed" ? null : lockedReason(o, a, c);
                  return (
                    <div key={a} className={`mcp-holder${seen === "active" ? "" : " off"}`}>
                      <AgentIcon id={a} size={16} />
                      <span className="tiny grow minw0 ellipsis">
                        {agentLabel(a)} <span className="muted">· {seen === "shadowed" ? t("skillsPage.shadowed", { path: scrub(winner?.path ?? "") }) : c.root.kind === "off" ? t("skillsPage.parked") : t(seen === "disabled" ? "skillsPage.off" : "skillsPage.on")}</span>
                      </span>
                      {seen !== "shadowed" && (
                        <span title={why ?? undefined}>
                          <Switch on={seen === "active"} disabled={!!why} onChange={(on) => onToggle(a, c, on)} label={t("skillsPage.toggleIn", { agent: agentLabel(a) })} />
                        </span>
                      )}
                    </div>
                  );
                })}
              </div>
            )}
          </div>
        );
      })}
    </section>
  );
}

/** Where a copy can go: the shared folder first (most agents read it), the library, then
 *  each agent's own folder. */
function targetsFor(o: SkillsOverview): SkillRoot[] {
  const shared = o.roots.filter((r) => r.kind === "shared" && r.readers.length > 1);
  const lib = o.roots.filter((r) => r.kind === "library");
  const own = o.agents.flatMap((a) => {
    const r = a.roots.map((p) => o.roots.find((x) => x.path === p)).find((x) => x?.kind === "own" && x.owner === a.agent);
    return r ? [r] : [];
  });
  return [...shared, ...lib, ...own.filter((r, i) => own.indexOf(r) === i)];
}

function CopyDialog({ c, o, flash, onClose, onDone }: { c: Copy; o: SkillsOverview; flash: Flash; onClose: () => void; onDone: () => void }) {
  const folder = c.s.dir.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? c.s.name;
  const targets = targetsFor(o).filter((r) => r.path !== c.root.path);
  const there = (r: SkillRoot) => r.skills.find((s) => s.id === folder || s.name === c.s.name);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  const go = async () => {
    setBusy(true);
    setErr(null);
    let added = 0;
    try {
      for (const r of targets.filter((x) => picked.has(x.path))) {
        const got = await api.skillsCopy(c.s.dir, r.path, true);
        if (got !== "same") added++;
      }
      flash(tn("skillsPage.copied", added, { name: c.s.name }));
      onDone();
      onClose();
    } catch (e) {
      setErr(errText(e));
      setBusy(false);
      onDone();
    }
  };

  const foot = (
    <>
      <button className="btn" onClick={onClose}>{t("common.cancel")}</button>
      <button className="btn primary" disabled={busy || picked.size === 0} onClick={() => void go()}>{busy ? t("common.writing") : t("skillsPage.copy")}</button>
    </>
  );
  return (
    <Modal label={t("skillsPage.copyTitle", { name: c.s.name })} title={t("skillsPage.copyTitle", { name: c.s.name })} onClose={onClose} busy={busy} foot={foot}>
      <span className="tiny muted">{t("skillsPage.copyFrom", { dir: scrub(c.s.dir) })}</span>
      <div className="agent-picks skills-targets">
        {targets.map((r) => {
          const ex = there(r);
          const same = !!c.s.sig && ex?.sig === c.s.sig;
          return (
            <label key={r.path} className={`apick${picked.has(r.path) && !same ? " on" : ""}${same ? " dim" : ""}`}>
              <input type="checkbox" disabled={same} checked={picked.has(r.path) && !same}
                onChange={() => setPicked((p) => { const n = new Set(p); if (n.has(r.path)) n.delete(r.path); else n.add(r.path); return n; })} />
              {r.kind === "library" ? <Icon.layers size={16} /> : r.kind === "shared" ? <Icon.folder size={16} /> : <AgentIcon id={r.owner!} size={18} />}
              <span className="minw0">
                <span className="small block">{r.kind === "shared" ? t("skillsPage.sharedTarget", { n: r.readers.length }) : r.kind === "library" ? t("skillsPage.kindLibrary") : agentLabel(r.owner!)}</span>
                <span className="mono tiny muted block ellipsis">{scrub(r.path)}</span>
              </span>
              {same && <span className="tiny muted">{t("skillsPage.alreadyThere")}</span>}
              {ex && !same && <span className="tiny warn-text">{t("skillsPage.willReplace")}</span>}
            </label>
          );
        })}
      </div>
      <em className="muted tiny hint">{t("skillsPage.copyHint")}</em>
      {err && <ErrorBox text={err} />}
    </Modal>
  );
}

/** Brings skills into the library from a folder, a .zip or a Git address. */
function ImportDialog({ flash, onClose, onDone }: { flash: Flash; onClose: () => void; onDone: () => void }) {
  const [source, setSource] = useState("");
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<string | null>(null);
  const [got, setGot] = useState<SkillImported[] | null>(null);
  const [replace, setReplace] = useState<Set<string>>(new Set());

  const go = async (again: string[] = []) => {
    setBusy(true);
    setErr(null);
    try {
      const list = await api.skillsImport(source, again);
      // A second round only brings the replacements: keep what the first one found.
      setGot((prev) => (again.length && prev ? prev.map((p) => list.find((x) => x.name === p.name) ?? p) : list));
      setReplace(new Set());
      onDone();
      const added = list.filter((x) => x.result === "added" || x.result === "replaced").length;
      if (added) flash(tn("skillsPage.imported", added));
    } catch (e) {
      setErr(errText(e));
    } finally {
      setBusy(false);
    }
  };
  const browse = async () => {
    const p = await api.pickFolder(null, "skill").catch(() => null);
    if (p) setSource(p);
  };
  const waiting = got?.filter((x) => x.result === "exists") ?? [];

  const foot = got ? (
    <>
      <button className="btn" onClick={onClose}>{t("common.close")}</button>
      {waiting.length > 0 && <button className="btn primary" disabled={busy || replace.size === 0} onClick={() => void go([...replace])}>{t("skillsPage.replaceChosen", { n: replace.size })}</button>}
    </>
  ) : (
    <>
      <button className="btn" onClick={onClose}>{t("common.cancel")}</button>
      <button className="btn primary" disabled={busy || !source.trim()} onClick={() => void go()}>{busy ? t("skillsPage.importing") : t("skillsPage.import")}</button>
    </>
  );
  return (
    <Modal label={t("skillsPage.importTitle")} title={t("skillsPage.importTitle")} onClose={onClose} busy={busy} foot={foot}>
      {!got && (
        <div className="field">
          <span>{t("skillsPage.importFrom")}</span>
          <div className="row gap6">
            <input className="input mono grow" value={source} autoFocus onChange={(e) => setSource(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter" && source.trim() && !busy) void go(); }}
              placeholder="https://github.com/owner/repo  ·  D:\\skills  ·  skill.zip" />
            <button className="btn" onClick={() => void browse()}><Icon.folder size={13} />{t("skillsPage.browse")}</button>
          </div>
          <em className="muted tiny hint">{t("skillsPage.importHint")}</em>
        </div>
      )}
      {got && (
        <div className="stack6">
          <span className="tiny muted">{t("skillsPage.importedInto")}</span>
          <div className="mcp-holders">
            {got.map((x) => (
              <label key={x.name} className="mcp-holder">
                {x.result === "exists"
                  ? <input type="checkbox" checked={replace.has(x.name)} onChange={() => setReplace((p) => { const n = new Set(p); if (n.has(x.name)) n.delete(x.name); else n.add(x.name); return n; })} />
                  : <Icon.check size={13} />}
                <span className="tiny grow minw0 ellipsis"><span className="strong">{x.name}</span> <span className="muted">· {x.description}</span></span>
                <span className={`tiny${x.result === "exists" ? " warn-text" : " muted"}`}>{t(RESULT[x.result])}</span>
              </label>
            ))}
          </div>
          {waiting.length > 0 && <em className="muted tiny">{t("skillsPage.existsHint")}</em>}
        </div>
      )}
      {err && <ErrorBox text={err} />}
    </Modal>
  );
}

const RESULT: Record<SkillImported["result"], TKey> = {
  added: "skillsPage.resultAdded",
  replaced: "skillsPage.resultReplaced",
  same: "skillsPage.resultSame",
  exists: "skillsPage.resultExists",
};
