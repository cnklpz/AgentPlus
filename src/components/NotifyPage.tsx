import { useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { inTauri } from "../tauri";
import { type AgentState, type LibEntry, type NotifyAgent, type NotifyChannel, type NotifyConfig, type NotifyHour, type NotifyStatus, api } from "../api";
import { type TKey, locale, t, tn, useLang } from "../i18n";
import { fmtAgo } from "../format";
import { AGENT_NAME } from "../services";
import { scrub } from "../privacy";
import { type Flash, errText, toggledIn } from "../util";
import { AgentIcon, Icon } from "./icons";
import { ErrorBox, Kv, Seg, SettingRow, StatTile, Switch } from "./controls";
import { Dropdown } from "./Dropdown";
import { ComboBox } from "./ComboBox";

const CHANNELS: { id: NotifyChannel; name: string; desc: TKey }[] = [
  { id: "bark", name: "Bark", desc: "notifyPage.barkDesc" },
  { id: "ntfy", name: "ntfy", desc: "notifyPage.ntfyDesc" },
  { id: "gotify", name: "Gotify", desc: "notifyPage.gotifyDesc" },
  { id: "serverchan", name: "ServerChan", desc: "notifyPage.serverchanDesc" },
  { id: "webhook", name: "Webhook", desc: "notifyPage.webhookDesc" },
];

/** Minimum turn lengths offered, in seconds. */
const MIN_SECS = [0, 30, 60, 180, 600];

const minLabel = (s: number) => (s === 0 ? t("notifyPage.minAny") : s < 60 ? t("notifyPage.secs", { n: s }) : t("notifyPage.mins", { n: s / 60 }));

const when = (ms: number) => new Date(ms).toLocaleString(locale(), { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" });

type Settings = NotifyConfig["settings"];

/** Completion notifications: which agents to follow, where to push, and what was sent.
 *  `onAside`: whether the page shows its right pane (while notifications are on), for the layout. */
export function NotifyPage({ flash, onAside, agents }: { flash: Flash; onAside: (on: boolean) => void; agents: AgentState[] }) {
  const lang = useLang();
  const [st, setSt] = useState<NotifyStatus | null>(null);
  // The settings as shown: text fields are saved when they lose focus.
  const [cfg, setCfg] = useState<NotifyConfig | null>(null);
  const [testing, setTesting] = useState<NotifyChannel | null>(null);
  const [testErr, setTestErr] = useState<Partial<Record<NotifyChannel, string>>>({});
  const [lib, setLib] = useState<LibEntry[] | null>(null);
  const [sumTest, setSumTest] = useState<{ busy: boolean; text?: string; error?: string }>({ busy: false });
  // Saves go out one after another, so a quick second change can't be overtaken by the first.
  const saving = useRef<Promise<unknown>>(Promise.resolve());

  // Sent messages arrive in the background: follow them while the page is open. Backend
  // text (titles, errors) follows the language too.
  useEffect(() => {
    let live = true;
    const load = (first: boolean) => api.notifyStatus().then((s) => {
      if (!live) return;
      setSt(s);
      // Keep what is being edited when only the language changed.
      if (first) setCfg((c) => c ?? s.config);
    }, (e) => { if (first) flash(errText(e), true); });
    void load(true);
    const timer = window.setInterval(() => { void load(false); }, 4000);
    // Switched from the tray menu: take its on/off, keep the rest as being edited.
    const off = inTauri ? listen("notify-changed", () => {
      api.notifyStatus().then((s) => {
        if (!live) return;
        setSt(s);
        setCfg((c) => (c ? { ...c, enabled: s.config.enabled } : s.config));
      }, () => undefined);
    }) : null;
    return () => { live = false; window.clearInterval(timer); void off?.then((f) => f()); };
  }, [lang]);

  // Providers for the AI summary, once it is turned on.
  const wantLib = !!cfg?.includeReply && !!cfg.summary;
  useEffect(() => {
    if (!wantLib || lib) return;
    let live = true;
    api.libraryList().then((l) => { if (live) setLib(l); }, (e) => flash(errText(e), true));
    return () => { live = false; };
  }, [wantLib, lang]);

  const aside = !!st?.config.enabled;
  useEffect(() => { onAside(aside); }, [aside]);
  useEffect(() => () => onAside(false), []);

  if (!st || !cfg) return <main className="page"><div className="empty">{t("notifyPage.loading")}</div></main>;

  const save = (next: NotifyConfig) => {
    setCfg(next);
    saving.current = saving.current.then(() => api.notifySetConfig(next)).then(setSt, (e) => flash(errText(e), true));
  };
  const testSummary = async () => {
    setSumTest({ busy: true });
    try {
      setSumTest({ busy: false, text: await api.notifyTestSummary(cfg) });
    } catch (e) {
      setSumTest({ busy: false, error: errText(e) });
    }
  };
  const sources = summarySources(agents, lib ?? []);
  const sumKey = JSON.stringify([cfg.summaryAgent, cfg.summaryProvider]);
  const sumSource = sources.find((s) => s.value === sumKey);
  const edit = <K extends keyof Settings>(k: K, v: Partial<Settings[K]>) => setCfg({ ...cfg, settings: { ...cfg.settings, [k]: { ...cfg.settings[k], ...v } } });
  const commit = () => { if (JSON.stringify(cfg) !== JSON.stringify(st.config)) save(cfg); };
  const test = async (c: NotifyChannel, name: string) => {
    setTesting(c);
    setTestErr((m) => ({ ...m, [c]: undefined }));
    try {
      await api.notifyTest(cfg, c);
      flash(t("notifyPage.testOk", { name }));
    } catch (e) {
      setTestErr((m) => ({ ...m, [c]: errText(e) }));
    } finally {
      setTesting(null);
    }
  };

  const ready = cfg.agents.length > 0 && cfg.channels.length > 0;
  const [stateKey, stateClass]: [TKey, string] = !cfg.enabled ? ["notifyPage.stateOff", "chip-muted"] : ready ? ["notifyPage.stateOn", "chip-ok"] : ["notifyPage.stateIdle", "chip-bad"];
  const field = (label: TKey, value: string, onChange: (v: string) => void, opts: { placeholder?: string; secret?: boolean; mono?: boolean } = {}) => (
    <label className="notify-field">
      <span className="tiny muted">{t(label)}</span>
      <input className={`input${opts.mono === false ? "" : " mono"}${opts.secret ? " sensitive" : ""}`} value={value} placeholder={opts.placeholder}
        spellCheck={false} autoComplete="off" onChange={(e) => onChange(e.target.value)} onBlur={commit}
        onKeyDown={(e) => { if (e.key === "Enter") commit(); }} />
    </label>
  );
  const fields = (c: NotifyChannel) => {
    const s = cfg.settings;
    switch (c) {
      case "bark": return <>
        {field("notifyPage.barkKey", s.bark.key, (v) => edit("bark", { key: v }), { placeholder: "https://api.day.app/…", secret: true })}
        {field("notifyPage.server", s.bark.server, (v) => edit("bark", { server: v }), { placeholder: "https://api.day.app" })}
        {field("notifyPage.sound", s.bark.sound, (v) => edit("bark", { sound: v }), { placeholder: t("notifyPage.soundDefault"), mono: false })}
      </>;
      case "ntfy": return <>
        {field("notifyPage.topic", s.ntfy.topic, (v) => edit("ntfy", { topic: v }), { secret: true })}
        {field("notifyPage.server", s.ntfy.server, (v) => edit("ntfy", { server: v }), { placeholder: "https://ntfy.sh" })}
        {field("notifyPage.token", s.ntfy.token, (v) => edit("ntfy", { token: v }), { placeholder: t("notifyPage.optional"), secret: true })}
      </>;
      case "gotify": return <>
        {field("notifyPage.server", s.gotify.server, (v) => edit("gotify", { server: v }), { placeholder: "https://gotify.example.com" })}
        {field("notifyPage.appToken", s.gotify.token, (v) => edit("gotify", { token: v }), { secret: true })}
        {field("notifyPage.priority", s.gotify.priority, (v) => edit("gotify", { priority: v }), { placeholder: "5" })}
      </>;
      case "serverchan": return field("notifyPage.sendKey", s.serverchan.key, (v) => edit("serverchan", { key: v }), { placeholder: "SCT… / sctp…", secret: true });
      case "webhook": return field("notifyPage.webhookUrl", s.webhook.url, (v) => edit("webhook", { url: v }), { placeholder: "https://…", secret: true });
    }
  };

  return (
    <>
    <main className="page">
      <div className="page-top">
        <div className="page-head">
          <span className="page-icon"><Icon.bell size={20} /></span>
          <div className="page-title">
            <h1 className="row gap10">{t("notifyPage.title")}<span className={stateClass}>{t(stateKey)}</span></h1>
            <span className="muted small hint">{t("notifyPage.intro")}</span>
          </div>
          <Switch on={cfg.enabled} label={t("notifyPage.title")} onChange={(on) => save({ ...cfg, enabled: on })} />
        </div>
      </div>
      <div className="page-body">
        <div className="settings full">
          <section className="sgroup">
            <h2>{t("notifyPage.agentsTitle")}</h2>
            <div className="srow stacked">
              <div className="check-chips" role="group" aria-label={t("notifyPage.agentsTitle")}>
                {st.agents.map((a) => (
                  <label key={a.id} className={`check-chip${a.found ? "" : " off"}`} title={a.found ? undefined : t("notifyPage.notFound")}>
                    <input type="checkbox" checked={cfg.agents.includes(a.id)} onChange={() => save({ ...cfg, agents: toggledIn(cfg.agents, a.id as NotifyAgent) })} />
                    <AgentIcon id={a.id} size={18} />
                    <span>{AGENT_NAME[a.id]}</span>
                    {!a.found && <span className="tiny muted">{t("notifyPage.notFound")}</span>}
                  </label>
                ))}
              </div>
              <span className="muted tiny hint">{t("notifyPage.agentsHint")}</span>
            </div>
          </section>

          <section className="sgroup">
            <h2>{t("notifyPage.channelsTitle")}</h2>
            {CHANNELS.map((c) => {
              const on = cfg.channels.includes(c.id);
              return (
                <div key={c.id} className="notify-channel">
                  <SettingRow as="label" className="check-row" label={c.name} desc={t(c.desc)}
                    lead={<input type="checkbox" checked={on} onChange={() => save({ ...cfg, channels: toggledIn(cfg.channels, c.id) })} />} />
                  {on && (
                    <div className="notify-fields">
                      <div className="notify-grid">{fields(c.id)}</div>
                      {c.id === "webhook" && <span className="tiny muted hint">{t("notifyPage.webhookHint")}</span>}
                      {testErr[c.id] && <ErrorBox text={testErr[c.id]!} alert />}
                      <div className="row gap6">
                        <button className="btn small" disabled={testing !== null} onClick={() => { void test(c.id, c.name); }}>
                          <Icon.bell size={12} />{t(testing === c.id ? "notifyPage.testing" : "notifyPage.test")}
                        </button>
                      </div>
                    </div>
                  )}
                </div>
              );
            })}
          </section>

          <section className="sgroup">
            <h2>{t("notifyPage.rulesTitle")}</h2>
            <SettingRow label={t("notifyPage.minDuration")} desc={t("notifyPage.minDurationHint")}>
              <Seg value={cfg.minSecs} onChange={(v) => save({ ...cfg, minSecs: v })} label={t("notifyPage.minDuration")}
                options={MIN_SECS.map((s) => ({ value: s, label: minLabel(s) }))} />
            </SettingRow>
            <SettingRow label={t("notifyPage.includeReply")} desc={t("notifyPage.includeReplyHint")}>
              <Switch on={cfg.includeReply} onChange={(v) => save({ ...cfg, includeReply: v })} label={t("notifyPage.includeReply")} />
            </SettingRow>
            {cfg.includeReply && (
              <SettingRow label={t("notifyPage.summary")} desc={t("notifyPage.summaryHint")}>
                <Switch on={cfg.summary} onChange={(v) => save({ ...cfg, summary: v })} label={t("notifyPage.summary")} />
              </SettingRow>
            )}
            {cfg.includeReply && cfg.summary && (
              <div className="notify-fields notify-summary">
                <div className="notify-grid">
                  <label className="notify-field">
                    <span className="tiny muted">{t("notifyPage.summaryProvider")}</span>
                    <Dropdown value={sumSource ? sumKey : ""} label={t("notifyPage.summaryProvider")} options={sources} disabled={sources.length === 0}
                      onChange={(v) => {
                        const [agent, provider] = JSON.parse(v) as [string, string];
                        const s = sources.find((x) => x.value === v);
                        save({ ...cfg, summaryAgent: agent, summaryProvider: provider, summaryModel: s?.models[0] ?? "" });
                      }} />
                  </label>
                  {/* A model picked from the list saves at once; a typed one on Enter or leaving the field. */}
                  <label className="notify-field" onBlur={commit}>
                    <span className="tiny muted">{t("notifyPage.summaryModel")}</span>
                    <ComboBox value={cfg.summaryModel} options={sumSource?.models ?? []} label={t("notifyPage.summaryModel")}
                      onChange={(v) => (sumSource?.models.includes(v) ? save({ ...cfg, summaryModel: v }) : setCfg({ ...cfg, summaryModel: v }))} onEnter={commit} />
                  </label>
                </div>
                {lib && sources.length === 0 && <span className="tiny muted">{t("notifyPage.noProviders")}</span>}
                {sumTest.error && <ErrorBox text={sumTest.error} alert />}
                {sumTest.text && <div className="notify-summary-out small sensitive">{sumTest.text}</div>}
                <div className="row gap6">
                  <button className="btn small" disabled={sumTest.busy || !sumSource || !cfg.summaryModel.trim()} onClick={() => { commit(); void testSummary(); }}>
                    <Icon.play size={12} />{t(sumTest.busy ? "notifyPage.testing" : "notifyPage.testSummary")}
                  </button>
                </div>
              </div>
            )}
          </section>

          <section className="sgroup">
            <h2 className="row between">
              {t("notifyPage.recentTitle")}
              {st.recent.length > 0 && <button className="link" onClick={() => api.notifyClearRecent().then(setSt, (e) => flash(errText(e), true))}>{t("notifyPage.clear")}</button>}
            </h2>
            {st.recent.length === 0 && <div className="srow muted small">{t("notifyPage.recentEmpty")}</div>}
            {st.recent.map((r, i) => (
              <div key={`${r.atMs}-${i}`} className="srow notify-record">
                <AgentIcon id={r.agent} size={22} />
                <div className="grow minw0">
                  <div className="small strong ellipsis sensitive">{r.title}</div>
                  <div className="tiny muted notify-body sensitive">{r.body}</div>
                </div>
                <div className="notify-meta">
                  <span className="tiny muted">{when(r.atMs)}</span>
                  <span className="row gap6">
                    {r.sent.map((s) => (
                      <span key={s.channel} className={s.error ? "chip-bad" : "chip-ok"} title={s.error ? scrub(s.error) : t("notifyPage.delivered")}>
                        {CHANNELS.find((c) => c.id === s.channel)?.name ?? s.channel}
                      </span>
                    ))}
                  </span>
                </div>
              </div>
            ))}
          </section>
        </div>
      </div>
    </main>
    {aside && <NotifyAside st={st} />}
    </>
  );
}

/** A provider the summary can use: one of an agent's (with an address of its own), or a
 *  provider library entry. `value` is JSON `[agent, provider]`. */
export interface SummarySource {
  value: string;
  label: string;
  group: string;
  hint?: string;
  models: string[];
}

export function summarySources(agents: AgentState[], lib: LibEntry[]): SummarySource[] {
  const out: SummarySource[] = [];
  for (const a of agents) {
    for (const p of a.providers) {
      // Built-in providers without an address of their own (an official sign-in) can't be called from here.
      if (!p.enabled || !p.baseUrl) continue;
      out.push({ value: JSON.stringify([a.id, p.id]), label: p.name || p.id, group: a.name, hint: p.host, models: p.models.map((m) => m.id) });
    }
  }
  for (const e of lib) {
    out.push({ value: JSON.stringify(["library", e.id]), label: e.name, group: t("notifyPage.library"), hint: e.hasKey ? undefined : t("notifyPage.noKey"), models: e.models });
  }
  return out;
}

type Range = "day" | "week";

interface Bucket {
  /** Start (Unix seconds). */
  t: number;
  turns: number;
  failed: number;
}

/** The chart's bars: the last 24 hours, or the last 7 local days. */
export function buckets(hours: NotifyHour[], nowMs: number, range: Range): Bucket[] {
  if (range === "day") {
    const end = Math.floor(nowMs / 3600_000) * 3600;
    return Array.from({ length: 24 }, (_, i) => {
      const t = end - (23 - i) * 3600;
      const h = hours.find((x) => x.t === t);
      return { t, turns: h?.turns ?? 0, failed: h?.failed ?? 0 };
    });
  }
  const today = new Date(nowMs);
  today.setHours(0, 0, 0, 0);
  return Array.from({ length: 7 }, (_, i) => {
    const d = new Date(today);
    d.setDate(d.getDate() - (6 - i));
    const next = new Date(d);
    next.setDate(next.getDate() + 1);
    const [a, b] = [d.getTime() / 1000, next.getTime() / 1000];
    const inDay = hours.filter((x) => x.t >= a && x.t < b);
    return { t: a, turns: inDay.reduce((n, x) => n + x.turns, 0), failed: inDay.reduce((n, x) => n + x.failed, 0) };
  });
}

/** Right pane while notifications are on: the watcher's state, counts and a chart of pushes. */
function NotifyAside({ st }: { st: NotifyStatus }) {
  const [range, setRange] = useState<Range>("day");
  const cfg = st.config;
  const bars = useMemo(() => buckets(st.hours, st.nowMs, range), [st.hours, st.nowMs, range]);
  const { turns, failed, byAgent } = useMemo(() => {
    const inRange = st.hours.filter((h) => h.t >= bars[0].t);
    const m = new Map<string, number>();
    for (const h of inRange) for (const [a, n] of Object.entries(h.agents)) m.set(a, (m.get(a) ?? 0) + (n ?? 0));
    return {
      turns: inRange.reduce((n, h) => n + h.turns, 0),
      failed: inRange.reduce((n, h) => n + h.failed, 0),
      byAgent: [...m].sort((a, b) => b[1] - a[1]),
    };
  }, [st.hours, bars]);
  const top = byAgent[0]?.[1] || 1;
  const ready = cfg.agents.length > 0 && cfg.channels.length > 0;
  const last = st.lastMs ?? st.recent[0]?.atMs;
  return (
    <aside className="aside wx-aside" aria-label={t("notifyPage.asideLabel")}>
      <section className="aside-cur">
        <h2 className="row between">{t("notifyPage.status")}<span className={ready ? "chip-ok" : "chip-bad"}>{t(ready ? "notifyPage.stateOn" : "notifyPage.stateIdle")}</span></h2>
        <div className="info-list">
          <Kv label={t("notifyPage.since")}>{st.sinceMs ? fmtAgo(st.sinceMs, st.nowMs) : "—"}</Kv>
          <Kv label={t("notifyPage.lastTurn")}>{last ? fmtAgo(last, st.nowMs) : t("notifyPage.never")}</Kv>
          <Kv label={t("notifyPage.channelsTitle")}>{cfg.channels.length ? cfg.channels.map((c) => CHANNELS.find((x) => x.id === c)?.name ?? c).join(" · ") : "—"}</Kv>
          <Kv label={t("notifyPage.agentsTitle")}>
            <span className="notify-agent-icons">
              {cfg.agents.length ? cfg.agents.map((a) => <span key={a} title={AGENT_NAME[a]}><AgentIcon id={a} size={16} /></span>) : "—"}
            </span>
          </Kv>
          <Kv label={t("notifyPage.minDuration")}>{minLabel(cfg.minSecs)}</Kv>
        </div>
      </section>
      <section className="aside-cur">
        <div className="row between">
          <h2>{t("notifyPage.activity")}</h2>
          <Seg className="gwc-range" value={range} onChange={setRange} label={t("notifyPage.rangeLabel")}
            options={[{ value: "day" as Range, label: t("notifyPage.range24h") }, { value: "week" as Range, label: t("notifyPage.range7d") }]} />
        </div>
        <div className="stat-tiles">
          <StatTile label={t("notifyPage.tileTurns")} n={turns} />
          <StatTile label={t("notifyPage.tileDelivered")} n={turns - failed} />
          <StatTile label={t("notifyPage.tileFailed")} n={failed} tone="bad" />
        </div>
      </section>
      <div className="gwc-list">
        <div className="gwc-card">
          <div className="gwc-head">
            <span className="small strong">{t("notifyPage.chartTitle")}</span>
            <span className="tiny muted">{t(range === "day" ? "notifyPage.perHour" : "notifyPage.perDay")}</span>
            <span className="grow" />
            <span className="gwc-legend">
              <span><i className="gwc-sw t-a" />{t("notifyPage.delivered")}</span>
              <span><i className="gwc-sw t-bad" />{t("notifyPage.failed")}</span>
            </span>
          </div>
          <Bars bars={bars} range={range} />
        </div>
        <div className="gwc-card">
          <div className="gwc-head"><span className="small strong">{t("notifyPage.byAgent")}</span></div>
          {byAgent.length === 0 ? <span className="tiny muted gwa-empty">{t("notifyPage.byAgentEmpty")}</span> : (
            <ul className="gwa-list">
              {byAgent.map(([id, n]) => (
                <li key={id} className="gwa-row">
                  <span className="gwa-icon" aria-hidden><AgentIcon id={id as NotifyAgent} size={16} /></span>
                  <span className="gwa-name ellipsis">{AGENT_NAME[id as NotifyAgent] ?? id}</span>
                  <span className="gwa-val">{tn("notifyPage.turnsN", n)}</span>
                  <i className="gwa-bar" style={{ width: `calc((100% - 36px) * ${n / top})` }} />
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </aside>
  );
}

const CH = 96;

/** Stacked bars, delivered below and failed on top; one per hour or per day. */
function Bars({ bars, range }: { bars: Bucket[]; range: Range }) {
  const max = Math.max(1, ...bars.map((b) => b.turns));
  const w = 10;
  const label = (b: Bucket) => {
    const d = new Date(b.t * 1000);
    return range === "day" ? d.toLocaleTimeString(locale(), { hour: "2-digit", minute: "2-digit" }) : d.toLocaleDateString(locale(), { month: "numeric", day: "numeric" });
  };
  const ticks = range === "day" ? [0, 6, 12, 18, 23] : [0, 1, 2, 3, 4, 5, 6];
  return (
    <div className="notify-chart">
      <svg viewBox={`0 0 ${bars.length * w} ${CH}`} preserveAspectRatio="none" role="img"
        aria-label={t("notifyPage.chartAria", { n: bars.reduce((n, b) => n + b.turns, 0) })}>
        <line x1={0} x2={bars.length * w} y1={CH - 0.5} y2={CH - 0.5} className="notify-chart-base" />
        {bars.map((b, i) => {
          const ok = b.turns - b.failed;
          const hOk = (ok / max) * (CH - 4);
          const hBad = (b.failed / max) * (CH - 4);
          return (
            <g key={b.t} className="notify-bar">
              <rect x={i * w} y={0} width={w} height={CH} className="notify-bar-hit" />
              {ok > 0 && <rect x={i * w + 2} y={CH - hOk} width={w - 4} height={hOk} rx={1.5} className="t-a" />}
              {b.failed > 0 && <rect x={i * w + 2} y={CH - hOk - hBad} width={w - 4} height={hBad} rx={1.5} className="t-bad" />}
              <title>{`${label(b)} · ${tn("notifyPage.turnsN", b.turns)}${b.failed ? ` · ${t("notifyPage.failedN", { n: b.failed })}` : ""}`}</title>
            </g>
          );
        })}
      </svg>
      <div className="notify-chart-x tiny muted">
        {ticks.map((i) => <span key={i} style={{ left: `${((i + 0.5) / bars.length) * 100}%` }}>{label(bars[i])}</span>)}
      </div>
    </div>
  );
}
