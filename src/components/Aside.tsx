import type { ReactNode } from "react";
import { type AgentState, type DiffGroup, isProjectId } from "../api";
import { Icon } from "./icons";
import { t, tn } from "../i18n";
import { scrub } from "../privacy";

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
          <span className="muted tiny">{t("aside.fromFiles")}</span>
        </div>
        <div className="kv">
          {st.current.map((r) => (
            <div key={r.k} className="kv-row">
              <span className="muted small">{breakable(r.k)}</span>
              <span className={r.mono ? "mono small wrap" : "small wrap"}>{scrub(r.v)}</span>
            </div>
          ))}
        </div>
      </section>}

      <section className="aside-diff">
        <div className="row between">
          <h2>{t("aside.pendingTitle")}</h2>
          <span className={`count${pending ? " warn" : ""}`}>{pending ? tn("aside.pendingCount", pending) : t("common.none")}</span>
        </div>
        {error && <div className="err">{scrub(error)}</div>}
        {diff.map((g) => (
          <div key={g.file} className="dgroup">
            <div className="dfile mono ellipsis">{scrub(g.file)}</div>
            {g.lines.map((l, i) => <div key={i} className={`dline mono ${l.add ? "add" : "del"}`}>{scrub(l.text)}</div>)}
          </div>
        ))}
        {pending === 0 && !error && (
          <div className="dempty">
            <Icon.check size={20} color="#16A34A" />
            <strong>{t("aside.upToDate")}</strong>
            <span className="muted small">{t("aside.upToDateHint")}</span>
          </div>
        )}
      </section>

      <div className="aside-foot">
        <div className="grid2">
          <button className="btn full" disabled={!pending || busy} onClick={onDiscard}>{t("aside.discard")}</button>
          <button className="btn primary full" disabled={!pending || busy || st.readonly} onClick={onApply}>{busy ? t("aside.writing") : t("common.apply")}</button>
        </div>
        <span className="muted tiny center">{st.restartable
          ? t(st.running ? "aside.footRestart" : "aside.footStart", { name: st.name })
          : t("aside.footNewSession", { name: isProjectId(st.id) ? "OpenCode" : st.name })}</span>
      </div>
    </aside>
  );
}
