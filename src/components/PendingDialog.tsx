import { useEffect, useState } from "react";
import { type AgentId, type AgentState, type DiffGroup, api } from "../api";
import { type Draft, opsToWrite } from "../draft";
import { t, tn } from "../i18n";
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
          <button className="icon-btn" aria-label={t("common.close")} disabled={busy} onClick={onCancel}><Icon.close /></button>
        </div>
        <div className="modal-body">
          <span className="muted small">{t("pendingDialog.intro")}</span>
          {withOps.map((a) => {
            const d = diffs[a.id];
            const on = keep[a.id];
            return (
              <section key={a.id} className={`pend${on ? "" : " drop"}`}>
                <div className="row gap10">
                  <AgentIcon id={a.id} size={24} />
                  <strong className="grow">{a.name}<span className="tiny muted">{tn("pendingDialog.changeCount", Object.keys(drafts[a.id]).length)}</span></strong>
                  <div className="seg">
                    <button className={on ? "on" : ""} onClick={() => setKeep((k) => ({ ...k, [a.id]: true }))}>{t("common.apply")}</button>
                    <button className={!on ? "on danger" : ""} onClick={() => setKeep((k) => ({ ...k, [a.id]: false }))}>{t("pendingDialog.discard")}</button>
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
                {!d && <span className="tiny muted">{t("pendingDialog.reading")}</span>}
              </section>
            );
          })}
        </div>
        <div className="modal-foot">
          <span className="muted tiny grow">
            {applying.length ? t("pendingDialog.willWrite", { names: applying.map((a) => a.name).join(t("pendingDialog.nameSep")) }) : t("pendingDialog.discardAll")}
            {withOps.length > applying.length && applying.length ? t("pendingDialog.restDiscarded") : ""}
          </span>
          <button className="btn" disabled={busy} onClick={onCancel}>{t("common.cancel")}</button>
          <button className="btn primary" disabled={busy} onClick={() => onConfirm(applying.map((a) => a.id))}>
            {t(busy ? "pendingDialog.writing" : applying.length ? "pendingDialog.applyContinue" : "pendingDialog.discardContinue")}
          </button>
        </div>
      </div>
    </div>
  );
}
