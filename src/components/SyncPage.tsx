import { useEffect, useState } from "react";
import { type SyncStatus, type SyncSuggestion, api } from "../api";
import { AgentIcon } from "./icons";

interface Props {
  flash: (t: string, e?: boolean) => void;
  /** Adds the chosen suggestions to the drafts of their agents. */
  onAdopt: (s: SyncSuggestion[]) => void;
}

export function SyncPage({ flash, onAdopt }: Props) {
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [folder, setFolder] = useState("");
  const [sugs, setSugs] = useState<SyncSuggestion[] | null>(null);
  const [chosen, setChosen] = useState<Set<number>>(new Set());
  const [busy, setBusy] = useState(false);

  const load = () => api.syncStatus().then((s) => { setStatus(s); setFolder(s.folder ?? ""); }).catch((e) => flash(String(e), true));
  useEffect(() => { load(); }, []);

  const wrap = async (fn: () => Promise<void>) => {
    setBusy(true);
    try { await fn(); } catch (e) { flash(String(e), true); } finally { setBusy(false); }
  };

  const saveFolder = () => wrap(async () => { await api.syncSetFolder(folder); flash("同步文件夹已保存"); await load(); });
  const exportNow = () => wrap(async () => { flash(await api.syncExport()); await load(); });
  const preview = () => wrap(async () => {
    const s = await api.syncPreview();
    setSugs(s);
    setChosen(new Set(s.map((_, i) => i)));
    if (s.length === 0) flash("同步文件和本机一致，没有需要导入的");
  });
  const adopt = () => {
    if (!sugs) return;
    onAdopt(sugs.filter((_, i) => chosen.has(i)));
    setSugs(null);
  };

  return (
    <main className="page">
      <div className="page-top">
        <div className="page-head">
          <div className="page-title">
            <h1>多设备同步</h1>
            <span className="muted small">选一个会被同步的文件夹（网盘、NAS、U 盘都可以）。导出时写入供应商和模型列表，不含任何密钥；另一台设备导入后，只需要补填密钥。</span>
          </div>
        </div>
      </div>
      <div className="page-body">
        <div className="settings">
          <section className="sgroup">
            <h2>同步文件夹</h2>
            <div className="srow">
              <input className="input mono grow" value={folder} onChange={(e) => setFolder(e.target.value)} placeholder="例如 D:\OneDrive\AgentPlus" />
              <button className="btn" disabled={busy || !folder.trim()} onClick={saveFolder}>保存</button>
              {status?.folder && <button className="btn" onClick={() => api.openPath(status.folder!)}>打开</button>}
            </div>
            <div className="srow muted small">
              {status?.fileExists
                ? `文件夹里有同步文件：${status.machine ? `来自 ${status.machine}，` : ""}导出于 ${status.exportedAt ? new Date(status.exportedAt).toLocaleString() : "未知时间"}`
                : status?.folder ? "文件夹里还没有同步文件。" : "还没有设置同步文件夹。"}
            </div>
          </section>

          <section className="sgroup">
            <h2>同步</h2>
            <div className="srow">
              <div className="grow">
                <div className="slabel">导出本机配置</div>
                <div className="muted small">把三个 Agent 的供应商和模型列表写到同步文件夹（覆盖旧的同步文件）。</div>
              </div>
              <button className="btn primary" disabled={busy || !status?.folder} onClick={exportNow}>导出</button>
            </div>
            <div className="srow">
              <div className="grow">
                <div className="slabel">从同步文件导入</div>
                <div className="muted small">和本机对比，列出缺少的供应商和模型。选中的会加入对应 Agent 的「待写入的改动」，确认后再应用。</div>
              </div>
              <button className="btn" disabled={busy || !status?.fileExists} onClick={preview}>对比</button>
            </div>
          </section>

          {sugs && sugs.length > 0 && (
            <section className="sgroup">
              <h2>可以导入的内容</h2>
              {sugs.map((s, i) => (
                <label key={i} className="srow check-row">
                  <input type="checkbox" checked={chosen.has(i)} onChange={() => setChosen((c) => { const n = new Set(c); if (n.has(i)) n.delete(i); else n.add(i); return n; })} />
                  <AgentIcon id={s.agent} size={22} />
                  <div className="grow minw0">
                    <div className="slabel">{s.title}</div>
                    <div className="muted small ellipsis">{s.detail}</div>
                  </div>
                </label>
              ))}
              <div className="srow">
                <span className="grow muted small">新增的供应商没有密钥，导入后在供应商详情里点「编辑」补上。</span>
                <button className="btn primary" disabled={chosen.size === 0} onClick={adopt}>加入待写入（{chosen.size}）</button>
              </div>
            </section>
          )}
        </div>
      </div>
    </main>
  );
}
