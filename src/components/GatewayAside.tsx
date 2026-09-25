import { useLayoutEffect, useMemo, useRef, useState } from "react";
import type { AgentId, AgentState, GatewayAgentUse, GatewayMinute, GatewayStatus } from "../api";
import { type TKey, t, tn } from "../i18n";
import { AGENT_NAME, agentLabel } from "../services";
import { AgentIcon, Icon } from "./icons";
import { Seg } from "./controls";

type Range = 15 | 60;

/** Series colour slot: "a" accent, "b" second categorical hue, "bad" failures. */
type Tone = "a" | "b" | "bad";

interface Series {
  /** Legend name (null for single-series charts). */
  name: TKey | null;
  tone: Tone;
  /** Value for one minute; null = nothing to show (e.g. latency without requests). */
  pick: (m: GatewayMinute | undefined) => number | null;
  /** Total / summary for the whole range. */
  total: (ms: GatewayMinute[]) => string;
  /** Tooltip format when it differs from the chart's. */
  fmt?: (v: number) => string;
}

interface Metric {
  key: string;
  title: TKey;
  unit: TKey;
  series: Series[];
  /** Headline for single-series charts (several series show a legend with totals instead). */
  headline?: (ms: GatewayMinute[]) => string;
  fmt: (v: number) => string;
  /** Y-axis label format, when it differs from `fmt` (no unit words). */
  axis?: (v: number) => string;
  /** Log y-axis, for series of very different size (input vs output tokens). */
  log?: boolean;
}

const sum = (ms: GatewayMinute[], f: (m: GatewayMinute) => number) => ms.reduce((n, m) => n + f(m), 0);

function tokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(n >= 10_000 ? 0 : 1)}K`;
  return String(Math.round(n));
}

function secs(ms: number): string {
  return ms >= 10_000 ? `${Math.round(ms / 1000)}s` : `${(ms / 1000).toFixed(1)}s`;
}

const one = (s: Omit<Series, "name" | "tone"> & { tone?: Tone }): Series[] => [{ name: null, tone: "a", ...s }];

const int = (v: number) => String(Math.round(v));

// Text is kept as keys / formatters here and translated at render time.
const METRICS: Metric[] = [
  {
    key: "req", title: "gatewayAside.reqTitle", unit: "gatewayAside.perMinute",
    series: [
      { name: "gatewayAside.requests", tone: "a", pick: (m) => m?.requests ?? 0, total: (ms) => tn("gatewayAside.reqCount", sum(ms, (m) => m.requests)) },
      {
        name: "gatewayAside.peakActive", tone: "b", pick: (m) => m?.peakActive ?? 0,
        total: (ms) => tn("gatewayAside.peakCount", ms.reduce((n, m) => Math.max(n, m.peakActive), 0)),
        fmt: (v) => tn("gatewayAside.peakCount", Math.round(v)),
      },
    ],
    fmt: (v) => tn("gatewayAside.reqCount", Math.round(v)),
    axis: int,
  },
  {
    key: "lat", title: "gatewayAside.latTitle", unit: "gatewayAside.seconds",
    series: one({
      pick: (m) => (m && m.requests ? m.msTotal / m.requests : null),
      total: (ms) => {
        const n = sum(ms, (m) => m.requests);
        return n ? secs(sum(ms, (m) => m.msTotal) / n) : "—";
      },
    }),
    fmt: secs,
  },
  {
    key: "tok", title: "gatewayAside.tokTitle", unit: "gatewayAside.perMinute",
    series: [
      { name: "gatewayAside.input", tone: "a", pick: (m) => m?.inputTokens ?? 0, total: (ms) => tokens(sum(ms, (m) => m.inputTokens)) },
      { name: "gatewayAside.output", tone: "b", pick: (m) => m?.outputTokens ?? 0, total: (ms) => tokens(sum(ms, (m) => m.outputTokens)) },
    ],
    fmt: tokens,
    log: true,
  },
  {
    key: "fail", title: "gatewayAside.failTitle", unit: "gatewayAside.timesPerMinute",
    series: one({ tone: "bad", pick: (m) => m?.failures ?? 0, total: (ms) => tn("gatewayAside.failCount", sum(ms, (m) => m.failures)) }),
    headline: (ms) => {
      const n = sum(ms, (m) => m.requests);
      const f = sum(ms, (m) => m.failures);
      return n ? t("gatewayAside.successRate", { pct: (((n - f) / n) * 100).toFixed(f && f < n ? 1 : 0) }) : "—";
    },
    fmt: (v) => tn("gatewayAside.failCount", Math.round(v)),
    axis: int,
  },
];

function hhmm(secs: number): string {
  const d = new Date(secs * 1000);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** Smallest 1 / 2 / 5 × 10ⁿ at or above v. */
function niceMax(v: number): number {
  if (v <= 0) return 1;
  const p = 10 ** Math.floor(Math.log10(v));
  return [1, 2, 5, 10].map((k) => k * p).find((x) => x >= v)!;
}

/** Gateway page right column: per-minute traffic charts for the last 15 / 60 minutes. */
export function GatewayAside({ status: s, agents }: { status: GatewayStatus | null; agents: AgentState[] }) {
  const [range, setRange] = useState<Range>(60);
  const { times, byT, inRange } = useMemo(() => {
    const end = Math.floor((s?.now ?? Date.now() / 1000) / 60) * 60;
    const times = Array.from({ length: range }, (_, i) => end - (range - 1 - i) * 60);
    const byT = new Map((s?.series ?? []).map((m) => [m.t, m]));
    return { times, byT, inRange: (s?.series ?? []).filter((m) => m.t >= times[0]) };
  }, [s?.now, s?.series, range]);
  const quiet = inRange.length === 0;

  return (
    <aside className="aside" aria-label={t("gatewayAside.ariaTraffic")}>
      <section className="aside-cur">
        <div className="row between">
          <h2>{t("gatewayAside.traffic")}</h2>
          <Seg className="gwc-range" value={range} onChange={setRange} label={t("gatewayAside.ariaRange")}
            options={([15, 60] as Range[]).map((r) => ({ value: r, label: t(r === 60 ? "gatewayAside.range60" : "gatewayAside.range15") }))} />
        </div>
        <span className="muted tiny">
          {quiet ? t(range === 60 ? "gatewayAside.quiet60" : "gatewayAside.quiet15") : t("gatewayAside.liveNote")}
        </span>
      </section>
      <div className="gwc-list">
        {METRICS.map((m) => {
          const multi = m.series.length > 1;
          const title = t(m.title);
          return (
            <div key={m.key} className="gwc-card">
              <div className="gwc-head">
                <span className="small strong">{title}</span>
                <span className="tiny muted">{t(m.unit)}</span>
                <span className="grow" />
                {multi ? (
                  <span className="gwc-legend">
                    {m.series.map((x) => (
                      <span key={x.name}><i className={`gwc-sw t-${x.tone}`} />{x.name && t(x.name)} <b>{quiet ? "—" : x.total(inRange)}</b></span>
                    ))}
                  </span>
                ) : (
                  <span className="gwc-headline">{quiet ? "—" : (m.headline ?? m.series[0].total)(inRange)}</span>
                )}
              </div>
              <LineChart
                times={times}
                series={m.series.map((x) => ({ name: x.name ? t(x.name) : "", tone: x.tone, fmt: x.fmt, values: times.map((tm) => x.pick(byT.get(tm))) }))}
                fmt={m.fmt}
                axis={m.axis ?? m.fmt}
                log={m.log}
                label={title}
              />
            </div>
          );
        })}
        <AgentUsage minutes={inRange} agents={agents} />
      </div>
    </aside>
  );
}

/** Calls per agent in the range; hovering (or focusing) a row shows its tokens instead. */
function AgentUsage({ minutes, agents }: { minutes: GatewayMinute[]; agents: AgentState[] }) {
  const rows = useMemo(() => {
    const by = new Map<string, GatewayAgentUse>();
    for (const m of minutes) {
      for (const [id, u] of Object.entries(m.agents ?? {})) {
        const x = by.get(id) ?? { requests: 0, failures: 0, inputTokens: 0, outputTokens: 0 };
        x.requests += u.requests;
        x.failures += u.failures;
        x.inputTokens += u.inputTokens;
        x.outputTokens += u.outputTokens;
        by.set(id, x);
      }
    }
    return [...by].sort((a, b) => b[1].requests - a[1].requests);
  }, [minutes]);
  const max = rows[0]?.[1].requests || 1;

  return (
    <div className="gwc-card">
      <div className="gwc-head">
        <span className="small strong">{t("gatewayAside.byAgentTitle")}</span>
        <span className="tiny muted">{t("gatewayAside.byAgentHint")}</span>
      </div>
      {rows.length === 0 ? (
        <span className="tiny muted gwa-empty">{t("gatewayAside.byAgentEmpty")}</span>
      ) : (
        <ul className="gwa-list">
          {rows.map(([id, u]) => {
            // Agents that aren't installed here (any more) still get their name and icon.
            const known = id in AGENT_NAME ? (id as AgentId) : null;
            const name = id === "agentplus" ? t("gatewayAside.agentSelf") : id === "legacy" ? t("gatewayAside.agentLegacy") : agents.find((x) => x.id === id)?.name ?? agentLabel(id);
            const tok = t("gatewayAside.agentTokens", { input: tokens(u.inputTokens), output: tokens(u.outputTokens) });
            const calls = tn("gatewayAside.agentCalls", u.requests);
            return (
              <li key={id} className="gwa-row" tabIndex={0}
                title={id === "legacy" ? t("gatewayAside.agentLegacyHint") : undefined}
                aria-label={t("gatewayAside.agentAria", { name, calls, tokens: tok })}>
                <span className="gwa-icon" aria-hidden>
                  {known ? <AgentIcon id={known} size={16} /> : id === "agentplus" ? <Icon.gateway size={14} /> : <Icon.key size={14} />}
                </span>
                <span className="gwa-name ellipsis">{name}</span>
                <span className="gwa-val" aria-hidden>
                  <span className="gwa-n">{calls}</span>
                  <span className="gwa-tok">{tok}</span>
                </span>
                <i className="gwa-bar" style={{ width: `calc((100% - 36px) * ${u.requests / max})` }} />
              </li>
            );
          })}
        </ul>
      )}
    </div>
  );
}

const H = 92;
const PAD = { top: 8, right: 6, bottom: 18, left: 34 };

function LineChart({ times, series, fmt, axis, log, label }: {
  times: number[];
  series: { name: string; tone: Tone; fmt?: (v: number) => string; values: (number | null)[] }[];
  fmt: (v: number) => string;
  axis: (v: number) => string;
  log?: boolean;
  label: string;
}) {
  const box = useRef<HTMLDivElement>(null);
  const [w, setW] = useState(300);
  const [hover, setHover] = useState<number | null>(null);
  useLayoutEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setW(el.clientWidth));
    ro.observe(el);
    setW(el.clientWidth);
    return () => ro.disconnect();
  }, []);

  const n = times.length;
  const peak = Math.max(0, ...series.flatMap((s) => s.values.map((v) => v ?? 0)));
  // Log axis: log10(1 + v), so 0 still sits on the baseline; top is the next power of ten.
  const scale = (v: number) => (log ? Math.log10(1 + Math.max(0, v)) : v);
  const decades = Math.max(1, Math.ceil(Math.log10(1 + peak)));
  const max = log ? 10 ** decades : niceMax(peak);
  const top = scale(max);
  const iw = Math.max(10, w - PAD.left - PAD.right);
  const ih = H - PAD.top - PAD.bottom;
  const x = (i: number) => PAD.left + (n <= 1 ? 0 : (i / (n - 1)) * iw);
  const y = (v: number) => PAD.top + ih - (scale(v) / top) * ih;
  // Log: a line per decade, labels on at most ~4 of them. Linear: 0, half, top.
  const step = Math.ceil(decades / 3);
  const grid = log
    ? [0, ...Array.from({ length: decades }, (_, k) => 10 ** (k + 1))].map((v, k) => ({ v, label: k === 0 || k % step === 0 || k === decades }))
    : [0, max / 2, max].map((v) => ({ v, label: true }));

  const paths = series.map((s) => {
    // Split into runs at nulls so gaps stay gaps.
    const runs: [number, number][][] = [];
    let cur: [number, number][] = [];
    s.values.forEach((v, i) => {
      if (v == null) {
        if (cur.length) runs.push(cur);
        cur = [];
      } else cur.push([x(i), y(v)]);
    });
    if (cur.length) runs.push(cur);
    const line = runs.map((r) => r.map(([a, b], k) => `${k ? "L" : "M"}${a.toFixed(1)},${b.toFixed(1)}`).join("")).join("");
    const area = runs
      .filter((r) => r.length > 1)
      .map((r) => `M${r[0][0].toFixed(1)},${y(0)}${r.map(([a, b]) => `L${a.toFixed(1)},${b.toFixed(1)}`).join("")}L${r[r.length - 1][0].toFixed(1)},${y(0)}Z`)
      .join("");
    return { ...s, line, area, lone: runs.filter((r) => r.length === 1).map((r) => r[0]) };
  });
  // One series gets a soft fill; with several, fills would muddy each other.
  const fill = series.length === 1;

  const onMove = (e: React.PointerEvent<SVGRectElement>) => {
    const rect = e.currentTarget.getBoundingClientRect();
    const i = Math.round(((e.clientX - rect.left) / rect.width) * (n - 1));
    setHover(Math.max(0, Math.min(n - 1, i)));
  };
  const tipLeft = hover != null ? Math.min(Math.max(x(hover), 70), w - 70) : 0;
  const ticks = [0, Math.floor((n - 1) / 2), n - 1];

  return (
    <div ref={box} className="gwc-chart">
      <svg width={w} height={H} role="img" aria-label={t(log ? "gatewayAside.chartLabelLog" : "gatewayAside.chartLabel", { label, n })}>
        {grid.map(({ v, label: shown }) => (
          <g key={v}>
            <line x1={PAD.left} x2={w - PAD.right} y1={y(v)} y2={y(v)} className={v === 0 ? "gwc-base" : "gwc-grid"} />
            {shown && <text x={PAD.left - 6} y={y(v) + 3.5} textAnchor="end" className="gwc-axis">{v === 0 ? "0" : axis(v)}</text>}
          </g>
        ))}
        {ticks.map((i, k) => (
          <text key={i} x={x(i)} y={H - 4} textAnchor={k === 0 ? "start" : k === 2 ? "end" : "middle"} className="gwc-axis">
            {k === 2 ? t("gatewayAside.now") : hhmm(times[i])}
          </text>
        ))}
        {paths.map((p) => (
          <g key={p.name} className={`t-${p.tone}`}>
            {fill && <path d={p.area} className="gwc-area" />}
            <path d={p.line} className="gwc-line" />
            {p.lone.map(([a, b], i) => <circle key={i} cx={a} cy={b} r={2.5} className="gwc-pt" />)}
          </g>
        ))}
        {hover != null && (
          <g>
            <line x1={x(hover)} x2={x(hover)} y1={PAD.top} y2={y(0)} className="gwc-cross" />
            {series.map((s) => s.values[hover] != null && (
              <circle key={s.name} cx={x(hover)} cy={y(s.values[hover]!)} r={4} className={`gwc-dot t-${s.tone}`} />
            ))}
          </g>
        )}
        <rect x={PAD.left} y={0} width={iw} height={H} fill="transparent"
          onPointerMove={onMove} onPointerLeave={() => setHover(null)} />
      </svg>
      {hover != null && (
        <div className="gwc-tip" style={{ left: tipLeft }}>
          <span className="muted">{hhmm(times[hover])}</span>
          {series.map((s) => {
            const v = s.values[hover];
            return (
              <span key={s.name} className="gwc-tip-row">
                {series.length > 1 && <><i className={`gwc-sw t-${s.tone}`} />{s.name} </>}
                <b>{v == null ? t("gatewayAside.noRequests") : (s.fmt ?? fmt)(v)}</b>
              </span>
            );
          })}
        </div>
      )}
    </div>
  );
}
