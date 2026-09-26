import { type ReactNode, useEffect, useState } from "react";
import { type TestResult, api } from "../api";
import { ComboBox } from "./ComboBox";
import { Icon } from "./icons";
import { t, tx } from "../i18n";
import { scrub } from "../privacy";
import { errText } from "../util";

interface Props {
  /** Where the address and key come from: an agent entry, or "library". */
  source: { agent: string; provider: string } | null;
  models: string[];
  defaultModel?: string | null;
  /** Why testing is not possible right now (e.g. not applied yet). */
  disabled?: string | null;
}

/** Sends one tiny real request through the provider: checks address, key, protocol and model together. */
export function ProviderTest({ source, models, defaultModel, disabled }: Props) {
  const [model, setModel] = useState(defaultModel && models.includes(defaultModel) ? defaultModel : models[0] ?? defaultModel ?? "");
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<TestResult | string | null>(null);

  // A different provider was picked: start over.
  useEffect(() => {
    setResult(null);
    setModel(defaultModel && models.includes(defaultModel) ? defaultModel : models[0] ?? defaultModel ?? "");
  }, [source?.agent, source?.provider]);

  const run = async () => {
    if (!source || !model.trim()) return;
    setRunning(true);
    setResult(null);
    try {
      setResult(await api.testProvider(source.agent, source.provider, model.trim()));
    } catch (e) {
      setResult(errText(e));
    } finally {
      setRunning(false);
    }
  };

  const why = disabled ?? (!source ? t("providerTest.noSource") : null);
  return (
    <div className="ptest">
      <div className="ptest-head">
        <strong className="small">{t("providerTest.title")}</strong>
        <span className="tiny muted hint">{t("providerTest.hint")}</span>
      </div>
      <div className="row gap6">
        <ComboBox value={model} options={models} onChange={setModel} onEnter={run} disabled={!!why || running}
          placeholder={t("providerTest.modelPlaceholder")} label={t("providerTest.modelLabel")} />
        <TestButton running={running} disabled={!!why || running || !model.trim()} onClick={run} />
      </div>
      {why && <span className="tiny muted">{why}</span>}
      {result !== null && <TestResultView result={result} />}
    </div>
  );
}

/** The "Test" button; a spinner while the request runs. */
export function TestButton({ running, disabled, onClick, className = "btn primary", title }: {
  running: boolean; disabled: boolean; onClick: () => void; className?: string; title?: string;
}) {
  return (
    <button className={className} disabled={disabled} onClick={onClick} title={title}>
      {running ? <><span className="spin" aria-hidden="true">↻</span>{t("providerTest.testing")}</> : <><Icon.pulse size={12} />{t("common.test")}</>}
    </button>
  );
}

/** Outcome of a test request (or the error text when it could not be sent). `extra` goes after the status line's facts. */
export function TestResultView({ result, extra }: { result: TestResult | string; extra?: ReactNode }) {
  if (typeof result === "string") return <div className="ptest-res bad"><strong>{t("providerTest.failed")}</strong><span>{scrub(result)}</span></div>;
  return (
    <div className={`ptest-res ${result.ok ? "good" : "bad"}`}>
      <div className="row gap6">
        <strong>{result.ok ? t("providerTest.ok") : t("providerTest.notOk")}</strong>
        <span className="mono tiny">{(result.ms / 1000).toFixed(2)} s</span>
        {result.status != null && <span className="mono tiny">HTTP {result.status}</span>}
        {extra}
        {result.usage && <span className="mono tiny">{t("providerTest.tokens", { input: result.usage[0], output: result.usage[1] })}</span>}
      </div>
      {result.ok
        ? <span>{result.reply ? tx("providerTest.reply", { reply: <span className="mono">{result.reply}</span> }) : t("providerTest.noText")}</span>
        : <span>{scrub(result.error)}</span>}
      <span className="mono tiny faint ellipsis" title={scrub(result.url)}>{result.model} · {scrub(result.url)}</span>
    </div>
  );
}
