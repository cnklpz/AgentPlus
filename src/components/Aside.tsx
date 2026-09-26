import type { ReactNode } from "react";
import { type AgentState, type DiffGroup, isProjectId } from "../api";
import { Icon } from "./icons";
import { t, tn } from "../i18n";
import { scrub } from "../privacy";
import { ErrorBox } from "./controls";

interface Props {
  st: AgentState;
  diff: DiffGroup[];
  pending: number;
  error: string | null;
  busy: boolean;
  /** Provider details replace "当前配置" while a card is selected. */
  detail: ReactNode | null;
  onDiscard: () => void;
  onApply: () => void;
}

/** Allow line breaks after `_` / `.` so long keys like GOOGLE_GEMINI_BASE_URL wrap at word boundaries. */
function breakable(k: string): ReactNode {
  const parts = k.split(/(?<=[_.])/);
  return parts.length < 2 ? k : parts.map((p, i) => <span key={i}>{p}{i < parts.length - 1 && <wbr />}</span>);
}

export function Aside({ st, diff, pending, error, busy, detail, onDiscard, onApply }: Props) {
  return (
    <aside className="aside" aria-label={t("aside.aria")}>
      {detail ?? <section className="aside-cur">
        <div className="row between">
          <h2>{t("aside.current")}</h2>
          <span className="muted tiny hint">{t("aside.fromFiles")}</span>
        </div>
        <div className="kv">
          {st.current.map((r) => (
            <div key={r.k} className="kv-row">
              <span className="muted small">{breakable(r.k)}</span>
              <span className={r.mono ? "mono small wrap" : "small wrap"}>{scrub(r.v)}</span>
            </div>
          ))}
          {st.restartable && <LaunchRow st={st} />}
        </div>
      </section>}

      <section className="aside-diff">
        <div className="row between">
          <h2>{t("common.pendingChanges")}</h2>
          <span className={`count${pending ? " warn" : ""}`}>{pending ? tn("common.changeCount", pending) : t("common.none")}</span>
        </div>
        {error && <ErrorBox text={error} />}
        <DiffGroups groups={diff} />
        {pending === 0 && !error && <UpToDate hint={t("aside.upToDateHint")} />}
      </section>

      <div className="aside-foot">
        <div className="grid2">
          <button className="btn full" disabled={!pending || busy} onClick={onDiscard}>{t("common.discard")}</button>
          <button className="btn primary full" disabled={!pending || busy || st.readonly} onClick={onApply}>{busy ? t("common.writing") : t("common.apply")}</button>
        </div>
        <span className="muted tiny center hint">{st.restartable
          ? t(st.running ? "aside.footRestart" : "aside.footStart", { name: st.name })
          : t("aside.footNewSession", { name: isProjectId(st.id) ? "OpenCode" : st.name })}</span>
      </div>
    </aside>
  );
}

/** Whether the running app is the one AgentPlus started (UI injection only reaches that one). */
function LaunchRow({ st }: { st: AgentState }) {
  const l = st.running ? st.launch : null;
  const text = !st.running ? t("aside.notRunning")
    : !l ? t("aside.launchUnknown")
    : l.byAgentplus ? t(l.debugPort && st.id === "codex" ? "aside.byAgentplusUi" : "aside.byAgentplus")
    : t("aside.notByAgentplus");
  return (
    <div className="kv-row">
      <span className="muted small">{t("aside.launch")}</span>
      <span className="small wrap">
        <span className={`launch-dot ${!st.running ? "off" : l?.byAgentplus ? "ok" : "warn"}`} />{text}
        {l?.uiInactive && <span className="block tiny warn-text">{t("aside.uiInactive", { name: st.name })}</span>}
      </span>
    </div>
  );
}

/** A diff preview: for each file, the lines that go away and the ones that are added. */
export function DiffGroups({ groups }: { groups: DiffGroup[] }) {
  return (
    <>
      {groups.map((g) => (
        <div key={g.file} className="dgroup">
          <div className="dfile mono ellipsis">{scrub(g.file)}</div>
          {g.lines.map((l, i) => <div key={i} className={`dline mono ${l.add ? "add" : "del"}`}>{scrub(l.text)}</div>)}
        </div>
      ))}
    </>
  );
}

/** Nothing pending: the config files are up to date. */
export function UpToDate({ hint }: { hint: string }) {
  return (
    <div className="dempty">
      <Icon.check size={20} color="var(--ok-dot)" />
      <strong>{t("aside.upToDate")}</strong>
      <span className="muted small hint">{hint}</span>
    </div>
  );
}
