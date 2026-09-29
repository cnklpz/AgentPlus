import { Component, type ErrorInfo, Fragment, type ReactNode } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api, logClient } from "../api";
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
    // The window has no system title bar (App draws its own): the page can be dragged, and quits
    // from here.
    return (
      <div className="crash" data-tauri-drag-region>
        <h1 data-tauri-drag-region>{t("errorBoundary.title")}</h1>
        <p className="muted">{t("errorBoundary.message")}</p>
        <pre className="mono small">{String(error.message || error)}</pre>
        <div className="row gap10">
          <button className="btn primary" onClick={() => this.setState({ error: null, n: n + 1 })}>{t("errorBoundary.retry")}</button>
          <button className="btn" onClick={() => api.quitApp()}>{t("common.quitApp")}</button>
        </div>
      </div>
    );
  }
}
