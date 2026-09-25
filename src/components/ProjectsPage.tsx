import { useState } from "react";
import type { AgentState, ProjectEntry } from "../api";
import { Dropdown } from "./Dropdown";
import { AgentIcon, Icon } from "./icons";
import { t, tn, tx } from "../i18n";
import { fmtAgo } from "../format";
import { scrub } from "../privacy";
import { onActivateKey } from "../util";
import { isMac } from "../platform";

interface ListProps {
  projects: ProjectEntry[];
  busy: boolean;
  /** Pending changes per project agent id. */
  pending: Record<string, number>;
  onOpen: (path: string) => void;
  onPick: () => void;
  onForget: (p: ProjectEntry) => void;
  onReveal: (path: string) => void;
}

/** OpenCode's 项目 tab: open a folder (picker or typed path) or one of the recent projects. */
export function ProjectList({ projects, busy, pending, onOpen, onPick, onForget, onReveal }: ListProps) {
  const [path, setPath] = useState("");
  const go = () => { if (path.trim()) onOpen(path.trim()); };
  return (
    <div className="stack12">
      <span className="muted small">{t("projectsPage.intro")}</span>
      <div className="proj-open">
        <button className="btn primary" disabled={busy} onClick={onPick}><Icon.folder />{t("projectsPage.pickFolder")}</button>
        <span className="muted small">{t("projectsPage.or")}</span>
        <input className="input mono grow sensitive" value={path} placeholder={t("projectsPage.pathPlaceholder")} aria-label={t("projectsPage.pathLabel")}
          onChange={(e) => setPath(e.target.value)} onKeyDown={(e) => { if (e.key === "Enter") go(); }} />
        <button className="btn" disabled={busy || !path.trim()} onClick={go}>{t("common.open")}</button>
      </div>
      <div className="side-label">{t("projectsPage.recent")}</div>
      {projects.length === 0 ? (
        <div className="empty">{t("projectsPage.empty")}</div>
      ) : (
        <div className="proj-list">
          {projects.map((p) => {
            const n = pending[p.agent] ?? 0;
            return (
              <div key={p.path} className={`proj-row${p.exists ? "" : " gone"}`} role="button" tabIndex={0}
                onClick={() => p.exists && onOpen(p.path)} onKeyDown={p.exists ? onActivateKey(() => onOpen(p.path)) : undefined}>
                <span className="proj-icon"><Icon.folder size={16} /></span>
                <span className="grow minw0">
                  <span className="row gap6 minw0">
                    <span className="strong ellipsis sensitive">{p.name}</span>
                    {p.git && <span className="api-chip">git</span>}
                    {n > 0 && <span className="ptag tag-new">{tn("projectsPage.unapplied", n)}</span>}
                  </span>
                  <span className="block mono tiny muted ellipsis sensitive">{scrub(p.path)}</span>
                </span>
                <span className="proj-meta small">
                  {!p.exists ? <span className="warn-text">{t("projectsPage.folderMissing")}</span>
                    : p.config ? <span>opencode.json{p.providers ? ` · ${tn("common.providerCount", p.providers)}` : ""}</span>
                    : <span className="muted">{t("projectsPage.notConfigured")}</span>}
                  <span className="tiny muted">{fmtAgo(p.lastOpened)}</span>
                </span>
                <span className="row gap6" onClick={(e) => e.stopPropagation()}>
                  {p.exists && <button className="icon-btn sm" title={t(isMac ? "projectsPage.revealTitleMac" : "projectsPage.revealTitle")} aria-label={t("projectsPage.revealLabel", { name: p.name })} onClick={() => onReveal(p.path)}><Icon.folder size={12} /></button>}
                  <button className="icon-btn sm" title={t("projectsPage.forgetTitle")} aria-label={t("projectsPage.forgetLabel", { name: p.name })} onClick={() => onForget(p)}><Icon.close size={10} /></button>
                </span>
              </div>
            );
          })}
        </div>
      )}
      <div className="notes">
        <span>{t("projectsPage.noteMerge")}</span>
        <span>{t("projectsPage.noteAuth")}</span>
      </div>
    </div>
  );
}

interface HeadProps {
  st: AgentState;
  project: ProjectEntry | undefined;
  projects: ProjectEntry[];
  onBack: () => void;
  onSwitch: (path: string) => void;
  onOpenDir: () => void;
}

/** Header of one project's page (replaces the agent header in AgentPage). */
export function ProjectHead({ st, project, projects, onBack, onSwitch, onOpenDir }: HeadProps) {
  const exists = !!project?.config;
  const others = projects.filter((p) => p.exists);
  return (
    <div className="page-head">
      <button className="icon-btn" title={t("projectsPage.backToList")} aria-label={t("projectsPage.backToList")} onClick={onBack}><Icon.back /></button>
      <AgentIcon id={st.id} size={46} />
      <div className="page-title">
        <div className="row gap10 minw0">
          <h1 className="ellipsis">{st.name}</h1>
          {exists ? <span className="chip-ok nowrap">{t("projectsPage.hasConfig")}</span> : <span className="chip-muted nowrap">{t("projectsPage.notCreated")}</span>}
        </div>
        <span className="muted small ellipsis">{tx("projectsPage.subtitle", { dir: <span className="mono">{scrub(st.configDir)}</span> })}</span>
      </div>
      {others.length > 1 && (
        <div className="proj-switch">
          <Dropdown value={project?.path ?? ""} label={t("projectsPage.switchProject")} onChange={onSwitch}
            options={others.map((p) => ({ value: p.path, label: p.name, hint: p.path }))} />
        </div>
      )}
      <button className="btn" onClick={onOpenDir}><Icon.folder />{t("projectsPage.openFolder")}</button>
    </div>
  );
}
