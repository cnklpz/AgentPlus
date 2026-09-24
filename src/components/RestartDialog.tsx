import { useEffect, useRef, useState } from "react";
import type { AgentId, RestartProgress, RestartStatus, RestartStep } from "../api";
import { type TKey, locale, t } from "../i18n";
import { AgentIcon, Icon } from "./icons";
import { Modal } from "./Modal";
import { scrub } from "../privacy";

export interface RunStep {
  id: RestartStep;
  status: "pending" | RestartStatus | "error";
  detail: string | null;
  start: number | null;
  end: number | null;
}

/** One restart (or start) as the dialog shows it, built from the backend's progress messages. */
export interface RestartRun {
  agent: AgentId;
  name: string;
  /** The agent wasn't running: the wording is "start" instead of "restart". */
  starting: boolean;
  steps: RunStep[];
  t0: number;
  result: { ok: boolean; msg: string; at: number } | null;
}

export const newRun = (agent: AgentId, name: string, starting: boolean): RestartRun => ({ agent, name, starting, steps: [], t0: Date.now(), result: null });

export function applyProgress(run: RestartRun, p: RestartProgress): RestartRun {
  if (p.kind === "plan") return { ...run, steps: p.steps.map((id) => ({ id, status: "pending", detail: null, start: null, end: null })) };
  const now = Date.now();
  return {
    ...run,
    steps: run.steps.map((s) => (s.id !== p.step ? s : {
      ...s,
      status: p.status,
      detail: p.detail,
      start: s.start ?? now,
      end: p.status === "active" ? null : now,
    })),
  };
}

/** The run's outcome; on failure the step that was running is marked as the one that failed. */
export function finishRun(run: RestartRun, ok: boolean, msg: string): RestartRun {
  const now = Date.now();
  return {
    ...run,
    steps: ok ? run.steps : run.steps.map((s) => (s.status === "active" ? { ...s, status: "error", end: now } : s)),
    result: { ok, msg, at: now },
  };
}

const LABEL: Record<RestartStep, TKey> = {
  stop: "restartDialog.stepStop",
  start: "restartDialog.stepStart",
  port: "restartDialog.stepPort",
  patch: "restartDialog.stepPatch",
};

const secs = (ms: number) => (ms / 1000).toLocaleString(locale(), { minimumFractionDigits: 1, maximumFractionDigits: 1 });

function StepIcon({ status }: { status: RunStep["status"] }) {
  switch (status) {
    case "active": return <span className="rs-ring" />;
    case "done": return <Icon.check size={12} />;
    case "warn": return <b>!</b>;
    case "error": return <Icon.close size={10} />;
    case "skip": return <b>–</b>;
    default: return null;
  }
}

/** Live progress of a restart: each step with its state, detail and time taken. */
export function RestartDialog({ run, onClose, onCancel }: { run: RestartRun; onClose: () => void; onCancel: () => void }) {
  const { result } = run;
  // Tick while it runs so the timers move.
  const [, setTick] = useState(0);
  useEffect(() => {
    if (result) return;
    const timer = window.setInterval(() => setTick((n) => n + 1), 200);
    return () => window.clearInterval(timer);
  }, [!!result]);
  // A clean success closes by itself; warnings and errors stay until dismissed.
  const clean = !!result?.ok && !run.steps.some((s) => s.status === "warn");
  useEffect(() => {
    if (!clean) return;
    const timer = window.setTimeout(onClose, 1600);
    return () => window.clearTimeout(timer);
  }, [clean]);
  const btnRef = useRef<HTMLButtonElement>(null);
  useEffect(() => { btnRef.current?.focus(); }, [!!result]);

  const now = Date.now();
  const vars = { name: run.name };
  const title = !result
    ? t(run.starting ? "restartDialog.titleStart" : "restartDialog.titleRestart", vars)
    : result.ok
      ? t(run.starting ? "restartDialog.doneStart" : "restartDialog.doneRestart", vars)
      : t(run.starting ? "restartDialog.failedStart" : "restartDialog.failedRestart", vars);
  const total = (result?.at ?? now) - run.t0;
  // Codex's injection summary says what was patched; a plain restart has nothing to add.
  const showMsg = !!result && (!result.ok || run.steps.some((s) => s.id === "patch"));

  const foot = (
    <>
      {!result && <button className="btn" onClick={onCancel}>{t(run.starting ? "restartDialog.cancelStart" : "restartDialog.cancelRestart")}</button>}
      <span className="grow" />
      <button ref={btnRef} className={`btn${result ? " primary" : ""}`} onClick={onClose}>{t(result ? "common.close" : "restartDialog.background")}</button>
    </>
  );

  return (
    <Modal label={title} className="rs" bgClassName="rs-bg" backdropClose={false} bare onClose={onClose} foot={foot}>
      <div className="rs-head">
        <AgentIcon id={run.agent} size={36} />
        <div className="grow minw0">
          <div className="confirm-title">{title}</div>
          <div className="tiny muted" aria-live="off">{t(result ? "restartDialog.took" : "restartDialog.elapsed", { s: secs(total) })}</div>
        </div>
      </div>
      <ol className="rs-steps" aria-live="polite">
        {!run.steps.length && !result && (
          <li className="rs-step active"><span className="rs-icon"><StepIcon status="active" /></span><span className="small">{t("restartDialog.preparing")}</span></li>
        )}
        {run.steps.map((s) => {
          const detail = s.detail ?? (s.status === "skip" ? t("restartDialog.skipped") : null);
          return (
            <li key={s.id} className={`rs-step ${s.status}`}>
              <span className="rs-icon"><StepIcon status={s.status} /></span>
              <div className="grow minw0">
                <div className="small strong">{t(LABEL[s.id], vars)}</div>
                {detail && <div className="tiny muted rs-detail">{scrub(detail)}</div>}
              </div>
              {s.start !== null && s.status !== "skip" && <span className="tiny muted mono noshrink">{secs((s.end ?? now) - s.start)}s</span>}
            </li>
          );
        })}
      </ol>
      {showMsg && <div className={`rs-result${result!.ok ? "" : " error"}`}>{scrub(result!.msg)}</div>}
    </Modal>
  );
}
