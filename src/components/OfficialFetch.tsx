import { useEffect, useState } from "react";
import { type FetchedModel, type OfficialFetch as Status, api } from "../api";
import { Icon } from "./icons";
import { t, tn, tx } from "../i18n";
import { scrub } from "../privacy";
import { errText, type Flash } from "../util";

interface Props {
  /** Pending Codex changes block the flow (config.toml is swapped temporarily). */
  pending: number;
  running: boolean;
  /** Desktop app AgentPlus can restart (false for the CLI, e.g. inside WSL). */
  restartable: boolean;
  restarting: boolean;
  onRestart: () => void;
  /** Re-read Codex after the catalog changed. */
  onReload: () => void;
  flash: Flash;
}

type Step = "idle" | "confirm" | "wait" | "ready" | "done";

/**
 * Fetch the official model list: temporarily point Codex at the ChatGPT login, let it
 * download models_cache.json, copy that into the catalog and restore config.toml.
 */
export function OfficialFetch({ pending, running, restartable, restarting, onRestart, onReload, flash }: Props) {
  const [st, setSt] = useState<Status | null>(null);
  const [step, setStep] = useState<Step>("idle");
  const [busy, setBusy] = useState(false);
  const [models, setModels] = useState<FetchedModel[]>([]);

  const load = () => api.officialStatus().then((s) => {
    setSt(s);
    setStep((cur) => (cur === "done" || cur === "confirm" ? cur : s.active ? (s.cacheReady ? "ready" : "wait") : "idle"));
  }).catch(() => undefined);
  useEffect(() => { load(); }, []);
  // While waiting for Codex, watch for the cache file.
  useEffect(() => {
    if (step !== "wait") return;
    const timer = window.setInterval(load, 2500);
    return () => window.clearInterval(timer);
  }, [step]);

  const run = async (f: () => Promise<void>) => {
    setBusy(true);
    try {
      await f();
    } catch (e) {
      flash(errText(e), true);
    } finally {
      setBusy(false);
    }
  };

  const start = () => run(async () => {
    setSt(await api.officialStart());
    setStep("wait");
    onReload();
    flash(t("officialFetch.startedFlash"));
  });
  const cancel = () => run(async () => {
    await api.officialCancel();
    setStep("idle");
    onReload();
    flash(t("officialFetch.cancelledFlash"));
  });
  const finish = () => run(async () => {
    const list = await api.officialFinish();
    setModels(list);
    setStep("done");
    onReload();
    flash(tn("officialFetch.finishedFlash", list.length));
  });

  const steps = [t("officialFetch.stepBackup"), t("officialFetch.stepLogin"), t("officialFetch.stepCopy"), t("officialFetch.stepRestore")];
  const at = step === "wait" ? 1 : step === "ready" ? 2 : step === "done" ? 4 : 0;

  return (
    <section className="card ofetch">
      <div className="ofetch-head">
        <span className="ofetch-icon"><Icon.cloud size={18} /></span>
        <div className="grow minw0">
          <div className="slabel">{t("officialFetch.title")}</div>
          <div className="muted small">{t("officialFetch.intro")}</div>
        </div>
        {step === "idle" && (
          <button className="btn primary" disabled={busy || pending > 0} title={pending ? t("officialFetch.pendingFirst") : undefined} onClick={() => setStep("confirm")}>
            <Icon.cloud size={13} />{t("officialFetch.start")}
          </button>
        )}
      </div>

      {step !== "idle" && (
        <ol className="ofetch-steps">
          {steps.map((s, i) => <li key={s} className={i < at ? "done" : i === at ? "now" : ""}><b>{i < at ? "✓" : i + 1}</b>{s}</li>)}
        </ol>
      )}

      {step === "idle" && (
        <div className="ofetch-note">
          <Icon.key size={13} /><span>{tx("officialFetch.idleNote", { plan: <b>{t("officialFetch.planPlus")}</b> })}{pending > 0 && <b className="warn-text"> {tn("officialFetch.pendingWarn", pending)}</b>}</span>
        </div>
      )}

      {step === "confirm" && (
        <div className="ofetch-body">
          <div className="ofetch-note warn">
            <Icon.key size={13} />
            <span>{tx("officialFetch.confirmNote", { plan: <b>{t("officialFetch.planAll")}</b>, catalog: scrub(st?.catalogPath) ?? "models.json" })}</span>
          </div>
          <div className="row gap6">
            <span className="grow" />
            <button className="btn" onClick={() => setStep("idle")}>{t("common.cancel")}</button>
            <button className="btn primary" disabled={busy} onClick={start}>{busy ? t("officialFetch.processing") : t("officialFetch.backupStart")}</button>
          </div>
        </div>
      )}

      {step === "wait" && (
        <div className="ofetch-body">
          <div className="ofetch-task">
            <span className="ofetch-n">1</span>
            <div className="grow minw0">
              <strong className="small">{t("officialFetch.restartCodex")}</strong>
              <div className="tiny muted">{tx("officialFetch.restartDesc", { dir: <span className="mono">{scrub(st?.backupDir)}</span> })}</div>
            </div>
            {restartable ? (
              <button className="btn primary small" disabled={restarting} onClick={onRestart}>
                <Icon.refresh size={12} />{restarting ? t("officialFetch.restarting") : running ? t("officialFetch.restartCodex") : t("officialFetch.startCodex")}
              </button>
            ) : <span className="tiny muted">{t("officialFetch.rerunInTerminal")}</span>}
          </div>
          <div className="ofetch-task">
            <span className="ofetch-n">2</span>
            <div className="grow minw0">
              <strong className="small">{t("officialFetch.loginTitle")}</strong>
              <div className="tiny muted">{tx("officialFetch.loginDesc", { detected: st?.chatgptLogin ? t("officialFetch.loginDetected") : "", path: <span className="mono">{scrub(st?.cachePath)}</span> })}</div>
            </div>
          </div>
          <div className="ofetch-wait">
            <span className="spin">↻</span>
            <span className="small">{t("officialFetch.waiting")}</span>
            <span className="grow" />
            <button className="btn small" disabled={busy} onClick={load}>{t("officialFetch.checkNow")}</button>
            <button className="btn small danger" disabled={busy} onClick={cancel}>{t("officialFetch.cancelRestore")}</button>
          </div>
        </div>
      )}

      {step === "ready" && (
        <div className="ofetch-body">
          <div className="ofetch-note ok">
            <Icon.check size={13} /><span>{tx("officialFetch.readyNote", { count: <b>{st?.cacheModels}</b>, catalog: <span className="mono">{scrub(st?.catalogPath)}</span> })}</span>
          </div>
          <div className="row gap6">
            <button className="btn small danger" disabled={busy} onClick={cancel}>{t("officialFetch.cancelRestore")}</button>
            <span className="grow" />
            <button className="btn primary" disabled={busy} onClick={finish}>{busy ? t("common.writing") : t("officialFetch.writeRestore")}</button>
          </div>
        </div>
      )}

      {step === "done" && (
        <div className="ofetch-body">
          <div className="ofetch-note ok">
            <Icon.check size={13} /><span>{tx("officialFetch.doneNote", { n: models.length, restart: <b>{t("officialFetch.pleaseRestart")}</b> })}</span>
          </div>
          <div className="ofetch-models">
            {models.map((m) => (
              <span key={m.slug} className={`ofetch-model${m.visible ? "" : " hidden"}`} title={m.visible ? t("officialFetch.shown") : t("officialFetch.hiddenByDefault")}>
                <span className="mono small">{m.slug}</span>{m.name && m.name !== m.slug && <span className="tiny muted">{m.name}</span>}
              </span>
            ))}
          </div>
          <div className="row gap6">
            <span className="tiny muted grow">{t("officialFetch.adjustHint")}</span>
            <button className="btn" onClick={() => setStep("idle")}>{t("common.done")}</button>
            {restartable && <button className="btn primary" disabled={restarting} onClick={onRestart}><Icon.refresh size={12} />{restarting ? t("officialFetch.restarting") : t("officialFetch.restartCodex")}</button>}
          </div>
        </div>
      )}
    </section>
  );
}
