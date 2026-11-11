import { type CSSProperties, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { api, type UsageCell, type UsagePrice, type UsagePriceValue, type UsageReport, type UsageSource } from "../api";
import { locale, type TKey, t, tn, useLang } from "../i18n";
import { useLoad, usePopover } from "../hooks";
import { agentLabel } from "../services";
import { errText, type Flash } from "../util";
import { ErrorBox, Seg, SegMulti } from "./controls";
import { Icon } from "./icons";
import { Modal } from "./Modal";

type Range = "today" | "7d" | "30d" | "90d" | "all" | "custom";
/** A custom range: first and last day, both included (yyyy-mm-dd). */
type Days = { from: string; to: string };
type Metric = "tokens" | "cost";
type Group = "hour" | "day" | "month";

const RANGES: [Exclude<Range, "custom">, TKey][] = [
  ["today", "usagePage.today"],
  ["7d", "usagePage.days7"],
  ["30d", "usagePage.days30"],
  ["90d", "usagePage.days90"],
  ["all", "usagePage.all"],
];

const SOURCES: UsageSource[] = ["official", "thirdParty", "gateway"];
const SOURCE_NAME: Record<UsageSource, TKey> = {
  official: "usagePage.official",
  thirdParty: "usagePage.thirdParty",
  gateway: "usagePage.gateway",
};
const SOURCE_HINT: Record<UsageSource, TKey> = {
  official: "usagePage.officialHint",
  thirdParty: "usagePage.thirdPartyHint",
  gateway: "usagePage.gatewayHint",
};
const SOURCE_COLOR: Record<UsageSource, string> = { official: "var(--accent)", thirdParty: "var(--chart-b)", gateway: "var(--teal)" };

type Window = { from: number; to: number; bucket: "hour" | "day" };

const day = (s: string): Date | null => {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(s);
  if (!m) return null;
  const d = new Date(+m[1], +m[2] - 1, +m[3]);
  return d.getMonth() === +m[2] - 1 ? d : null;
};

/** yyyy-mm-dd of a local date. */
export const isoDay = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;

/** [from, to) for whole local days `from`…`to` (to included, cut at now); null when invalid. Up to two days in hours. */
export function customWindow(r: Days, now = new Date()): Window | null {
  const a = day(r.from), b = day(r.to);
  if (!a || !b || a > b) return null;
  const end = new Date(b);
  end.setDate(end.getDate() + 1);
  const from = Math.floor(a.getTime() / 1000);
  const to = Math.min(Math.floor(end.getTime() / 1000), Math.floor(now.getTime() / 1000) + 60);
  if (to <= from) return null;
  return { from, to, bucket: (end.getTime() - a.getTime()) / 86_400_000 <= 2 ? "hour" : "day" };
}

/** [from, to) in Unix seconds and the backend's bucket for a range, as of now. */
export function rangeWindow(r: Exclude<Range, "custom">, now = new Date()): Window {
  const to = Math.floor(now.getTime() / 1000) + 60;
  if (r === "all") return { from: 0, to, bucket: "day" };
  const d = new Date(now);
  d.setHours(0, 0, 0, 0);
  if (r !== "today") d.setDate(d.getDate() - (parseInt(r, 10) - 1));
  return { from: Math.floor(d.getTime() / 1000), to, bucket: r === "today" ? "hour" : "day" };
}

/** Start of the local hour / day / month holding `t` (Unix seconds). */
function groupStart(t: number, g: Group): number {
  const d = new Date(t * 1000);
  if (g === "hour") d.setMinutes(0, 0, 0);
  else {
    d.setHours(0, 0, 0, 0);
    if (g === "month") d.setDate(1);
  }
  return Math.floor(d.getTime() / 1000);
}

/** Every bucket start from `from` (or the first cell) up to `to`. */
export function bucketTimes(from: number, to: number, g: Group, cells: { t: number }[]): number[] {
  const first = from > 0 ? from : Math.min(to, ...cells.map((c) => c.t));
  if (first >= to) return [];
  const d =new Date(groupStart(first, g) * 1000);
  const out: number[] = [];
  while (d.getTime() / 1000 < to && out.length < 5000) {
    out.push(Math.floor(d.getTime() / 1000));
    if (g === "hour") d.setHours(d.getHours() + 1);
    else if (g === "day") d.setDate(d.getDate() + 1);
    else d.setMonth(d.getMonth() + 1);
  }
  return out;
}

interface Sum {
  requests: number;
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  cost: number;
  unpriced: number;
}

const zero = (): Sum => ({ requests: 0, input: 0, output: 0, cacheRead: 0, cacheWrite: 0, cost: 0, unpriced: 0 });

function add(s: Sum, c: Sum): Sum {
  s.requests += c.requests;
  s.input += c.input;
  s.output += c.output;
  s.cacheRead += c.cacheRead;
  s.cacheWrite += c.cacheWrite;
  s.cost += c.cost;
  s.unpriced += c.unpriced;
  return s;
}

export const totalTokens = (s: Sum) => s.input + s.output + s.cacheRead + s.cacheWrite;

/** Share of the prompt read from the cache; null without input. */
export function cacheRate(s: Sum): number | null {
  const prompt = s.input + s.cacheRead + s.cacheWrite;
  return prompt > 0 ? s.cacheRead / prompt : null;
}

function by<K extends string>(cells: UsageCell[], key: (c: UsageCell) => K): Map<K, Sum> {
  const m = new Map<K, Sum>();
  for (const c of cells) add(m.get(key(c)) ?? m.set(key(c), zero()).get(key(c))!, c);
  return m;
}

const fmtTokens = (n: number) => n.toLocaleString(locale(), { notation: "compact", maximumFractionDigits: n >= 1000 ? 1 : 0 });
const fmtInt = (n: number) => n.toLocaleString(locale());
const fmtPct = (r: number | null) => (r === null ? "–" : r.toLocaleString(locale(), { style: "percent", maximumFractionDigits: 1 }));

export function fmtCost(usd: number): string {
  if (usd > 0 && usd < 0.01) return `<${(0.01).toLocaleString(locale(), { style: "currency", currency: "USD" })}`;
  return usd.toLocaleString(locale(), { style: "currency", currency: "USD", maximumFractionDigits: usd >= 1000 ? 0 : 2 });
}

function fmtBucket(t: number, g: Group): string {
  const d = new Date(t * 1000);
  if (g === "hour") return d.toLocaleTimeString(locale(), { hour: "2-digit", minute: "2-digit" });
  if (g === "month") return d.toLocaleDateString(locale(), { year: "numeric", month: "short" });
  return d.toLocaleDateString(locale(), { month: "short", day: "numeric" });
}

/** A scan of the logs at most this old (seconds) is reused when the page opens or the range changes. */
const REUSE_SCAN = 600;
/** A scan older than this (seconds) is read again in the background after it is shown. */
const STALE_AFTER = 15;

/** The logs behind a report were read more than `STALE_AFTER` seconds ago. */
export function isStale(scannedAt: number, now = Date.now()): boolean {
  return now / 1000 - scannedAt > STALE_AFTER;
}

/** The agents among `visible` (the sidebar's, in its order) that have usage in the cells. */
export function agentsWithUsage(cells: { agent: string }[], visible: string[]): string[] {
  const used = new Set(cells.map((c) => c.agent));
  return visible.filter((a) => used.has(a));
}

/** `visible`: the agents the sidebar lists (detected, not hidden in Settings), in its order. */
export function UsagePage({ flash, visible }: { flash: Flash; visible: string[] }) {
  const lang = useLang();
  const [range, setRange] = useState<Range>("7d");
  const [days, setDays] = useState<Days>(() => {
    const d = new Date();
    const to = isoDay(d);
    d.setDate(d.getDate() - 6);
    return { from: isoDay(d), to };
  });
  const [agent, setAgent] = useState("all");
  const [sources, setSources] = useState<UsageSource[]>(SOURCES);
  const [metric, setMetric] = useState<Metric>("tokens");
  const [table, setTable] = useState<"agents" | "models">("models");
  const [pricesOpen, setPricesOpen] = useState(false);
  /** The price dialog: the model, or "" to add one. */
  const [pricing, setPricing] = useState<string | null>(null);
  // Requests in flight (a range switch can overlap the minute refresh): the page shows it loads.
  const [pending, setPending] = useState(0);
  // How old a scan of the logs may be for the next load (seconds), and whether that load runs
  // without showing it. Opening the page and switching the range take the last scan (instant,
  // even in WSL); `revalidate` then reads the logs again in the background.
  const want = useRef({ maxAge: REUSE_SCAN, quiet: false });
  const { data: report, error, reload } = useLoad(() => {
    const w = range === "custom" ? customWindow(days) ?? rangeWindow("7d") : rangeWindow(range);
    const { maxAge, quiet } = want.current;
    want.current = { maxAge: REUSE_SCAN, quiet: false };
    if (!quiet) setPending((n) => n + 1);
    return api.usageReport(w.from, w.to, w.bucket, maxAge).finally(() => { if (!quiet) setPending((n) => n - 1); });
  }, [range, days, lang]);
  const loading = pending > 0;
  const load = (maxAge: number, quiet: boolean) => {
    want.current = { maxAge, quiet };
    return reload();
  };
  const refresh = async () => {
    await load(0, false);
    flash(t("usagePage.refreshed"));
  };
  const { data: prices, reload: reloadPrices } = useLoad(() => api.usagePrices(), []);
  // Shown from an older scan: read the logs again, quietly (the numbers just update).
  useEffect(() => {
    if (report && isStale(report.scannedAt)) void load(0, true);
  }, [report?.scannedAt]);
  // Agents keep writing logs: refresh while the page is open.
  useEffect(() => {
    const id = window.setInterval(() => void load(0, true), 60_000);
    return () => window.clearInterval(id);
  }, [reload]);

  // Only the sidebar's agents count: an agent hidden there is hidden here too.
  const shownCells = useMemo(() => (report?.cells ?? []).filter((c) => visible.includes(c.agent)), [report, visible]);
  const agents = useMemo(() => agentsWithUsage(shownCells, visible), [shownCells, visible]);
  // The picked agent was hidden (or has no usage in this range): back to all.
  useEffect(() => { if (agent !== "all" && !agents.includes(agent)) setAgent("all"); }, [agent, agents]);
  const cells = useMemo(() => shownCells.filter((c) => (agent === "all" || c.agent === agent) && sources.includes(c.source)), [shownCells, agent, sources]);
  const scanned = useMemo(() => (report?.sources ?? []).filter((s) => visible.includes(s.agent)), [report, visible]);
  const total = useMemo(() => cells.reduce((s, c) => add(s, c), zero()), [cells]);
  const bySource = useMemo(() => by(cells, (c) => c.source), [cells]);
  const unpricedModels = useMemo(() => [...by(cells.filter((c) => c.unpriced > 0), (c) => c.model)].sort((a, b) => b[1].unpriced - a[1].unpriced), [cells]);

  const usedModels = useMemo(() => [...new Set(cells.map((c) => c.model))], [cells]);
  // Remounting the bars replays their entrance whenever what they show changes.
  // Keyed on the loaded report's start (not the clicked range), so the bars grow with the new data.
  // Filters don't remount: their changes animate the bars from the old heights.
  const barsKey = `${report?.from}|${report?.bucket}`;
  const priceChanged = () => {
    void reloadPrices();
    // New prices, same logs.
    void load(REUSE_SCAN, false);
  };

  return (
    <main className="page">
      <div className="page-top">
        <div className="page-head">
          <span className="page-icon"><Icon.chart size={20} /></span>
          <div className="page-title">
            <h1>{t("usagePage.title")}</h1>
            <span className="muted small hint">{t("usagePage.subtitle")}</span>
          </div>
          <RangeControl range={range} days={days} onRange={setRange} onDays={(d) => { setDays(d); setRange("custom"); }} />
          <button className="btn usage-refresh" disabled={loading} onClick={() => void refresh()}>
            <span className={loading ? "usage-spin" : "usage-spin-off"}><Icon.refresh size={13} /></span>{t("common.refresh")}
          </button>
        </div>
      </div>
      <div className={`page-body usage${loading && report ? " reloading" : ""}`} aria-busy={loading}>
        {error && <ErrorBox text={error} />}
        {report?.errors.map((e) => <ErrorBox key={e} text={e} />)}
        {!report && !error && <Skeleton />}
        {report && (
          <>
            <div className="usage-filters">
              <Seg value={agent} onChange={setAgent} label={t("usagePage.agent")}
                options={[{ value: "all", label: t("usagePage.allAgents") }, ...agents.map((a) => ({ value: a, label: agentLabel(a) }))]} />
              <SegMulti value={sources} onChange={setSources} label={t("usagePage.source")}
                options={SOURCES.map((s) => ({ value: s, label: t(SOURCE_NAME[s]), title: t(SOURCE_HINT[s]) }))} />
            </div>

            <div className="usage-tiles">
              <Tile label={t("usagePage.tokens")} value={fmtTokens(totalTokens(total))} sub={t("usagePage.inOut", { input: fmtTokens(total.input + total.cacheRead + total.cacheWrite), output: fmtTokens(total.output) })} />
              <Tile label={t("usagePage.requests")} value={fmtInt(total.requests)} sub={t("usagePage.perRequest", { n: fmtTokens(total.requests ? totalTokens(total) / total.requests : 0) })} />
              <Tile label={t("usagePage.cost")} value={fmtCost(total.cost)} sub={total.unpriced ? tn("usagePage.unpricedReq", total.unpriced) : t("usagePage.costHint")} warn={total.unpriced > 0} />
              <Tile label={t("usagePage.cacheRate")} value={fmtPct(cacheRate(total))} sub={t("usagePage.cacheRead", { n: fmtTokens(total.cacheRead) })} />
            </div>

            <SourceBar bySource={bySource} />
            <Dedup report={report} />

            <section className="card usage-card">
              <div className="usage-card-head">
                <h2>{t("usagePage.overTime")}</h2>
                <Seg value={metric} onChange={setMetric} label={t("usagePage.metric")} options={[{ value: "tokens", label: t("usagePage.tokens") }, { value: "cost", label: t("usagePage.cost") }]} />
              </div>
              <Bars key={barsKey} report={report} cells={cells} metric={metric} range={range} />
            </section>

            <section className="card usage-card">
              <div className="usage-card-head">
                <Seg value={table} onChange={setTable} label={t("usagePage.breakdown")} options={[{ value: "models", label: t("usagePage.byModel") }, { value: "agents", label: t("usagePage.byAgent") }]} />
              </div>
              <Breakdown cells={cells} kind={table} onPrice={setPricing} />
            </section>

            {unpricedModels.length > 0 && (
              <div className="notes">
                <span>{t("usagePage.unpricedNote")}</span>
                <div className="usage-chips">
                  {unpricedModels.slice(0, 12).map(([m, s]) => (
                    <button key={m} className="btn xs" onClick={() => setPricing(m)}>{t("usagePage.setPriceFor", { model: m, n: fmtInt(s.unpriced) })}</button>
                  ))}
                </div>
              </div>
            )}

            <section className="card usage-card">
              <button className="usage-fold" aria-expanded={pricesOpen} onClick={() => setPricesOpen(!pricesOpen)}>
                <span className="usage-h2">{t("usagePage.prices")}</span>
                <span className="muted small">{t("usagePage.pricesHint")}</span>
                <span className={`usage-chev${pricesOpen ? " open" : ""}`}><Icon.chevron /></span>
              </button>
              {pricesOpen && <Prices flash={flash} prices={prices} used={usedModels} onEdit={setPricing} onChanged={priceChanged} />}
            </section>

            <p className="muted tiny">
              {scanned.length
                ? t("usagePage.scanned", { list: scanned.map((s) => `${agentLabel(s.agent)} ${tn("usagePage.files", s.files)}`).join(t("common.listSep")) })
                : t("usagePage.noLogs")}
            </p>
          </>
        )}
      </div>
      {pricing !== null && (
        <PriceDialog model={pricing} entry={findPrice(prices?.entries ?? [], pricing)} flash={flash}
          onClose={() => setPricing(null)} onSaved={() => { setPricing(null); priceChanged(); }} />
      )}
    </main>
  );
}

function fmtDay(s: string): string {
  const d = day(s);
  return d ? d.toLocaleDateString(locale(), { month: "short", day: "numeric" }) : s;
}

/** The presets and "Custom" as one segmented control (the glider slides onto Custom too), and the
 *  custom popover (a `.dd-menu`, so it enters and leaves like every menu). */
function RangeControl({ range, days, onRange, onDays }: { range: Range; days: Days; onRange: (r: Range) => void; onDays: (d: Days) => void }) {
  const active = range === "custom";
  const onPick = onDays;
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(days);
  const root = useRef<HTMLDivElement>(null);
  usePopover(open, () => setOpen(false), [root]);
  const today = isoDay(new Date());
  const ok = customWindow(draft) !== null;
  const apply = () => {
    if (!ok) return;
    onPick(draft);
    setOpen(false);
  };
  return (
    <div className="usage-range" ref={root}>
      <div className="seg" role="radiogroup" aria-label={t("usagePage.range")}>
        {RANGES.map(([v, k]) => (
          <button key={v} type="button" role="radio" aria-checked={range === v} className={range === v ? "on" : ""} onClick={() => { setOpen(false); onRange(v); }}>{t(k)}</button>
        ))}
        <button type="button" role="radio" aria-checked={active} className={active ? "on" : ""} aria-haspopup="dialog" aria-expanded={open}
          onClick={() => { setDraft(days); setOpen((o) => !o); }}>
          <Icon.calendar size={13} />
          {active ? t("usagePage.dayRange", { from: fmtDay(days.from), to: fmtDay(days.to) }) : t("usagePage.custom")}
        </button>
      </div>
      {open && (
        <div className="dd-menu usage-range-pop" role="dialog" aria-label={t("usagePage.customRange")}
          onKeyDown={(e) => { if (e.key === "Enter" && !(e.target instanceof HTMLButtonElement)) { e.preventDefault(); apply(); } }}>
          <label className="field">
            <span>{t("usagePage.fromDay")}</span>
            <input type="date" className="input" value={draft.from} max={draft.to || today} autoFocus onChange={(e) => setDraft({ ...draft, from: e.target.value })} />
          </label>
          <label className="field">
            <span>{t("usagePage.toDay")}</span>
            <input type="date" className="input" value={draft.to} min={draft.from} max={today} onChange={(e) => setDraft({ ...draft, to: e.target.value })} />
          </label>
          {!ok && <span className="field-err">{t("usagePage.badRange")}</span>}
          <div className="usage-range-foot">
            <button type="button" className="btn small" onClick={() => setOpen(false)}>{t("common.cancel")}</button>
            <button type="button" className="btn small primary" disabled={!ok} onClick={apply}>{t("usagePage.apply")}</button>
          </div>
        </div>
      )}
    </div>
  );
}

/** The page's shape while the first report loads. */
function Skeleton() {
  return (
    <div className="usage-skeleton" aria-label={t("usagePage.loading")} role="status">
      <div className="usage-filters">
        <span className="sk sk-pill" style={{ width: 300 }} />
        <span className="sk sk-pill" style={{ width: 170 }} />
      </div>
      <div className="usage-tiles">
        {[0, 1, 2, 3].map((i) => (
          <div key={i} className="usage-tile">
            <span className="sk sk-text" style={{ width: 44 }} />
            <span className="sk sk-num" style={{ width: `${[52, 44, 60, 40][i]}%` }} />
            <span className="sk sk-text" style={{ width: `${[70, 58, 64, 54][i]}%` }} />
          </div>
        ))}
      </div>
      <div className="usage-sources">
        <span className="sk sk-bar" />
        <div className="usage-legend">{[120, 140, 110].map((w) => <span key={w} className="sk sk-text" style={{ width: w }} />)}</div>
      </div>
      <div className="card usage-card">
        <div className="usage-card-head">
          <span className="sk sk-text" style={{ width: 56, height: 14 }} />
          <span className="sk sk-pill" style={{ width: 120 }} />
        </div>
        <div className="sk-bars">
          {[42, 66, 34, 78, 56, 90, 48, 70, 38, 60, 84, 52].map((h, i) => <span key={i} style={{ height: `${h}%`, "--i": i } as CSSProperties} />)}
        </div>
      </div>
      <div className="card usage-card">
        {[0, 1, 2, 3].map((i) => (
          <div key={i} className="sk-row">
            <span className="sk sk-text" style={{ width: `${[26, 20, 16, 22][i]}%` }} />
            <span className="sk sk-text" style={{ width: "34%" }} />
          </div>
        ))}
      </div>
    </div>
  );
}

function Tile({ label, value, sub, warn }: { label: string; value: string; sub?: string; warn?: boolean }) {
  return (
    <div className="usage-tile">
      <span className="muted small">{label}</span>
      <span className="usage-tile-n">{value}</span>
      {sub && <span className={`tiny ${warn ? "usage-warn" : "muted"}`}>{sub}</span>}
    </div>
  );
}

function SourceBar({ bySource }: { bySource: Map<UsageSource, Sum> }) {
  const all = SOURCES.reduce((n, s) => n + totalTokens(bySource.get(s) ?? zero()), 0);
  return (
    <div className="usage-sources">
      <div className="usage-stack" role="img" aria-label={t("usagePage.sourceShare")}>
        {SOURCES.map((s) => {
          // Every segment stays mounted (0 % wide when empty), so a filter change glides instead of popping.
          const share = all ? totalTokens(bySource.get(s) ?? zero()) / all : 0;
          return <span key={s} className={share > 0 ? "on" : undefined} style={{ width: `${share * 100}%`, background: SOURCE_COLOR[s] }} />;
        })}
      </div>
      <div className="usage-legend">
        {SOURCES.map((s) => {
          const v = bySource.get(s) ?? zero();
          return (
            <div key={s} className="usage-legend-item" title={t(SOURCE_HINT[s])}>
              <span className="usage-sw" style={{ background: SOURCE_COLOR[s] }} />
              <span className="strong">{t(SOURCE_NAME[s])}</span>
              <span className="muted">{fmtTokens(totalTokens(v))} · {fmtCost(v.cost)} · {fmtPct(all ? totalTokens(v) / all : null)}</span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function Dedup({ report }: { report: UsageReport }) {
  const since = report.ledgerSince ? new Date(report.ledgerSince * 1000).toLocaleDateString(locale(), { year: "numeric", month: "short", day: "numeric" }) : null;
  return (
    <div className="notes">
      <span>{since ? t("usagePage.ledgerSince", { date: since }) : t("usagePage.ledgerNone")}</span>
      {report.dedupedRequests > 0 && <span>{tn("usagePage.deduped", report.dedupedRequests, { tokens: fmtTokens(report.dedupedTokens) })}</span>}
      {report.repeatedRequests > 0 && <span>{tn("usagePage.repeated", report.repeatedRequests)}</span>}
    </div>
  );
}

const H = 180;
/** Equal side padding keeps the plot centred in its card; axis labels sit inside, above their line. */
const PAD = { top: 16, right: 4, bottom: 22, left: 4 };

function Bars({ report, cells, metric, range }: { report: UsageReport; cells: UsageCell[]; metric: Metric; range: Range }) {
  const box = useRef<HTMLDivElement>(null);
  const [w, setW] = useState(600);
  const [hover, setHover] = useState<number | null>(null);
  const tip = useRef<HTMLDivElement>(null);
  const [tipW, setTipW] = useState(0);
  // Measured after each move: the text (and so the width) changes from bar to bar.
  useLayoutEffect(() => setTipW(tip.current?.offsetWidth ?? 0), [hover]);
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setW(el.clientWidth));
    ro.observe(el);
    setW(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  const group: Group = report.bucket === "hour" ? "hour" : range === "all" && cells.length && (report.to - Math.min(...cells.map((c) => c.t))) > 120 * 86400 ? "month" : "day";
  const times = useMemo(() => bucketTimes(report.from, report.to, group, cells), [report, cells, group]);
  const index = useMemo(() => new Map(times.map((tm, i) => [tm, i])), [times]);
  const stacks = useMemo(() => {
    const out = times.map(() => ({ official: 0, thirdParty: 0, gateway: 0 } as Record<UsageSource, number>));
    for (const c of cells) {
      const i = index.get(groupStart(c.t, group));
      if (i !== undefined) out[i][c.source] += metric === "cost" ? c.cost : totalTokens(c);
    }
    return out;
  }, [times, index, cells, group, metric]);

  const fmt = metric === "cost" ? fmtCost : fmtTokens;
  const peak = Math.max(0, ...stacks.map((s) => s.official + s.thirdParty + s.gateway));
  const max = niceMax(peak);
  const iw = Math.max(10, w - PAD.left - PAD.right);
  const ih = H - PAD.top - PAD.bottom;
  const n = Math.max(1, times.length);
  const slot = iw / n;
  const bw = Math.max(1, Math.min(28, slot * 0.7));
  const y = (v: number) => PAD.top + ih - (max ? (v / max) * ih : 0);
  const labelEvery = Math.max(1, Math.ceil(n / Math.max(2, Math.floor(iw / 70))));

  const hv = hover === null ? null : stacks[hover];
  // The wrapper stays mounted when there is no data, so the width is always measured.
  if (!cells.length) return <div className="usage-chart" ref={box}><div className="usage-empty muted small">{t("usagePage.empty")}</div></div>;
  return (
    <div className="usage-chart" ref={box} onMouseLeave={() => setHover(null)}>
      <svg width={w} height={H} role="img" aria-label={t("usagePage.chartLabel", { n: times.length })} style={{ "--base": y(0) } as CSSProperties}
        onMouseMove={(e) => {
          const x = e.clientX - e.currentTarget.getBoundingClientRect().left - PAD.left;
          const i = Math.floor(x / slot);
          setHover(i >= 0 && i < n ? i : null);
        }}>
        {[0, 0.5, 1].map((f) => (
          <g key={f}>
            <line className={f ? "gwc-grid" : "gwc-base"} x1={PAD.left} x2={PAD.left + iw} y1={y(max * f)} y2={y(max * f)} />
          </g>
        ))}
        {stacks.map((s, i) => {
          let acc = 0;
          const x = PAD.left + i * slot + (slot - bw) / 2;
          return (
            <g key={times[i]} className="usage-bar" style={{ "--i": Math.min(i, 40) } as CSSProperties} opacity={hover === null || hover === i ? 1 : 0.55}>
              {SOURCES.map((src) => {
                const v = s[src];
                const top = y(acc + v), bottom = y(acc);
                acc += v;
                // A unit-high rect moved and scaled by CSS variables: a filter change transitions the
                // transform, so segments slide and stretch to their new place (an empty one is 0 high).
                return <rect key={src} className="usage-seg" x={x} y={0} width={bw} height={1} fill={SOURCE_COLOR[src]}
                  style={{ "--y": top, "--h": v > 0 ? Math.max(0.5, bottom - top) : 0 } as CSSProperties} />;
              })}
            </g>
          );
        })}
        {[0.5, 1].map((f) => <text key={f} className="gwc-axis usage-axis" x={PAD.left} y={y(max * f) - 4}>{fmt(max * f)}</text>)}
        {times.map((tm, i) => (i % labelEvery === 0 ? (
          <text key={tm} className="gwc-axis" x={PAD.left + i * slot + slot / 2} y={H - 6} textAnchor="middle">{fmtBucket(tm, group)}</text>
        ) : null))}
      </svg>
      {hv && hover !== null && (
        <div className="gwc-tip usage-tip" ref={tip} style={{ left: tipLeft(PAD.left + hover * slot + slot / 2, tipW, w) }}>
          <b>{fmtBucket(times[hover], group)} · {fmt(hv.official + hv.thirdParty + hv.gateway)}</b>
          {SOURCES.filter((s) => hv[s] > 0).map((s) => (
            <span key={s} className="gwc-tip-row"><span className="usage-sw" style={{ background: SOURCE_COLOR[s] }} />{fmt(hv[s])}</span>
          ))}
        </div>
      )}
    </div>
  );
}

/** Left edge of a tooltip `width` wide centred on `x`, kept inside [0, chart]. */
export function tipLeft(x: number, width: number, chart: number): number {
  return Math.max(0, Math.min(x - width / 2, chart - width));
}

/** A round axis top at or above `v`: 1, 2 or 5 times a power of ten. */
export function niceMax(v: number): number {
  if (v <= 0) return 0;
  const p = 10 ** Math.floor(Math.log10(v));
  return [1, 2, 5, 10].map((m) => m * p).find((m) => m >= v)!;
}

function Breakdown({ cells, kind, onPrice }: { cells: UsageCell[]; kind: "agents" | "models"; onPrice: (model: string) => void }) {
  const rows = useMemo(() => {
    const m = by(cells, (c) => (kind === "agents" ? c.agent : c.model));
    const agentsOf = new Map<string, Set<string>>();
    if (kind === "models") for (const c of cells) (agentsOf.get(c.model) ?? agentsOf.set(c.model, new Set()).get(c.model)!).add(c.agent);
    return [...m].map(([k, s]) => ({ k, s, agents: [...(agentsOf.get(k) ?? [])] })).sort((a, b) => totalTokens(b.s) - totalTokens(a.s));
  }, [cells, kind]);
  if (!rows.length) return <div className="usage-empty muted small">{t("usagePage.empty")}</div>;
  return (
    <div className="usage-table-wrap">
      <table className="usage-table">
        <thead>
          <tr>
            <th>{t(kind === "agents" ? "usagePage.agent" : "usagePage.model")}</th>
            <th>{t("usagePage.requests")}</th>
            <th>{t("usagePage.input")}</th>
            <th>{t("usagePage.output")}</th>
            <th>{t("usagePage.cacheReadCol")}</th>
            <th>{t("usagePage.cacheWriteCol")}</th>
            <th>{t("usagePage.tokens")}</th>
            <th>{t("usagePage.cost")}</th>
          </tr>
        </thead>
        <tbody>
          {rows.map(({ k, s, agents }) => (
            <tr key={k}>
              <td>
                <span className={kind === "models" ? "mono" : "strong"}>{kind === "agents" ? agentLabel(k) : k}</span>
                {kind === "models" && <span className="muted tiny usage-sub">{agents.map(agentLabel).join(t("common.listSep"))}</span>}
              </td>
              <td>{fmtInt(s.requests)}</td>
              <td>{fmtTokens(s.input)}</td>
              <td>{fmtTokens(s.output)}</td>
              <td>{fmtTokens(s.cacheRead)}</td>
              <td>{fmtTokens(s.cacheWrite)}</td>
              <td className="strong">{fmtTokens(totalTokens(s))}</td>
              <td>
                {s.cost === 0 && s.unpriced > 0 ? "–" : fmtCost(s.cost)}
                {kind === "models" && s.unpriced > 0 && <button className="usage-link" onClick={() => onPrice(k)}>{t("usagePage.setPrice")}</button>}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

const PRICE_FIELDS: [keyof UsagePriceValue, TKey][] = [
  ["input", "usagePage.input"],
  ["output", "usagePage.output"],
  ["cacheRead", "usagePage.cacheReadCol"],
  ["cacheWrite", "usagePage.cacheWriteCol"],
];

type Values = Record<keyof UsagePriceValue, string>;

const fieldOk = (s: string) => s.trim() === "" || (Number.isFinite(Number(s)) && Number(s) >= 0);

/** The fields as prices, or null while one is not a number ≥ 0 (empty counts as 0). */
export function parseDraft(values: Values): UsagePriceValue | null {
  const out = {} as UsagePriceValue;
  for (const [k] of PRICE_FIELDS) {
    if (!fieldOk(values[k])) return null;
    out[k] = Number(values[k].trim() || 0);
  }
  return out;
}

/** The model id the way the backend keys prices: lower case, no vendor prefix, context tag or date. */
export function normalizeModel(model: string): string {
  let m = model.trim().toLowerCase();
  m = m.slice(m.lastIndexOf("/") + 1);
  if (m.includes("[")) m = m.slice(0, m.indexOf("["));
  return m.replace(/[-@]20\d{6}$/, "").replace(/-20\d{2}-\d{2}-\d{2}$/, "").trim();
}

/** The price the backend bills `model` at: its own entry, else the longest entry it extends by a
 *  non-version suffix (`gpt-5.1-codex` → `gpt-5.1`, never `claude-opus-5-7` → `claude-opus-5`). */
export function findPrice(entries: UsagePrice[], model: string): UsagePrice | null {
  const k = normalizeModel(model);
  let best: UsagePrice | null = null;
  for (const e of entries) {
    if (e.model === k) return e;
    if (k.startsWith(`${e.model}-`) && !/\d/.test(k.charAt(e.model.length + 1)) && (!best || e.model.length > best.model.length)) best = e;
  }
  return best;
}

const ORIGIN: Record<UsagePrice["origin"], TKey> = { builtin: "usagePage.originBuiltin", modelsDev: "usagePage.originModelsDev", custom: "usagePage.originCustom" };

const fmtPrice = (n: number) => (n ? `$${n.toLocaleString(locale(), { maximumFractionDigits: 4 })}` : "–");

/** The price table: models used in the range plus custom ones; a search covers every known price. */
function Prices({ flash, prices, used, onEdit, onChanged }: {
  flash: Flash; prices: { entries: UsagePrice[]; modelsDevAt: number | null } | null; used: string[]; onEdit: (model: string) => void; onChanged: () => void;
}) {
  const [filter, setFilter] = useState("");
  const [fetching, setFetching] = useState(false);
  const entries = prices?.entries ?? [];
  const rows = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (q) return entries.filter((e) => e.model.includes(q)).slice(0, 100).map((e) => ({ model: e.model, e }));
    const out = new Map<string, UsagePrice | null>();
    for (const m of used) out.set(normalizeModel(m), findPrice(entries, m));
    for (const e of entries) if (e.origin === "custom") out.set(e.model, e);
    return [...out].sort(([a], [b]) => a.localeCompare(b)).map(([model, e]) => ({ model, e }));
  }, [entries, used, filter]);

  const fetchPrices = async () => {
    setFetching(true);
    try {
      const n = await api.usageFetchPrices();
      flash(tn("usagePage.fetched", n));
      onChanged();
    } catch (e) {
      flash(errText(e), true);
    } finally {
      setFetching(false);
    }
  };

  return (
    <div className="usage-prices">
      <div className="usage-prices-bar">
        <input className="input small-input" value={filter} placeholder={t("usagePage.filterModels", { n: fmtInt(entries.length) })} onChange={(e) => setFilter(e.target.value)} />
        <span className="muted tiny grow">
          {prices?.modelsDevAt ? t("usagePage.modelsDevAt", { date: new Date(prices.modelsDevAt * 1000).toLocaleString(locale(), { dateStyle: "medium", timeStyle: "short" }) }) : t("usagePage.modelsDevNever")}
        </span>
        <button className="btn small" disabled={fetching} onClick={fetchPrices}>
          <span className={fetching ? "usage-spin" : undefined}><Icon.refresh size={12} /></span>{t(fetching ? "usagePage.fetching" : "usagePage.fetchModelsDev")}
        </button>
        <button className="btn small" onClick={() => onEdit("")}><Icon.plus size={12} />{t("usagePage.addModel")}</button>
      </div>
      <div className="usage-table-wrap">
        <table className="usage-table">
          <thead>
            <tr>
              <th>{t("usagePage.model")}</th>
              {PRICE_FIELDS.map(([k, label]) => <th key={k}>{t(label)}</th>)}
              <th />
            </tr>
          </thead>
          <tbody>
            {rows.map(({ model, e }) => (
              <tr key={model}>
                <td>
                  <span className="mono">{model}</span>
                  <span className={e ? "chip-muted usage-tag" : "chip-muted usage-tag usage-warn"}>{e ? t(e.origin === "custom" && !e.hasDefault ? "usagePage.added" : ORIGIN[e.origin]) : t("usagePage.noPrice")}</span>
                </td>
                {PRICE_FIELDS.map(([k]) => <td key={k}>{e ? fmtPrice(e[k]) : "–"}</td>)}
                <td className="usage-actions">
                  <button className="btn xs" onClick={() => onEdit(model)}><Icon.edit size={12} />{t(e ? "common.edit" : "usagePage.setPrice")}</button>
                </td>
              </tr>
            ))}
            {!rows.length && <tr><td colSpan={6} className="usage-empty muted small">{t("usagePage.noPrices")}</td></tr>}
          </tbody>
        </table>
      </div>
    </div>
  );
}

/** Add or change one model's price; a custom price can go back to the list price (or be removed). */
function PriceDialog({ model, entry, flash, onClose, onSaved }: { model: string; entry: UsagePrice | null; flash: Flash; onClose: () => void; onSaved: () => void }) {
  const isNew = model === "";
  const [id, setId] = useState(normalizeModel(model));
  const [values, setValues] = useState<Values>(() => {
    const v = (n: number | undefined) => (n === undefined ? "" : String(n));
    return { input: v(entry?.input), output: v(entry?.output), cacheRead: v(entry?.cacheRead), cacheWrite: v(entry?.cacheWrite) };
  });
  const [busy, setBusy] = useState(false);
  const parsed = parseDraft(values);
  const can = !!parsed && !!id.trim() && !busy;

  const run = async (price: UsagePriceValue | null) => {
    setBusy(true);
    try {
      await api.usageSetPrice(id.trim(), price);
      flash(t(price ? "usagePage.priceSaved" : "usagePage.priceReset", { model: id.trim() }));
      onSaved();
    } catch (e) {
      flash(errText(e), true);
      setBusy(false);
    }
  };

  const title = isNew ? t("usagePage.addModel") : t("usagePage.priceOf", { model: id });
  const custom = entry?.origin === "custom";
  return (
    <Modal label={title} title={title} onClose={onClose} busy={busy}
      onKeyDown={(e) => { if (e.key === "Enter" && can && parsed && !(e.target instanceof HTMLButtonElement)) { e.preventDefault(); void run(parsed); } }}
      foot={(
        <>
          {custom && <button className="btn" disabled={busy} onClick={() => run(null)}>{t(entry.hasDefault ? "usagePage.reset" : "common.delete")}</button>}
          <span className="grow" />
          <button className="btn" disabled={busy} onClick={onClose}>{t("common.cancel")}</button>
          <button className="btn primary" disabled={!can} onClick={() => parsed && run(parsed)}>{t("common.save")}</button>
        </>
      )}>
      <div className="usage-price-form">
        {isNew && (
          <label className="field usage-price-model">
            <span>{t("usagePage.modelId")}</span>
            <input className="input mono" autoFocus value={id} placeholder="glm-5" onChange={(e) => setId(e.target.value)} />
          </label>
        )}
        {PRICE_FIELDS.map(([k, label], i) => (
          <label key={k} className="field">
            <span>{t(label)}</span>
            <span className="usage-money">
              <span className="muted">$</span>
              <input className={`input mono${fieldOk(values[k]) ? "" : " bad"}`} inputMode="decimal" placeholder="0" autoFocus={!isNew && i === 0}
                value={values[k]} onChange={(e) => setValues({ ...values, [k]: e.target.value })} />
            </span>
          </label>
        ))}
      </div>
      <p className="muted tiny">
        {t("usagePage.priceUnit")}
        {entry && entry.origin !== "custom" ? ` ${t("usagePage.priceFrom", { origin: t(ORIGIN[entry.origin]) })}` : ""}
      </p>
    </Modal>
  );
}
