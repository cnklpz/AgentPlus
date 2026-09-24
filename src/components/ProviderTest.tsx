import { useEffect, useState } from "react";
import { type TestResult, api } from "../api";
import { ComboBox } from "./ComboBox";
import { Icon } from "./icons";
import { t, tx } from "../i18n";

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
      setResult(String(e));
    } finally {
      setRunning(false);
    }
  };

  const why = disabled ?? (!source ? t("providerTest.noSource") : null);
  return (
    <div className="ptest">
      <div className="ptest-head">
        <strong className="small">{t("providerTest.title")}</strong>
        <span className="tiny muted">{t("providerTest.hint")}</span>
      </div>
      <div className="row gap6">
        <ComboBox value={model} options={models} onChange={setModel} onEnter={run} disabled={!!why || running}
          placeholder={t("providerTest.modelPlaceholder")} label={t("providerTest.modelLabel")} />
        <button className="btn primary" disabled={!!why || running || !model.trim()} onClick={run}>
          {running ? <><span className="spin" aria-hidden="true">↻</span>{t("providerTest.testing")}</> : <><Icon.pulse size={12} />{t("common.test")}</>}
        </button>
      </div>
      {why && <span className="tiny muted">{why}</span>}
      {typeof result === "string" && <div className="ptest-res bad"><strong>{t("providerTest.failed")}</strong><span>{result}</span></div>}
      {result && typeof result !== "string" && (
        <div className={`ptest-res ${result.ok ? "good" : "bad"}`}>
          <div className="row gap6">
            <strong>{result.ok ? t("providerTest.ok") : t("providerTest.notOk")}</strong>
            <span className="mono tiny">{(result.ms / 1000).toFixed(2)} s</span>
            {result.status != null && <span className="mono tiny">HTTP {result.status}</span>}
            {result.usage && <span className="mono tiny">{result.usage[0]} → {result.usage[1]} tokens</span>}
          </div>
          {result.ok
            ? <span>{result.reply ? tx("providerTest.reply", { reply: <span className="mono">{result.reply}</span> }) : t("providerTest.noText")}</span>
            : <span>{result.error}</span>}
          <span className="mono tiny faint ellipsis" title={result.url}>{result.model} · {result.url}</span>
        </div>
      )}
    </div>
  );
}
