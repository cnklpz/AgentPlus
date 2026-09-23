import type { ReactNode } from "react";
import type { AgentState, DiffGroup } from "../api";
import { Icon } from "./icons";

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

export function Aside({ st, diff, pending, error, busy, detail, onDiscard, onApply }: Props) {
  return (
    <aside className="aside" aria-label="配置与改动">
      {detail ?? <section className="aside-cur">
        <div className="row between">
          <h2>当前配置</h2>
          <span className="muted tiny">读取自配置文件</span>
        </div>
        <div className="kv">
          {st.current.map((r) => (
            <div key={r.k} className="kv-row">
              <span className="muted small">{r.k}</span>
              <span className={r.mono ? "mono small wrap" : "small wrap"}>{r.v}</span>
            </div>
          ))}
        </div>
      </section>}

      <section className="aside-diff">
        <div className="row between">
          <h2>待写入的改动</h2>
          <span className={`count${pending ? " warn" : ""}`}>{pending ? `${pending} 项` : "无"}</span>
        </div>
        {error && <div className="err">{error}</div>}
        {diff.map((g) => (
          <div key={g.file} className="dgroup">
            <div className="dfile mono ellipsis">{g.file}</div>
            {g.lines.map((l, i) => <div key={i} className={`dline mono ${l.add ? "add" : "del"}`}>{l.text}</div>)}
          </div>
        ))}
        {pending === 0 && !error && (
          <div className="dempty">
            <Icon.check size={20} color="#16A34A" />
            <strong>配置已是最新</strong>
            <span className="muted small">在左侧修改后，这里实时列出将写入的内容</span>
          </div>
        )}
      </section>

      <div className="aside-foot">
        <div className="grid2">
          <button className="btn full" disabled={!pending || busy} onClick={onDiscard}>放弃</button>
          <button className="btn primary full" disabled={!pending || busy || st.readonly} onClick={onApply}>{busy ? "写入中…" : "应用"}</button>
        </div>
        <span className="muted tiny center">写入前自动备份原文件 · 应用后点「重启 {st.name}」生效</span>
      </div>
    </aside>
  );
}
