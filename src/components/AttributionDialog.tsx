import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { type AttributionLive, type AttributionTarget, api } from "../api";
import { BANK_SOURCE, type Bank, type Match } from "../attribution/bank";
import { AttributionError } from "../attribution/core";
import { type HistoryEntry, type ResultSummary, historyEntry, outcome, parseHistory, summarize } from "../attribution/history";
import { CancelToken, RUN_CHOICES, type RunReport, type Runs, type SampleRow, runAttribution } from "../attribution/run";
import license from "../attribution/LICENSE-ModelTrace.txt?raw";
import { API_LABEL } from "../draft";
import { locale, t, tn, useLang } from "../i18n";
import { scrub } from "../privacy";
import { errText } from "../util";
import { ask } from "./Confirm";
import { Seg } from "./controls";
import { Icon } from "./icons";
import { Modal } from "./Modal";

interface Props {
  agent: string;
  /** The model list's provider: an id, or `CATALOG` for Codex's shared catalog. */
  provider: string;
  /** The model id as the row lists it: what the requests send. */
  model: string;
  match: Match;
  bank: Bank;
  /** Provider names by id, for display. */
  names: Record<string, string>;
  /** Whether a provider has changes not applied yet (the test only uses the saved config). */
  pending: (provider: string) => boolean;
  onClose: () => void;
}

const fmt = (p: number) => p.toLocaleString(locale(), { style: "percent", minimumFractionDigits: 1, maximumFractionDigits: 1 });
/** A probability as a percentage; a tiny one shows as "<0.1%" rather than a flat 0. */
const pct = (p: number) => (p > 0 && p < 0.0005 ? t("attributionDialog.below", { value: fmt(0.001) }) : fmt(p));
const secs = (ms: number) => (ms / 1000).toLocaleString(locale(), { maximumFractionDigits: 1 });
const apiLabel = (api: string) => API_LABEL[api as keyof typeof API_LABEL];

function errorOf(e: unknown): string {
  if (e instanceof AttributionError) {
    if (e.code === "bankInvalid") return t("attributionDialog.errBankInvalid", { detail: e.detail });
    if (e.code === "bankMissing") return t("attributionDialog.errBankMissing");
    return t("attributionDialog.errNoValid");
  }
  return errText(e);
}

/** One sample's part of the live log: what streamed in, in order. */
interface LogSample {
  index: number;
  requests: number;
  status: number | null;
  parts: { reasoning: boolean; text: string }[];
}

/** Text kept per sample in the log; the start of a longer one is dropped. */
const LOG_KEEP = 60_000;

function appendLog(logs: LogSample[], index: number, e: AttributionLive): LogSample[] {
  const at = logs.findIndex((l) => l.index === index);
  const cur: LogSample = at < 0 ? { index, requests: 0, status: null, parts: [] } : { ...logs[at], parts: [...logs[at].parts] };
  if (e.type === "request") {
    cur.requests = e.n;
    cur.status = null;
  } else if (e.type === "status") {
    cur.status = e.status;
  } else {
    const reasoning = e.type === "reasoning";
    const last = cur.parts[cur.parts.length - 1];
    if (last && last.reasoning === reasoning) cur.parts[cur.parts.length - 1] = { reasoning, text: last.text + e.text };
    else cur.parts.push({ reasoning, text: e.text });
    let total = cur.parts.reduce((s, p) => s + p.text.length, 0);
    while (total > LOG_KEEP && cur.parts.length) {
      const cut = Math.min(cur.parts[0].text.length, total - LOG_KEEP);
      total -= cut;
      cur.parts[0] = { ...cur.parts[0], text: cur.parts[0].text.slice(cut) };
      if (!cur.parts[0].text) cur.parts.shift();
    }
  }
  return at < 0 ? [...logs, cur] : logs.map((l, i) => (i === at ? cur : l));
}

type View = "test" | "history";

/** Sends N fingerprint challenges to one model of one provider and shows how its answers match the bank. */
export function AttributionDialog({ agent, provider, model, match, bank, names, pending, onClose }: Props) {
  const lang = useLang();
  const [view, setView] = useState<View>("test");
  const [target, setTarget] = useState<AttributionTarget | null>(null);
  const [targetError, setTargetError] = useState<string | null>(null);
  const [runs, setRuns] = useState<Runs>(3);
  const [report, setReport] = useState<RunReport | null>(null);
  const [logs, setLogs] = useState<LogSample[]>([]);
  const [runError, setRunError] = useState<string | null>(null);
  const [running, setRunning] = useState(false);
  const [history, setHistory] = useState<HistoryEntry[] | null>(null);
  const [historyError, setHistoryError] = useState<string | null>(null);
  /** The current run; anything from another (older) run is dropped. */
  const run = useRef<CancelToken | null>(null);
  /** A run is on its way (set at once, so a double click can't start two). */
  const active = useRef(false);

  useEffect(() => {
    let alive = true;
    setTarget(null);
    setTargetError(null);
    api.attributionTarget(agent, provider, model)
      .then((x) => { if (alive) setTarget(x); })
      .catch((e) => { if (alive) setTargetError(errText(e)); });
    return () => { alive = false; };
  }, [agent, provider, model, lang]);

  useEffect(() => {
    let alive = true;
    api.attributionHistory()
      .then((l) => { if (alive) setHistory(parseHistory(l)); })
      .catch((e) => { if (alive) setHistoryError(errText(e)); });
    return () => { alive = false; };
  }, []);

  // Closing (or a new target, which mounts a new dialog) stops the run.
  useEffect(() => () => { run.current?.cancel(); run.current = null; }, []);

  const unapplied = !!target && pending(target.provider);
  const blocked = target?.blocked?.message ?? (unapplied ? t("attributionDialog.pendingChanges") : null);
  const canStart = !!target && !blocked && !running;

  /** Keeps a finished run in the history (one that sent nothing isn't a result). */
  const keep = (t0: AttributionTarget, r: RunReport) => {
    if (r.requests === 0) return;
    api.attributionHistoryAdd(historyEntry(t0, names[t0.provider] ?? null, match.candidate, r, BANK_SOURCE.commit))
      .then((l) => { setHistory(parseHistory(l)); setHistoryError(null); })
      .catch((e) => setHistoryError(errText(e)));
  };

  const start = () => {
    if (!target || !canStart || active.current) return;
    active.current = true;
    const token = new CancelToken();
    run.current = token;
    setView("test");
    setRunning(true);
    setReport(null);
    setLogs([]);
    setRunError(null);
    const t0 = target;
    runAttribution({
      runs, bank, token,
      sample: (c, live) => api.attributionSample(agent, provider, model, c.prompt, t0.fp, live),
      onUpdate: (r) => { if (run.current === token) setReport(r); },
      onLive: (i, e) => { if (run.current === token) setLogs((l) => appendLog(l, i, e)); },
      errorText: errText,
    })
      .then((r) => {
        if (run.current !== token) return;
        setReport(r);
        keep(t0, r);
      })
      .catch((e) => { if (run.current === token) setRunError(errorOf(e)); })
      .finally(() => {
        if (run.current !== token) return;
        active.current = false;
        setRunning(false);
      });
  };
  const cancel = () => run.current?.cancel();

  const current = report?.rows.find((r) => r.status === "running");
  const done = !!report && !report.rows.some((r) => r.status === "running" || r.status === "pending");
  const foot = (
    <>
      <span className="grow tiny muted">
        {running && current && <><span className="spin" aria-hidden="true">↻</span> {t("attributionDialog.running", { i: current.index, n: report!.requested })}</>}
      </span>
      {running && <button className="btn" onClick={cancel}>{t("common.cancel")}</button>}
      <button className="btn" onClick={onClose}>{t("common.close")}</button>
      <button className="btn primary" disabled={!canStart} onClick={start}>{report ? t("attributionDialog.again") : t("attributionDialog.start")}</button>
    </>
  );

  return (
    // Once a run has started, a stray click on the backdrop must not cancel it or drop its result.
    <Modal label={t("attributionDialog.title")} title={t("attributionDialog.title")} onClose={onClose} wide foot={foot}
      dirty={running || !!report || !!runError}>
      <Seg value={view} label={t("attributionDialog.title")} onChange={setView} options={[
        { value: "test", label: t("attributionDialog.viewTest") },
        { value: "history", label: history?.length ? t("attributionDialog.viewHistoryCount", { n: history.length }) : t("attributionDialog.viewHistory") },
      ]} />
      {view === "test" ? (
        <>
          <TargetView target={target} error={targetError} provider={provider} model={model} match={match} bank={bank} names={names} />
          {blocked && <div className="ptest-res bad"><span>{scrub(blocked)}</span></div>}

          <div className="attr-runs">
            <span className="small">{t("attributionDialog.samples")}</span>
            <Seg className="sm" value={runs} label={t("attributionDialog.samples")} onChange={(v) => { if (!running) setRuns(v); }}
              options={RUN_CHOICES.map((n) => ({ value: n, label: String(n), disabled: running }))} />
            <span className="tiny muted hint grow">{t("attributionDialog.samplesHint")}</span>
          </div>
          <div className="attr-cost tiny">{tn("attributionDialog.cost", runs)}</div>

          {report && <LiveLog logs={logs} rows={report.rows} />}
          {report && <SummaryView s={summarize(report, match.candidate)} candidate={match.candidate} done={done} />}
          {runError && <div className="ptest-res bad"><strong>{t("attributionDialog.statusFailed")}</strong><span>{scrub(runError)}</span></div>}
          {historyError && done && <div className="tiny warn-text">{t("attributionDialog.historySaveFailed", { err: scrub(historyError) })}</div>}

          <details className="attr-source tiny muted">
            <summary>{t("attributionDialog.sourceLicense")}</summary>
            <p>{t("attributionDialog.source", { commit: BANK_SOURCE.commit.slice(0, 7), n: bank.models.length })}</p>
            <p className="mono">{BANK_SOURCE.repository} @ {BANK_SOURCE.commit}</p>
            <pre className="attr-license">{license}</pre>
          </details>
        </>
      ) : (
        <HistoryView entries={history} error={historyError} agent={agent} model={model} names={names}
          onChange={(l) => { setHistory(parseHistory(l)); setHistoryError(null); }} onError={setHistoryError} />
      )}
    </Modal>
  );
}

/** What the model sends back, as it streams in; reasoning is shown dimmed and never scored. */
function LiveLog({ logs, rows }: { logs: LogSample[]; rows: SampleRow[] }) {
  const box = useRef<HTMLDivElement>(null);
  /** Follow new text, unless the user scrolled up to read. */
  const follow = useRef(true);
  useLayoutEffect(() => {
    const el = box.current;
    if (el && follow.current) el.scrollTop = el.scrollHeight;
  }, [logs]);
  const shown = rows.filter((r) => r.status !== "pending" && r.status !== "skipped");
  return (
    <div className="stack6">
      <div className="row gap6">
        <span className="small">{t("attributionDialog.log")}</span>
        <span className="tiny muted hint">{t("attributionDialog.logHint")}</span>
      </div>
      <div ref={box} className="attr-log sensitive" role="log" aria-live="off"
        onScroll={(e) => { const el = e.currentTarget; follow.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24; }}>
        {shown.map((r) => {
          const l = logs.find((x) => x.index === r.index);
          return (
            <div key={r.index} className="attr-log-sample">
              <div className="attr-log-head">
                {t("attributionDialog.logSample", { i: r.index, n: r.expectedCount })}
                {l && l.requests > 1 && <> · {t("attributionDialog.logRequest", { n: l.requests })}</>}
                {l?.status != null && <> · HTTP {l.status}</>}
              </div>
              {l?.parts.map((p, i) => <span key={i} className={p.reasoning ? "attr-log-think" : undefined}>{p.text}</span>)}
              {!l?.parts.length && <span className="attr-log-wait">{r.status === "running" ? t("attributionDialog.rowRunning") : t("attributionDialog.logNothing")}</span>}
            </div>
          );
        })}
      </div>
    </div>
  );
}

function TargetView({ target, error, provider, model, match, bank, names }: {
  target: AttributionTarget | null; error: string | null; provider: string; model: string; match: Match; bank: Bank; names: Record<string, string>;
}) {
  const shown = target?.provider ?? (provider === "*" ? null : provider);
  const g = target?.gateway;
  const candidate = bank.models.find((m) => m.id === match.candidate);
  return (
    <div className="kv">
      <div className="kv-row">
        <span className="tiny muted">{t("common.provider")}</span>
        <span className="attr-lines">
          <span>{shown ? <>{names[shown] ?? shown} <span className="mono tiny muted">{shown}</span></> : "…"}</span>
          {provider === "*" && <span className="tiny muted">{t("attributionDialog.codexCurrent")}</span>}
          {target?.entry && <span className="tiny muted">{t("attributionDialog.configEntry", { entry: target.entry })}</span>}
        </span>
      </div>
      <div className="kv-row">
        <span className="tiny muted">{t("attributionDialog.endpoint")}</span>
        {error ? <span className="tiny warn-text">{t("attributionDialog.targetFailed", { err: scrub(error) })}</span>
          : !target ? <span className="tiny muted">{t("attributionDialog.resolving")}</span>
          : (
            <span className="attr-lines minw0">
              <span className="mono tiny">{target.api ? `${apiLabel(target.api)} · ` : ""}{scrub(target.url)}</span>
              {!target.hasKey && !target.blocked && <span className="tiny warn-text">{t("attributionDialog.noKey")}</span>}
            </span>
          )}
      </div>
      {g && (
        <div className="kv-row">
          <span className="tiny muted">{t("attributionDialog.gateway")}</span>
          <span className="attr-lines minw0">
            <span className="tiny">{t("attributionDialog.gatewayRoute", { route: g.routeName, upstream: g.upstreamName, url: scrub(g.upstreamUrl), api: apiLabel(g.upstreamApi) })}</span>
            {g.pinned && <span className="tiny muted">{t(g.entry === "unified" ? "attributionDialog.pinnedUnified" : "attributionDialog.pinnedCombined")}</span>}
            {g.skipped.length > 0 && <span className="tiny muted">{t("attributionDialog.skipped", { ids: g.skipped.join(t("common.listSep")) })}</span>}
            {g.mappedModel && <span className="tiny warn-text">{t("attributionDialog.mapped", { model: g.mappedModel })}</span>}
          </span>
        </div>
      )}
      <div className="kv-row">
        <span className="tiny muted">{t("attributionDialog.requestModel")}</span>
        <span className="mono tiny">{model}</span>
      </div>
      <div className="kv-row">
        <span className="tiny muted">{t("attributionDialog.comparedAs")}</span>
        <span className="attr-lines">
          <span className="mono tiny">{candidate?.display_name ?? match.candidate}</span>
          {match.via === "alias" && <span className="tiny muted">{t("attributionDialog.viaAlias", { id: match.candidate })}</span>}
          {match.via === "prefix" && <span className="tiny muted">{t("attributionDialog.viaPrefix", { id: match.candidate })}</span>}
          <span className="tiny muted">{tn("attributionDialog.wholeBank", bank.models.length)}</span>
        </span>
      </div>
    </div>
  );
}

function statusLine(s: ResultSummary): { tone: "good" | "attr-warn" | "bad"; text: string } {
  switch (outcome(s)) {
    case "complete": return { tone: "good", text: t("attributionDialog.statusComplete") };
    case "incomplete": return { tone: "attr-warn", text: tn("attributionDialog.statusIncomplete", s.requested, { valid: s.valid }) };
    case "cancelled": return s.scores
      ? { tone: "attr-warn", text: tn("attributionDialog.statusCancelled", s.requested, { valid: s.valid }) }
      : { tone: "bad", text: t("attributionDialog.statusCancelledNone") };
    case "failed": return { tone: "bad", text: t("attributionDialog.statusFailed") };
  }
}

/** A run's result: live while it runs, or as kept in the history. */
function SummaryView({ s, candidate, done }: { s: ResultSummary; candidate: string; done: boolean }) {
  const sc = s.scores;
  const status = done ? statusLine(s) : null;
  const best = sc?.top[0];
  return (
    <div className="stack6">
      {status && (
        <div className={`ptest-res ${status.tone}`}>
          <strong>{status.text}</strong>
          {s.cancelled && <span className="tiny">{t("attributionDialog.cancelledNote")}</span>}
          {!sc && <span className="tiny">{t("attributionDialog.errNoValid")}</span>}
        </div>
      )}
      {sc && sc.own && best && (
        <>
          <div className="attr-head">
            <div className="attr-stat">
              <span className="tiny muted">{t("attributionDialog.thisModel", { id: candidate })}</span>
              <strong className="attr-big">{pct(sc.own.probability)}</strong>
              <span className="tiny muted">{t("attributionDialog.rank", { rank: sc.own.rank, total: sc.candidates })}</span>
            </div>
            <div className="attr-stat">
              <span className="tiny muted">{t("attributionDialog.bestMatch")}</span>
              <strong className="attr-big">{pct(best.probability)}</strong>
              <span className="mono tiny">{best.displayName}</span>
            </div>
          </div>
          <div className="attr-top">
            <span className="tiny muted">{t("attributionDialog.topCandidates")}</span>
            {sc.top.map((r, i) => (
              <div key={r.model} className={`attr-cand${r.model === candidate ? " self" : ""}`}>
                <span className="tiny muted">{i + 1}</span>
                <span className="mono tiny ellipsis">{r.displayName}</span>
                <span className="attr-bar"><i style={{ width: `${Math.max(0, Math.min(1, r.probability)) * 100}%` }} /></span>
                <span className="mono tiny attr-pct">{pct(r.probability)}</span>
              </div>
            ))}
          </div>
          <div className="tiny muted">
            {t("attributionDialog.families")}{" "}
            {sc.families.map((f) => `${f.displayName} ${pct(f.probability)}`).join(t("common.listSep"))}
          </div>
        </>
      )}
      <div className="attr-rows">
        {s.rows.map((r) => <RowView key={r.index} r={r} />)}
      </div>
      <div className="tiny muted">
        {t(s.requestsExact ? "attributionDialog.stats" : "attributionDialog.statsAtLeast", { requests: s.requests, valid: s.valid, n: s.requested })}
        {sc && <> · {tn("attributionDialog.calibration", Number(sc.calibration.queries), { beta: sc.calibration.beta.toLocaleString(locale(), { maximumFractionDigits: 2 }) })}</>}
      </div>
      {sc && <div className="tiny attr-disclaimer">{t("attributionDialog.disclaimer")}</div>}
    </div>
  );
}

function RowView({ r }: { r: SampleRow }) {
  const text = (() => {
    switch (r.status) {
      case "pending": return t("attributionDialog.rowQueued");
      case "running": return t("attributionDialog.rowRunning");
      case "valid": return t("attributionDialog.rowValid", { parsed: r.parsedNumbers ?? 0 });
      case "tooShort": return t("attributionDialog.rowTooShort", { parsed: r.parsedNumbers ?? 0, min: r.minimumNumbers });
      case "discarded": return t("attributionDialog.rowDiscarded");
      case "skipped": return t("attributionDialog.rowSkipped");
      case "failed": return scrub(r.error) ?? r.kind ?? "";
    }
  })();
  const tone = r.status === "valid" ? "ok" : r.status === "failed" || r.status === "tooShort" ? "bad" : "";
  return (
    <div className={`attr-row ${tone}`}>
      <span className="mono tiny muted">#{r.index}</span>
      <span className="tiny muted">{t("attributionDialog.rowAsked", { n: r.expectedCount })}</span>
      <span className="tiny grow minw0">{text}</span>
      {r.requests > 1 && <span className="tiny muted">{tn("attributionDialog.rowRequests", r.requests)}</span>}
      {r.ms > 0 && <span className="mono tiny muted">{t("attributionDialog.seconds", { s: secs(r.ms) })}</span>}
    </div>
  );
}

const OUTCOME_KEY = {
  complete: "attributionDialog.statusComplete",
  incomplete: "attributionDialog.outcomeIncomplete",
  cancelled: "attributionDialog.outcomeCancelled",
  failed: "attributionDialog.statusFailed",
} as const;

/** Earlier results: this model's by default, or all. */
function HistoryView({ entries, error, agent, model, names, onChange, onError }: {
  entries: HistoryEntry[] | null; error: string | null; agent: string; model: string; names: Record<string, string>;
  onChange: (list: unknown[]) => void; onError: (e: string) => void;
}) {
  const [scope, setScope] = useState<"model" | "all">("model");
  const [open, setOpen] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const list = (entries ?? []).filter((e) => scope === "all" || (e.agent === agent && e.model === model));
  const remove = (ids: string[] | null) => {
    setBusy(true);
    api.attributionHistoryDelete(ids).then(onChange).catch((e) => onError(errText(e))).finally(() => setBusy(false));
  };
  const clearAll = async () => {
    if (await ask({ title: t("attributionDialog.clearTitle"), message: t("attributionDialog.clearMsg"), danger: true, confirmText: t("attributionDialog.clear") })) remove(null);
  };
  return (
    <div className="stack8">
      <div className="row gap6">
        <Seg className="sm" value={scope} label={t("attributionDialog.viewHistory")} onChange={setScope} options={[
          { value: "model", label: t("attributionDialog.scopeModel") },
          { value: "all", label: t("attributionDialog.scopeAll") },
        ]} />
        <span className="grow" />
        <button className="btn" disabled={busy || !entries?.length} onClick={() => { clearAll(); }}><Icon.trash size={12} />{t("attributionDialog.clear")}</button>
      </div>
      {error && <div className="tiny warn-text">{scrub(error)}</div>}
      {entries === null && !error && <div className="tiny muted">{t("common.reading")}</div>}
      {entries !== null && !list.length && <div className="empty tiny">{t(scope === "all" ? "attributionDialog.historyEmpty" : "attributionDialog.historyEmptyModel")}</div>}
      <div className="attr-hist">
        {list.map((e) => {
          const o = outcome(e);
          const best = e.scores?.top[0];
          const expanded = open === e.id;
          return (
            <div key={e.id} className={`attr-hist-item${expanded ? " open" : ""}`}>
              <div className="attr-hist-row" role="button" tabIndex={0} aria-expanded={expanded}
                onClick={() => setOpen(expanded ? null : e.id)} onKeyDown={(k) => { if (k.key === "Enter" || k.key === " ") { k.preventDefault(); setOpen(expanded ? null : e.id); } }}>
                <span className="mono tiny muted">{new Date(e.at).toLocaleString(locale())}</span>
                <span className="tiny ellipsis minw0">
                  {scope === "all" && <span className="mono">{e.model} · </span>}
                  {e.providerName ?? names[e.provider] ?? e.provider}
                </span>
                <span className={`attr-badge ${o}`}>{t(OUTCOME_KEY[o])}</span>
                <span className="tiny attr-hist-own">{e.scores?.own ? pct(e.scores.own.probability) : "—"}</span>
                <span className="tiny muted ellipsis minw0">{best ? t("attributionDialog.historyBest", { model: best.displayName, value: pct(best.probability) }) : ""}</span>
                <span className="mono tiny muted">{e.valid}/{e.requested}</span>
                <button className="icon-btn" aria-label={t("common.delete")} disabled={busy} onClick={(ev) => { ev.stopPropagation(); remove([e.id]); }}><Icon.trash size={12} /></button>
              </div>
              {expanded && (
                <div className="attr-hist-detail">
                  <div className="tiny muted attr-lines">
                    <span className="mono">{apiLabel(e.api)} · {scrub(e.url)}</span>
                    {e.forward && <span>{t("attributionDialog.historyForward", { route: e.forward })}</span>}
                    <span>{t("attributionDialog.historyCompared", { model: e.model, candidate: e.candidate, bank: e.bank.slice(0, 7) })}</span>
                  </div>
                  <SummaryView s={e} candidate={e.candidate} done />
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
