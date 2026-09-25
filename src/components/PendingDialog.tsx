import { useState } from "react";
import type { AgentId, AgentState } from "../api";
import { type Draft, agentsWithOps, opCount } from "../draft";
import { t, tn } from "../i18n";
import { AgentIcon } from "./icons";
import { usePreviews } from "../hooks";
import { scrub } from "../privacy";
import { joinList } from "../format";
import { ErrorBox } from "./controls";
import { Modal } from "./Modal";

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
  const withOps = agentsWithOps(agents, drafts);
  const [keep, setKeep] = useState<Record<string, boolean>>(() => Object.fromEntries(withOps.map((a) => [a.id, true])));
  const diffs = usePreviews(agents, drafts);

  const applying = withOps.filter((a) => keep[a.id]);
  const rest = withOps.length > applying.length;
  return (
    // Like the backdrop and the close button, Esc does nothing while the changes are being written.
    <Modal label={title} title={title} wide busy={busy} onClose={onCancel} foot={<>
      <span className="muted tiny grow">
        {!applying.length ? t("common.discardAll") : t(rest ? "pendingDialog.willWriteRestDiscarded" : "pendingDialog.willWrite", { names: joinList(applying.map((a) => a.name)) })}
      </span>
      <button className="btn" disabled={busy} onClick={onCancel}>{t("common.cancel")}</button>
      <button className="btn primary" disabled={busy} onClick={() => onConfirm(applying.map((a) => a.id))}>
        {t(busy ? "common.writing" : applying.length ? "pendingDialog.applyContinue" : "pendingDialog.discardContinue")}
      </button>
    </>}>
      <span className="muted small">{t("pendingDialog.intro")}</span>
      {withOps.map((a) => {
        const d = diffs[a.id];
        const on = keep[a.id];
        return (
          <section key={a.id} className={`pend${on ? "" : " drop"}`}>
            <div className="row gap10">
              <AgentIcon id={a.id} size={24} />
              <strong className="grow">{a.name}<span className="tiny muted">{tn("pendingDialog.changeCount", opCount(drafts[a.id]))}</span></strong>
              <div className="seg">
                <button className={on ? "on" : ""} onClick={() => setKeep((k) => ({ ...k, [a.id]: true }))}>{t("common.apply")}</button>
                <button className={!on ? "on danger" : ""} onClick={() => setKeep((k) => ({ ...k, [a.id]: false }))}>{t("common.discard")}</button>
              </div>
            </div>
            {typeof d === "string" && <ErrorBox text={d} />}
            {Array.isArray(d) && (
              <div className="pend-lines">
                {d.flatMap((g) => g.lines.map((l, i) => (
                  <div key={`${g.file}-${i}`} className={`dline mono ${l.add ? "add" : "del"}`}>{scrub(l.text)}</div>
                )))}
              </div>
            )}
            {!d && <span className="tiny muted">{t("common.reading")}</span>}
          </section>
        );
      })}
    </Modal>
  );
}
