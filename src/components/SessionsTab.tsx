import { useEffect, useMemo, useState } from "react";
import { type SessionList, type SessionRow, api } from "../api";
import { Dropdown } from "./Dropdown";

interface Props {
  /** Default target: the fixed id when on, else the configured provider. */
  target: string;
  flash: (text: string, error?: boolean) => void;
  /** Prefilled search (e.g. a session id picked in Ctrl+K). */
  initialQuery?: string;
}

const KIND_LABEL: Record<SessionRow["kind"], string> = {
  user: "对话", automation: "自动化", subagent: "子代理", review: "审查", exec: "exec", agent: "代理创建",
};

function fmtSize(b: number): string {
  if (b >= 1_048_576) return `${(b / 1_048_576).toFixed(1)} MB`;
  return `${Math.max(1, Math.round(b / 1024))} KB`;
}

function fmtTime(ms: number): string {
  const d = new Date(ms);
  const diff = (Date.now() - ms) / 1000;
  if (diff < 3600) return `${Math.max(1, Math.round(diff / 60))} 分钟前`;
  if (diff < 86400) return `${Math.round(diff / 3600)} 小时前`;
  if (diff < 7 * 86400) return `${Math.round(diff / 86400)} 天前`;
  return `${d.getMonth() + 1}月${d.getDate()}日`;
}

const Warn = () => (
  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
    <path d="M12 9v4M12 17h.01" /><path d="M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0Z" />
  </svg>
);

export function SessionsTab({ target, flash, initialQuery }: Props) {
  const [data, setData] = useState<SessionList | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [provider, setProvider] = useState<string>("all");
  const [allKinds, setAllKinds] = useState(!!initialQuery);
  const [showArchived, setShowArchived] = useState(!!initialQuery);
  const [q, setQ] = useState(initialQuery ?? "");
  const [busy, setBusy] = useState(false);
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [sources, setSources] = useState<Set<string> | null>(null);
  const [repairTarget, setRepairTarget] = useState(target);
  const [moveTarget, setMoveTarget] = useState(target);
  const [confirm, setConfirm] = useState<string | null>(null); // "repair" | "move" | session id

  useEffect(() => { if (initialQuery) { setQ(initialQuery); setAllKinds(true); setShowArchived(true); } }, [initialQuery]);

  const load = () => {
    setError(null);
    api.codexSessions().then((d) => { setData(d); setPicked(new Set()); }).catch((e) => setError(String(e)));
  };
  useEffect(load, []);
  useEffect(() => { setRepairTarget(target); setMoveTarget(target); }, [target]);

  const targetOptions = useMemo(() => {
    const t = data ? [...data.targets] : [];
    if (!t.includes(target)) t.push(target);
    return t.map((v) => ({
      value: v,
      label: v,
      hint: v === target ? "当前供应商" : data && !data.targets.includes(v) ? "还没在 config.toml 里定义" : undefined,
    }));
  }, [data, target]);

  // Sessions not on the repair target, grouped by their provider.
  const misplacedBy = useMemo(() => {
    const m = new Map<string, number>();
    for (const s of data?.sessions ?? []) if (s.provider && s.provider !== repairTarget) m.set(s.provider, (m.get(s.provider) ?? 0) + 1);
    return [...m.entries()].sort((a, b) => b[1] - a[1]);
  }, [data, repairTarget]);
  const chosenSources = sources ?? new Set(misplacedBy.map(([p]) => p));
  const repairIds = (data?.sessions ?? []).filter((s) => chosenSources.has(s.provider) && s.provider !== repairTarget).map((s) => s.id);

  const rows = useMemo(() => {
    if (!data) return [];
    const needle = q.trim().toLowerCase();
    return data.sessions.filter((s) =>
      (provider === "all" || s.provider === provider) &&
      (allKinds || s.kind === "user" || s.kind === "automation") &&
      (showArchived || !s.archived) &&
      (!needle || s.title.toLowerCase().includes(needle) || s.cwd.toLowerCase().includes(needle) || s.id.includes(needle)),
    );
  }, [data, provider, allKinds, showArchived, q]);
  const shown = rows.slice(0, 300);
  const allShownPicked = shown.length > 0 && shown.every((s) => picked.has(s.id));

  const run = async (fn: () => Promise<string>) => {
    setBusy(true);
    try {
      flash(await fn());
      setSources(null);
      setConfirm(null);
      load();
    } catch (e) {
      flash(String(e), true);
    } finally {
      setBusy(false);
    }
  };

  if (error) return <div className="empty">{error}</div>;
  if (!data) return <div className="empty">正在读取会话…</div>;

  const canWrite = data.writable && !data.codexRunning;
  const blockedWhy = data.codexRunning ? "先退出 Codex（包括 CLI）" : !data.writable ? "数据库版本未验证，只读" : undefined;
  const definedTarget = (t: string) => data.targets.includes(t);
  const last = data.lastRepair && !data.lastRepair.undone ? data.lastRepair : null;
  const togglePick = (id: string) => setPicked((s) => { const n = new Set(s); if (n.has(id)) n.delete(id); else n.add(id); return n; });

  return (
    <div className="stack12">
      {misplacedBy.length > 0 && (
        <section className="card repair2">
          <div className="repair2-icon" aria-hidden="true"><Warn /></div>
          <div className="grow minw0 stack8">
            <div>
              <div className="strong">有 {misplacedBy.reduce((n, [, c]) => n + c, 0)} 个会话在 Codex 里可能看不到</div>
              <div className="muted small">它们记录在别的供应商下，而 Codex 的最近列表和归档只显示当前供应商的会话。迁移后会重新出现；会先自动备份，可撤销。</div>
            </div>
            <div className="repair2-bar">
              <span className="tiny muted">来源</span>
              <div className="chips left">
                {misplacedBy.map(([p, n]) => {
                  const on = chosenSources.has(p);
                  return (
                    <button key={p} className={`chip${on ? " on" : ""}`} aria-pressed={on}
                      onClick={() => { const next = new Set(chosenSources); if (on) next.delete(p); else next.add(p); setSources(next); }}>
                      {p} <b>{n}</b>
                    </button>
                  );
                })}
              </div>
              <span className="tiny muted">迁移到</span>
              <Dropdown value={repairTarget} options={targetOptions} label="目标供应商" onChange={(t) => { setRepairTarget(t); setSources(null); setConfirm(null); }} />
              <span className="grow" />
              {confirm === "repair" ? (
                <>
                  <button className="btn" disabled={busy} onClick={() => setConfirm(null)}>取消</button>
                  <button className="btn primary" disabled={busy} onClick={() => run(() => api.codexRepair(repairIds, repairTarget))}>{busy ? "处理中…" : `确认修复 ${repairIds.length} 个`}</button>
                </>
              ) : (
                <button className="btn primary" disabled={!canWrite || busy || repairIds.length === 0} title={blockedWhy} onClick={() => setConfirm("repair")}>
                  {blockedWhy ? blockedWhy : `修复 ${repairIds.length} 个`}
                </button>
              )}
            </div>
            {!definedTarget(repairTarget) && <div className="tiny warn-text">「{repairTarget}」还没在 config.toml 里定义，迁移后这些会话暂时无法恢复。</div>}
          </div>
        </section>
      )}

      {last && (
        <div className="row between note-line">
          <span className="small">上次操作：{last.count} 个会话 → 「{last.target}」</span>
          <button className="link" disabled={!canWrite || busy} onClick={() => run(() => api.codexUndoRepair(last.stamp))}>撤销</button>
        </div>
      )}
      {data.note && <div className="notes"><span>{data.note}</span></div>}

      <div className="toolbar">
        <div className="seg">
          <button className={provider === "all" ? "on" : ""} onClick={() => setProvider("all")}>全部 <b>{data.sessions.length}</b></button>
          {data.providers.map(([p, n]) => (
            <button key={p} className={provider === p ? "on" : ""} onClick={() => setProvider(p)}>{p || "(空)"} <b>{n}</b></button>
          ))}
        </div>
        <label className="toggle"><input type="checkbox" checked={allKinds} onChange={(e) => setAllKinds(e.target.checked)} /><span>子代理/审查</span></label>
        <label className="toggle"><input type="checkbox" checked={showArchived} onChange={(e) => setShowArchived(e.target.checked)} /><span>已归档</span></label>
        <div className="search-box">
          <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" aria-hidden="true"><circle cx="11" cy="11" r="7" /><path d="m20 20-3.5-3.5" /></svg>
          <input placeholder="搜索标题、目录或 ID" value={q} onChange={(e) => setQ(e.target.value)} />
        </div>
        <button className="icon-btn" title="刷新" aria-label="刷新" onClick={load}>
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M21 12a9 9 0 1 1-2.6-6.4L21 8" /><path d="M21 3v5h-5" /></svg>
        </button>
      </div>

      {picked.size > 0 && (
        <div className="movebar">
          <span className="strong small">已选 {picked.size} 个</span>
          <span className="tiny muted">迁移到</span>
          <Dropdown value={moveTarget} options={targetOptions} label="迁移到" onChange={(t) => { setMoveTarget(t); setConfirm(null); }} />
          <span className="grow" />
          {confirm === "move" ? (
            <>
              <button className="btn" disabled={busy} onClick={() => setConfirm(null)}>取消</button>
              <button className="btn primary" disabled={busy} onClick={() => run(() => api.codexRepair([...picked], moveTarget))}>{busy ? "处理中…" : "确认迁移"}</button>
            </>
          ) : (
            <button className="btn primary" disabled={!canWrite || busy} title={blockedWhy} onClick={() => setConfirm("move")}>迁移 {picked.size} 个</button>
          )}
          <button className="link" onClick={() => { setPicked(new Set()); setConfirm(null); }}>取消选择</button>
        </div>
      )}

      <div className="stable">
        <div className="srow-h">
          <input type="checkbox" aria-label="全选当前列表" checked={allShownPicked}
            onChange={() => setPicked((s) => { const n = new Set(s); shown.forEach((r) => (allShownPicked ? n.delete(r.id) : n.add(r.id))); return n; })} />
          <span>会话</span><span>供应商</span><span>更新</span><span />
        </div>
        {shown.map((s) => {
          const misplaced = !!s.provider && s.provider !== target;
          return (
            <div key={s.id} className={`sess${picked.has(s.id) ? " picked" : ""}`}>
              <input type="checkbox" aria-label={`选择 ${s.title}`} checked={picked.has(s.id)} onChange={() => togglePick(s.id)} />
              <div className="minw0">
                <div className="row gap6 minw0">
                  <span className="sess-title ellipsis">{s.title}</span>
                  {s.kind !== "user" && <span className="mtag">{KIND_LABEL[s.kind]}</span>}
                  {s.archived && <span className="mtag">已归档</span>}
                </div>
                <div className="mono tiny muted ellipsis">{s.cwd}</div>
                {s.hidden.length > 0 && <div className="why2"><Warn />{s.hidden.join("；")}</div>}
              </div>
              <span className={`ptag ${misplaced ? "tag-warn" : "tag-soft"}`}>{s.provider || "—"}</span>
              <span className="meta2">
                <span className="small">{fmtTime(s.updatedMs)}</span>
                <span className="tiny muted mono">{s.rolloutExists ? fmtSize(s.size) : "文件缺失"}</span>
              </span>
              <span className="row gap6 sess-actions">
                {misplaced && s.rolloutExists && (
                  confirm === s.id ? (
                    <>
                      <button className="btn xs primary" disabled={busy} onClick={() => run(() => api.codexRepair([s.id], target))}>{busy ? "…" : "确认"}</button>
                      <button className="btn xs" disabled={busy} onClick={() => setConfirm(null)}>取消</button>
                    </>
                  ) : (
                    <button className="btn xs restore" disabled={!canWrite || busy} title={blockedWhy ?? `迁移到「${target}」，让它重新出现在 Codex 里`} onClick={() => setConfirm(s.id)}>恢复</button>
                  )
                )}
                <button className="icon-btn sm" title={`复制恢复命令 codex resume ${s.id}`} aria-label="复制恢复命令"
                  onClick={() => navigator.clipboard.writeText(`codex resume ${s.id}`).then(() => flash("已复制恢复命令")).catch(() => flash("复制失败", true))}>
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><rect x="9" y="9" width="12" height="12" rx="2" /><path d="M5 15V5a2 2 0 0 1 2-2h10" /></svg>
                </button>
                <button className="icon-btn sm" title="在资源管理器中显示会话文件" aria-label="显示会话文件" disabled={!s.rolloutExists} onClick={() => api.revealPath(s.rolloutPath)}>
                  <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z" /></svg>
                </button>
              </span>
            </div>
          );
        })}
        {shown.length === 0 && <div className="empty-row muted small">没有符合条件的会话</div>}
        <div className="mtable-foot muted small">显示 {shown.length} / {rows.length} 个会话</div>
      </div>
    </div>
  );
}
