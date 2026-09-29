import { Component, type ErrorInfo, Fragment, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { logClient } from "../api";
import { t } from "../i18n";
import { inTauri } from "../tauri";

/**
 * Catches a render error anywhere below it. The window starts hidden and App shows it after
 * its first render, so an error there would otherwise leave AgentPlus running with no window.
 */
export class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null; n: number }> {
  state = { error: null as Error | null, n: 0 };

  static getDerivedStateFromError(error: Error) {
    return { error };
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    logClient("error", "render", `${error.stack ?? error}\n${info.componentStack ?? ""}`);
    if (inTauri) getCurrentWindow().show().catch(() => {});
  }

  render() {
    const { error, n } = this.state;
    if (!error) return <Fragment key={n}>{this.props.children}</Fragment>;
    return (
      <div className="crash">
        <h1>{t("errorBoundary.title")}</h1>
        <p className="muted">{t("errorBoundary.message")}</p>
        <pre className="mono small">{String(error.message || error)}</pre>
        <button className="btn primary" onClick={() => this.setState({ error: null, n: n + 1 })}>{t("errorBoundary.retry")}</button>
      </div>
    );
  }
}
