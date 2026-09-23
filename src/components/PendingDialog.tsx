import { useEffect, useState } from "react";
import { type AgentId, type AgentState, type DiffGroup, api } from "../api";
import { type Draft, opsToWrite } from "../draft";
import { AgentIcon, Icon } from "./icons";

interface Props {
  title: string;
  agents: AgentState[];
  drafts: Record<string, Draft>;
  busy: boolean;
  /** Agents whose changes should be written first; the rest are discarded. */
  onConfirm: (apply: AgentId[]) => void;
  onCancel: () => void;
}

/** Lists every agent's pending changes and lets the user apply or drop each before leaving. */
export function PendingDialog({ title, agents, drafts, busy, onConfirm, onCancel }: Props) {
  const withOps = agents.filter((a) => Object.keys(drafts[a.id] ?? {}).length > 0);
  const [keep, setKeep] = useState<Record<string, boolean>>(() => Object.fromEntries(withOps.map((a) => [a.id, true])));
  const [diffs, setDiffs] = useState<Record<string, DiffGroup[] | string>>({});

  useEffect(() => {
    for (const a of withOps) {
      api.preview(a.id, opsToWrite(a, drafts[a.id]))
        .then((d) => setDiffs((m) => ({ ...m, [a.id]: d })))
        .catch((e) => setDiffs((m) => ({ ...m, [a.id]: String(e) })));
    }
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onCancel(); };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  const applying = withOps.filter((a) => keep[a.id]);
  return (
    <div className="modal-bg" onMouseDown={(e) => { if (e.target === e.currentTarget && !busy) onCancel(); }}>
      <div className="modal wide" role="dialog" aria-modal="true" aria-label={title}>
        <div className="modal-head">
          <h2>{title}</h2>
          <button className="icon-btn" aria-label="关闭" disabled={busy} onClick={onCancel}><Icon.close /></button>
        </div>
        <div className="modal-body">
          <span className="muted small">还有没写入的改动。选择每个 Agent 是先应用还是放弃：</span>
          {withOps.map((a) => {
            const d = diffs[a.id];
            const on = keep[a.id];
            return (
              <section key={a.id} className={`pend${on ? "" : " drop"}`}>
                <div className="row gap10">
                  <AgentIcon id={a.id} size={24} />
                  <strong className="grow">{a.name}<span className="tiny muted"> · {Object.keys(drafts[a.id]).length} 项</span></strong>
                  <div className="seg">
                    <button className={on ? "on" : ""} onClick={() => setKeep((k) => ({ ...k, [a.id]: true }))}>应用</button>
                    <button className={!on ? "on danger" : ""} onClick={() => setKeep((k) => ({ ...k, [a.id]: false }))}>放弃</button>
                  </div>
                </div>
                {typeof d === "string" && <div className="err">{d}</div>}
                {Array.isArray(d) && (
                  <div className="pend-lines">
                    {d.flatMap((g) => g.lines.map((l, i) => (
                      <div key={`${g.file}-${i}`} className={`dline mono ${l.add ? "add" : "del"}`}>{l.text}</div>
                    )))}
                  </div>
                )}
                {!d && <span className="tiny muted">正在读取…</span>}
              </section>
            );
          })}
        </div>
        <div className="modal-foot">
          <span className="muted tiny grow">
            {applying.length ? `将写入 ${applying.map((a) => a.name).join("、")}（先备份）` : "全部放弃"}
            {withOps.length > applying.length && applying.length ? "，其余放弃" : ""}
          </span>
          <button className="btn" disabled={busy} onClick={onCancel}>取消</button>
          <button className="btn primary" disabled={busy} onClick={() => onConfirm(applying.map((a) => a.id))}>
            {busy ? "写入中…" : applying.length ? "应用并继续" : "放弃并继续"}
          </button>
        </div>
      </div>
    </div>
  );
}
