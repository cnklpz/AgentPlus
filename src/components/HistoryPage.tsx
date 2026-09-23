import { useEffect, useState } from "react";
import { type BackupEntry, api } from "../api";

const AGENT_NAME: Record<string, string> = {
  codex: "Codex", zcode: "ZCode", mimo: "MiMo Desktop", "codex-cleanup": "Codex 清理", "codex-repair": "Codex 会话修复",
};

function fmtStamp(s: string): string {
  const m = s.match(/^(\d{4})(\d{2})(\d{2})-(\d{2})(\d{2})(\d{2})/);
  return m ? `${m[1]}-${m[2]}-${m[3]} ${m[4]}:${m[5]}:${m[6]}` : s;
}

function size(b: number): string {
  return b >= 1_048_576 ? `${(b / 1_048_576).toFixed(1)} MB` : `${Math.max(1, Math.round(b / 1024))} KB`;
}

export function HistoryPage({ flash, onChanged }: { flash: (t: string, e?: boolean) => void; onChanged: () => void }) {
  const [list, setList] = useState<BackupEntry[] | null>(null);
  const [confirm, setConfirm] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const load = () => api.listBackups().then(setList).catch((e) => flash(String(e), true));
  useEffect(() => { load(); }, []);

  const restore = async (id: string) => {
    setBusy(true);
    try {
      flash(await api.restoreBackup(id));
      setConfirm(null);
      load();
      onChanged();
    } catch (e) {
      flash(String(e), true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <main className="page">
      <div className="page-top">
        <div className="page-head">
          <div className="page-title">
            <h1>历史与回滚</h1>
            <span className="muted small">AgentPlus 每次写入前都会备份原文件。回滚会把文件恢复到那次写入之前，回滚前的文件也会先备份。</span>
          </div>
          <button className="btn" onClick={load}>刷新</button>
        </div>
      </div>
      <div className="page-body">
        {!list && <div className="empty">正在读取…</div>}
        {list && list.length === 0 && <div className="empty">还没有备份。应用过配置之后，这里会列出每一次写入。</div>}
        {list && list.length > 0 && (
          <div className="stable">
            {list.map((b) => (
              <div key={b.id} className="hrow">
                <div className="minw0">
                  <div className="row gap6">
                    <span className="strong small">{fmtStamp(b.stamp)}</span>
                    <span className="ptag tag-soft">{AGENT_NAME[b.agent] ?? b.agent}</span>
                    <span className="tiny muted">{b.reason}</span>
                  </div>
                  <div className="mono tiny muted ellipsis">{b.files.map((f) => f.name).join("、")} · {size(b.bytes)}</div>
                </div>
                <div className="row gap6">
                  {!b.restorable ? (
                    <span className="tiny muted" title="数据库类备份请在「会话」页撤销，或手动处理">不支持自动回滚</span>
                  ) : confirm === b.id ? (
                    <>
                      <button className="btn small" disabled={busy} onClick={() => setConfirm(null)}>取消</button>
                      <button className="btn small primary" disabled={busy} onClick={() => restore(b.id)}>{busy ? "回滚中…" : "确认回滚"}</button>
                    </>
                  ) : (
                    <button className="btn small" onClick={() => setConfirm(b.id)}>回滚到这之前</button>
                  )}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </main>
  );
}
