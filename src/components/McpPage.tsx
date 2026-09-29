import { useEffect, useMemo, useState } from "react";
import { type AgentId, type AgentState, type McpInput, type McpKv, type McpLink, type McpProbe, type McpServer, type McpSource, api } from "../api";
import { type Draft, type McpView, deleteMcp, keys, mcpView, sameCore, setMcpEnabled, undoMcp, upsertMcp } from "../draft";
import { t, tn, useLang } from "../i18n";
import { AgentIcon, Icon } from "./icons";
import { ErrorBox, Switch } from "./controls";
import { ask, askCheck } from "./Confirm";
import { PendingPanel } from "./HubAside";
import { MCP_TRANSPORT, type McpEdit, McpDialog, type McpTarget } from "./McpDialog";
import { useLoad } from "../hooks";
import { scrub } from "../privacy";
import { agentLabel } from "../services";
import { errText, type Flash, onActivateKey } from "../util";

const LIBRARY = "library" as const;

/** One agent's (or the library's) copy of a server, with its pending change. */
interface Entry {
  at: McpSource;
  v: McpView;
}

/** A finished connection test. */
type Probed = { ok: McpProbe } | { err: string };

const probeKey = (e: Entry) => `${e.at}|${e.v.s.name}`;

/** A connection test's outcome: running, the server and its tools, or why it failed. */
function ProbeResult({ r }: { r: Probed | null | undefined }) {
  if (r === undefined) return null;
  if (r === null) return <span className="tiny muted mcp-probe">{t("mcpPage.testing")}</span>;
  if ("err" in r) return <ErrorBox className="mcp-probe" text={t("mcpPage.testFailed", { error: r.err })} />;
  const p = r.ok;
  return (
    <div className="mcp-probe">
      <span className="tiny ok-text">
        {t("mcpPage.testOk", { tools: tn("mcpPage.toolCount", p.tools.length), ms: p.ms })}
        {p.server && <span className="muted"> · {p.server}</span>}
      </span>
      {p.tools.length > 0 && (
        <div className="mcp-tools">
          {p.tools.map((x) => <span key={x.name} className="ptag tag-soft mono" title={x.description || undefined}>{x.name}</span>)}
        </div>
      )}
    </div>
  );
}

/** A server name across agents; `variants` groups the copies by what they run. */
interface Group {
  name: string;
  entries: Entry[];
  variants: Entry[][];
}

function groups(entries: Entry[]): Group[] {
  const by = new Map<string, Entry[]>();
  for (const e of entries) by.set(e.v.s.name, [...(by.get(e.v.s.name) ?? []), e]);
  return [...by.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([name, list]) => {
      const v = new Map<string, Entry[]>();
      for (const e of list) v.set(e.v.s.sig, [...(v.get(e.v.s.sig) ?? []), e]);
      return { name, entries: list, variants: [...v.values()] };
    });
}

const quote = (a: string) => (/[\s"]/.test(a) ? `"${a.replace(/"/g, '\\"')}"` : a);

/** What runs: the command line, or the URL. */
function summary(s: McpServer): string {
  return s.transport === "stdio" ? [s.command ?? "", ...s.args].map(quote).join(" ") : s.url ?? "";
}

const sourceName = (at: McpSource) => (at === LIBRARY ? t("mcpPage.library") : agentLabel(at));

function SourceIcon({ at, size }: { at: McpSource; size: number }) {
  return at === LIBRARY ? <span className="mcp-lib-icon"><Icon.layers size={size - 4} /></span> : <AgentIcon id={at} size={size} />;
}

function stateText(e: Entry): string {
  if (e.at === LIBRARY) return t("mcpPage.inLibrary");
  const { s, pending } = e.v;
  if (pending === "deleted") return t("mcpPage.pendingDelete");
  if (pending === "new") return t("mcpPage.pendingNew");
  const state = t(s.enabled ? "mcpPage.on" : s.stashed ? "mcpPage.stashed" : "mcpPage.off");
  return pending ? `${state} · ${t("mcpPage.pending")}` : state;
}

/** What the MCP page and an agent's MCP tab share: the servers with their pending changes,
 *  the edits, and the add / edit dialog. */
function useMcp(agents: AgentState[], drafts: Record<string, Draft>, setDraftFor: (agent: string, d: Draft | ((cur: Draft) => Draft)) => void, flash: Flash, onRenamed?: (from: string, to: string) => void) {
  const lang = useLang();
  const ids = useMemo(() => agents.map((a) => a.id).filter((id) => !id.includes("@")), [agents]);
  // Errors come from the backend in the UI language; `agents` changes after every apply.
  const { data, error, reload } = useLoad(() => api.mcpList(ids), [lang, agents]);
  const [dialog, setDialog] = useState<McpEdit | null | undefined>(undefined);
  const [fromLink, setFromLink] = useState<{ link: McpLink; n: number } | null>(null);
  const [defaultTo, setDefaultTo] = useState<McpSource[]>([]);
  /** Connection tests, by `<where>|<name>`: running (null), or what came back. */
  const [probes, setProbes] = useState<Record<string, Probed | null>>({});

  const source = (from: [McpSource, string]) => from[0] === LIBRARY
    ? data?.library.find((s) => s.name === from[1])
    : data?.agents.find((a) => a.agent === from[0])?.servers.find((s) => s.name === from[1]);
  const views = useMemo(() => {
    const out = new Map<McpSource, McpView[]>();
    for (const a of data?.agents ?? []) if (a.supported) out.set(a.agent, mcpView(a.servers, drafts[a.agent] ?? {}, source));
    if (data) out.set(LIBRARY, data.library.map((s) => ({ s, pending: null, exists: true })));
    return out;
  }, [data, drafts]);
  const list = useMemo(() => groups([...views].flatMap(([at, vs]) => vs.map((v) => ({ at, v })))), [views]);
  const targets: McpTarget[] = [...views].map(([id, vs]) => ({ id, name: sourceName(id), names: new Set(vs.map((v) => v.s.name)) }));

  // A function of the draft as it is when applied: `edit` often runs after a confirm dialog.
  const edit = (agent: McpSource, f: (d: Draft) => Draft) => { if (agent !== LIBRARY) setDraftFor(agent, f); };
  const attempt = async (p: Promise<unknown>) => {
    try {
      await p;
      await reload();
    } catch (e) {
      flash(errText(e), true);
    }
  };

  /** Where the values an entry shows are read from (a pending copy: its own source). */
  const fromOf = (e: Entry): [McpSource, string] | undefined => {
    if (e.at === LIBRARY || (e.v.exists && e.v.pending !== "edited")) return [e.at, e.v.s.name];
    const op = drafts[e.at]?.[keys.mcpUpsert(e.v.s.name)];
    return op?.op === "upsert_mcp" ? op.server.from : undefined;
  };
  const openEdit = (variant: Entry[]) => {
    // A copy is read from a holder that keeps the server (see `writeOrder`).
    const e = variant.find((x) => x.v.exists && !x.v.pending) ?? variant.find((x) => x.v.exists && x.v.pending === "toggled") ?? variant[0];
    setDialog({ s: e.v.s, from: fromOf(e), holders: variant.filter((x) => x.v.pending !== "deleted").map((x) => x.at) });
  };
  /** The add dialog; `to` is ticked to begin with, `link` fills it in. */
  const openAdd = (to: McpSource[] = [], link: { link: McpLink; n: number } | null = null) => {
    setDefaultTo(to);
    setFromLink(link);
    setDialog(null);
  };

  /** The copies of a server that stay once the pending removals are written. */
  const kept = (name: string) => list.find((g) => g.name === name)?.entries.filter((x) => x.v.pending !== "deleted") ?? [];

  /**
   * Asks before the last copy of a server goes. With `from`, it offers to keep a copy in the
   * MCP library first. True to go ahead.
   */
  const confirmLast = async (name: string, message: string, from?: [McpSource, string], s?: McpServer): Promise<boolean> => {
    const opts = { title: t("mcpPage.lastTitle", { name }), message, confirmText: t("common.delete"), danger: true };
    if (!from || !s) return ask(opts);
    const keep = await askCheck({ ...opts, check: { label: t("mcpPage.keepInLibrary"), value: false } });
    if (keep === null) return false;
    if (keep) {
      const pair = (kv: McpKv[]) => kv.map(({ key, value }) => ({ key, value }));
      try {
        await api.mcpLibrarySave({ name: s.name, transport: s.transport, command: s.command, args: s.args, cwd: s.cwd, url: s.url, env: pair(s.env), headers: pair(s.headers), enabled: true, from });
        await reload();
      } catch (e) {
        flash(errText(e), true);
        return false;
      }
    }
    return true;
  };

  const remove = async (e: Entry) => {
    const name = e.v.s.name;
    const left = kept(name);
    // A copy that is only a pending addition isn't anyone's configuration yet.
    if (left.length === 1 && left[0].at === e.at && (e.at === LIBRARY || e.v.exists)) {
      const lib = e.at === LIBRARY;
      const ok = await confirmLast(name, t(lib ? "mcpPage.lastLibrary" : "mcpPage.lastAgent", { agent: sourceName(e.at) }), lib ? undefined : fromOf(e), lib ? undefined : e.v.s);
      if (!ok) return;
    }
    if (e.at === LIBRARY) await attempt(api.mcpLibraryDelete(name));
    else edit(e.at, (d) => deleteMcp(d, name, e.v.exists));
  };

  /** Tests the copy a server's entry reads from (a pending copy: the one it was copied from). */
  const test = async (e: Entry) => {
    const from = fromOf(e);
    if (!from) return;
    const key = probeKey(e);
    setProbes((p) => ({ ...p, [key]: null }));
    const done = await api.mcpProbe(from[0], from[1]).then((ok): Probed => ({ ok }), (err): Probed => ({ err: errText(err) }));
    setProbes((p) => ({ ...p, [key]: done }));
  };
  // A pending edit runs something the configs don't have yet; a pending copy is testable
  // through its source while it runs the same thing.
  const canTest = (e: Entry) => !!fromOf(e) && e.v.pending !== "edited" && !e.v.s.sig.startsWith("draft:");

  const toggle = (e: Entry, on: boolean) => {
    const actual = e.v.exists ? data?.agents.find((a) => a.agent === e.at)?.servers.find((s) => s.name === e.v.s.name)?.enabled ?? null : null;
    edit(e.at, (d) => setMcpEnabled(d, e.v.s.name, on, actual));
  };
  const undo = (e: Entry) => edit(e.at, (d) => undoMcp(d, e.v.s.name));

  const save = async (input: McpInput, to: McpSource[], removeFrom: McpSource[]): Promise<boolean> => {
    const old = dialog?.s.name ?? input.name;
    // Every copy unticked: nothing keeps the server.
    if (dialog && to.length === 0 && removeFrom.length > 0 && kept(old).every((x) => removeFrom.includes(x.at))) {
      const offer = !removeFrom.includes(LIBRARY) && dialog.from;
      if (!(await confirmLast(old, t("mcpPage.lastEverywhere"), offer ? dialog.from : undefined, offer ? dialog.s : undefined))) return false;
    }
    const viewIn = (at: McpSource) => views.get(at)?.find((v) => v.s.name === old);
    for (const at of to) {
      if (at === LIBRARY) continue;
      const v = viewIn(at);
      const renames = !!v && input.replaces !== undefined;
      // An agent that already runs exactly this gets no change (only the newly ticked ones do).
      if (v && !renames && sameCore(input, v.s)) continue;
      edit(at, (d) => upsertMcp(d, { ...input, enabled: v ? v.s.enabled : true, replaces: renames ? input.replaces : undefined }));
    }
    for (const at of removeFrom) if (at !== LIBRARY) edit(at, (d) => deleteMcp(d, old, !!viewIn(at)?.exists));
    if (to.includes(LIBRARY)) await api.mcpLibrarySave({ ...input, replaces: dialog?.holders.includes(LIBRARY) ? input.replaces : undefined });
    if (removeFrom.includes(LIBRARY)) await api.mcpLibraryDelete(old);
    if (to.includes(LIBRARY) || removeFrom.includes(LIBRARY)) await reload();
    if (input.name !== old) onRenamed?.(old, input.name);
    return true;
  };

  const dialogEl = dialog !== undefined && data && (
    <McpDialog key={fromLink?.n ?? 0} edit={dialog} targets={targets} link={dialog === null ? fromLink?.link : undefined} defaultTo={defaultTo} onSave={save}
      onClose={() => { setDialog(undefined); setFromLink(null); }} />
  );

  return { data, error, reload, views, list, openEdit, openAdd, remove, toggle, undo, test, canTest, probes, dialogEl };
}

interface Props {
  /** The agents in the sidebar. */
  agents: AgentState[];
  /** Everything "Apply" writes (agents and open project configs). */
  pending: AgentState[];
  drafts: Record<string, Draft>;
  setDraftFor: (agent: string, d: Draft | ((cur: Draft) => Draft)) => void;
  busy: boolean;
  onApplyAll: () => void;
  onDiscard: (agent: AgentId | null) => void;
  flash: Flash;
  /** An MCP import link to fill in the add dialog with; `n` tells one link from the next. */
  link: { link: McpLink; n: number } | null;
  /** The link has filled in the dialog: it shouldn't open again when the page comes back. */
  onLinkUsed: () => void;
}

export function McpPage({ agents, pending, drafts, setDraftFor, busy, onApplyAll, onDiscard, flash, link, onLinkUsed }: Props) {
  const [sel, setSel] = useState<string | null>(null);
  const { data, error, reload, list, openEdit, openAdd, remove, toggle, undo, test, canTest, probes, dialogEl } = useMcp(agents, drafts, setDraftFor, flash, (from, to) => setSel((s) => (s === from ? to : s)));
  useEffect(() => {
    if (!link) return;
    openAdd(link.link.agents, link);
    onLinkUsed();
  }, [link?.n]);
  const picked = list.find((g) => g.name === sel) ?? null;
  const differ = list.filter((g) => g.variants.length > 1).length;

  return (
    <>
      <main className="page">
        <div className="page-top">
          <div className="page-head">
              <span className="page-icon"><Icon.plug size={20} /></span>
            <div className="page-title">
              <h1>{t("mcpPage.title")}</h1>
              <span className="muted small hint">{t("mcpPage.subtitle")}</span>
            </div>
            <div className="row gap6">
              <button className="btn" onClick={() => void reload()}>{t("common.refresh")}</button>
              <button className="btn primary" disabled={!data} onClick={() => openAdd()}><Icon.plus size={12} />{t("mcpPage.add")}</button>
            </div>
          </div>
        </div>
        <div className="page-body">
          {error && (data ? <ErrorBox text={error} /> : <div className="empty">{scrub(error)}</div>)}
          {!data && !error && <div className="empty">{t("common.reading")}</div>}
          {data?.agents.filter((a) => a.error).map((a) => <ErrorBox key={a.agent} text={t("mcpPage.agentError", { agent: agentLabel(a.agent), error: a.error! })} />)}
          {data && list.length === 0 && <div className="empty">{t("mcpPage.empty")}</div>}
          {list.length > 0 && (
            <div className="stable">
              {list.map((g) => {
                const on = sel === g.name;
                const toggle = () => setSel(on ? null : g.name);
                const shown = g.entries.find((e) => e.v.pending !== "deleted") ?? g.entries[0];
                return (
                  <div key={g.name} className={`hrow pick${on ? " on" : ""}`} role="button" tabIndex={0} aria-pressed={on} onClick={toggle} onKeyDown={onActivateKey(toggle)}>
                    <div className="minw0">
                      <div className="row gap6">
                        <span className="strong small">{g.name}</span>
                        <span className="ptag tag-soft">{MCP_TRANSPORT[shown.v.s.transport]}</span>
                        {g.variants.length > 1 && <span className="ptag tag-warn" title={t("mcpPage.differsTitle")}>{t("mcpPage.differs")}</span>}
                        {g.entries.some((e) => e.v.pending) && <span className="ptag tag-new">{t("mcpPage.pending")}</span>}
                      </div>
                      <div className="mono tiny muted ellipsis">{scrub(summary(shown.v.s))}</div>
                    </div>
                    <div className="mcp-agents">
                      {g.entries.map((e) => (
                        <span key={e.at} className={`mcp-agent${e.v.s.enabled && e.v.pending !== "deleted" ? "" : " off"}${e.v.pending ? " dirty" : ""}`} title={`${sourceName(e.at)} · ${stateText(e)}`}>
                          <SourceIcon at={e.at} size={20} />
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
          <McpDetail key={picked.name} g={picked} onClose={() => setSel(null)} onEdit={openEdit} onToggle={toggle} onRemove={(e) => void remove(e)} onUndo={undo}
            onTest={(e) => void test(e)} canTest={canTest} probes={probes} />
        ) : (
          <section className="aside-cur mcp-scroll">
            <h2>{t("mcpPage.overview")}</h2>
            <span className="muted tiny hint">{t("mcpPage.overviewHint")}</span>
            {data && (
              <>
                <div className="hub-stats">
                  <div><b>{list.length}</b><span>{t("mcpPage.statServers")}</span></div>
                  <div><b>{data.agents.filter((a) => a.servers.length > 0).length}</b><span>{t("mcpPage.statAgents")}</span></div>
                  <div><b>{differ}</b><span>{t("mcpPage.statDiffer")}</span></div>
                </div>
                <div className="kv">
                  {data.agents.map((a) => (
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
                  <div className="kv-row">
                    <span className="tiny row gap6"><SourceIcon at={LIBRARY} size={16} />{t("mcpPage.library")}</span>
                    <span className="minw0">
                      <span className="tiny block">{tn("mcpPage.serverCount", data.library.length)}</span>
                      <span className="tiny muted block">{t("mcpPage.libraryHint")}</span>
                    </span>
                  </div>
                </div>
              </>
            )}
          </section>
        )}
        <PendingPanel pending={pending} drafts={drafts} busy={busy} onDiscard={onDiscard} onApplyAll={onApplyAll} emptyHint={t("mcpPage.pendingHint")} />
      </aside>
      {dialogEl}
    </>
  );
}


/** An agent's own MCP servers (its page's MCP tab): the same edits as the MCP page. */
export function AgentMcpTab({ agent, agents, drafts, setDraftFor, flash, onOpenPage }: {
  agent: AgentId;
  /** Every agent in the sidebar (edits can copy a server to them). */
  agents: AgentState[];
  drafts: Record<string, Draft>;
  setDraftFor: (agent: string, d: Draft | ((cur: Draft) => Draft)) => void;
  flash: Flash;
  onOpenPage: () => void;
}) {
  const { data, error, list, openEdit, openAdd, remove, toggle, undo, test, canTest, probes, dialogEl } = useMcp(agents, drafts, setDraftFor, flash);
  const mine = data?.agents.find((a) => a.agent === agent);
  const rows = list.flatMap((g) => g.entries.filter((e) => e.at === agent).map((e) => ({ g, e })));
  const name = agentLabel(agent);
  return (
    <div className="stack12">
      <div className="row between gap6">
        <span className="mono muted small ellipsis" title={scrub(mine?.file ?? "")}>{scrub(mine?.file ?? "")}</span>
        <div className="row gap6">
          <button className="btn small" onClick={onOpenPage}>{t("mcpPage.openPage")}</button>
          <button className="btn small primary" disabled={!data} onClick={() => openAdd([agent])}><Icon.plus size={12} />{t("mcpPage.add")}</button>
        </div>
      </div>
      {error && <ErrorBox text={error} />}
      {mine?.error && <ErrorBox text={mine.error} />}
      {!data && !error && <div className="empty">{t("common.reading")}</div>}
      {data && rows.length === 0 && !mine?.error && <div className="empty">{t("mcpPage.agentEmpty", { agent: name })}</div>}
      {rows.length > 0 && (
        <div className="stable">
          {rows.map(({ g, e }) => {
            const s = e.v.s;
            const gone = e.v.pending === "deleted";
            return (
              <div key={s.name} className={`hrow mcp-row${s.enabled && !gone ? "" : " off"}`}>
                <div className="minw0">
                  <div className="row gap6">
                    <span className="strong small">{s.name}</span>
                    <span className="ptag tag-soft">{MCP_TRANSPORT[s.transport]}</span>
                    {g.variants.length > 1 && <span className="ptag tag-warn" title={t("mcpPage.differsTitle")}>{t("mcpPage.differs")}</span>}
                    {e.v.pending && <span className="ptag tag-new">{stateText(e)}</span>}
                    {g.entries.length > 1 && <span className="tiny muted">{tn("mcpPage.alsoIn", g.entries.length - 1)}</span>}
                  </div>
                  <div className="mono tiny muted ellipsis">{scrub(summary(s))}</div>
                  <ProbeResult r={probes[probeKey(e)]} />
                </div>
                <div className="row gap6">
                  {e.v.pending && <button className="link tiny" onClick={() => undo(e)}>{t("common.undo")}</button>}
                  <button className="icon-btn sm" disabled={!canTest(e) || probes[probeKey(e)] === null} aria-label={t("mcpPage.test")} title={canTest(e) ? t("mcpPage.testHint") : t("mcpPage.testPending")} onClick={() => void test(e)}><Icon.pulse size={12} /></button>
                  {!gone && <Switch on={s.enabled} onChange={(on) => toggle(e, on)} label={t("mcpPage.toggleIn", { agent: name })} />}
                  {!gone && <button className="icon-btn sm" aria-label={t("mcpPage.editOrCopy")} title={t("mcpPage.editOrCopy")} onClick={() => openEdit(g.variants.find((v) => v.includes(e)) ?? [e])}><Icon.edit size={12} /></button>}
                  {!gone && <button className="icon-btn sm" aria-label={t("mcpPage.removeFrom", { agent: name })} title={t("mcpPage.removeFrom", { agent: name })} onClick={() => void remove(e)}><Icon.trash size={12} /></button>}
                </div>
              </div>
            );
          })}
        </div>
      )}
      {dialogEl}
    </div>
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

function McpDetail({ g, onClose, onEdit, onToggle, onRemove, onUndo, onTest, canTest, probes }: {
  g: Group;
  onClose: () => void;
  onEdit: (variant: Entry[]) => void;
  onToggle: (e: Entry, on: boolean) => void;
  onRemove: (e: Entry) => void;
  onUndo: (e: Entry) => void;
  onTest: (e: Entry) => void;
  canTest: (e: Entry) => boolean;
  probes: Record<string, Probed | null>;
}) {
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
        const s = (v.find((e) => e.v.pending !== "deleted") ?? v[0]).v.s;
        const extras = v.filter((e) => Object.keys(e.v.s.extra).length > 0);
        // One test per definition: through the first copy that is in a config already.
        const tested = v.find((e) => e.v.exists && canTest(e)) ?? v.find(canTest);
        return (
          <div key={s.sig} className="mcp-variant">
            <div className="row between">
              <span className="tiny strong">{g.variants.length > 1 ? t("mcpPage.variant", { i: i + 1, n: g.variants.length }) : t("mcpPage.definition")}</span>
              <div className="row gap6">
                <button className="btn small" disabled={!tested || probes[probeKey(tested)] === null} title={tested ? t("mcpPage.testHint") : t("mcpPage.testPending")}
                  onClick={() => tested && onTest(tested)}><Icon.pulse size={12} />{t("mcpPage.test")}</button>
                <button className="btn small" onClick={() => onEdit(v)}><Icon.edit size={12} />{t("mcpPage.editOrCopy")}</button>
              </div>
            </div>
            {tested && <ProbeResult r={probes[probeKey(tested)]} />}
            <div className="mcp-holders">
              {v.map((e) => (
                <div key={e.at} className={`mcp-holder${e.v.s.enabled && e.v.pending !== "deleted" ? "" : " off"}`}>
                  <SourceIcon at={e.at} size={16} />
                  <span className="tiny grow minw0 ellipsis">{sourceName(e.at)} <span className="muted">· {stateText(e)}</span></span>
                  {e.v.pending && e.at !== LIBRARY && <button className="link tiny" onClick={() => onUndo(e)}>{t("common.undo")}</button>}
                  {e.at !== LIBRARY && e.v.pending !== "deleted" && (
                    <Switch on={e.v.s.enabled} onChange={(on) => onToggle(e, on)} label={t("mcpPage.toggleIn", { agent: sourceName(e.at) })} />
                  )}
                  {e.v.pending !== "deleted" && (
                    <button className="icon-btn sm" aria-label={t("mcpPage.removeFrom", { agent: sourceName(e.at) })} title={t("mcpPage.removeFrom", { agent: sourceName(e.at) })} onClick={() => onRemove(e)}><Icon.trash size={12} /></button>
                  )}
                </div>
              ))}
            </div>
            <div className="kv">
              <div className="kv-row"><span className="tiny muted">{t("mcpPage.transport")}</span><span className="tiny">{MCP_TRANSPORT[s.transport]}</span></div>
              {s.transport === "stdio" ? (
                <div className="kv-row"><span className="tiny muted">{t("mcpPage.command")}</span><span className="mono tiny mcp-wrap">{scrub(summary(s))}</span></div>
              ) : (
                <div className="kv-row"><span className="tiny muted">{t("mcpPage.url")}</span><span className="mono tiny mcp-wrap">{scrub(s.url ?? "")}</span></div>
              )}
              {s.cwd && <div className="kv-row"><span className="tiny muted">{t("mcpPage.cwd")}</span><span className="mono tiny mcp-wrap">{scrub(s.cwd)}</span></div>}
              {s.env.length > 0 && <div className="kv-row"><span className="tiny muted">{t("mcpPage.env")}</span><Pairs items={s.env} /></div>}
              {s.headers.length > 0 && <div className="kv-row"><span className="tiny muted">{t("mcpPage.headers")}</span><Pairs items={s.headers} /></div>}
              {extras.map((e) => (
                <div key={e.at} className="kv-row">
                  <span className="tiny muted">{t("mcpPage.otherFields", { agent: sourceName(e.at) })}</span>
                  <span className="minw0">
                    {Object.entries(e.v.s.extra).map(([k, val]) => (
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
