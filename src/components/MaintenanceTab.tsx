import { useEffect, useState } from "react";
import { type CleanupPreview, type HealthItem, api } from "../api";

const DAYS = [3, 7, 14];

function mb(b: number): string {
  return `${(b / 1_048_576).toFixed(1)} MB`;
}

export function MaintenanceTab({ flash }: { flash: (text: string, error?: boolean) => void }) {
  const [health, setHealth] = useState<HealthItem[] | null>(null);
  const [healthErr, setHealthErr] = useState<string | null>(null);
  const [days, setDays] = useState(3);
  const [pre, setPre] = useState<CleanupPreview | null>(null);
  const [tmp, setTmp] = useState(true);
  const [logs, setLogs] = useState(true);
  const [wal, setWal] = useState(true);
  const [busy, setBusy] = useState(false);

  const check = () => {
    setHealth(null);
    setHealthErr(null);
    api.codexHealth().then(setHealth).catch((e) => setHealthErr(String(e)));
  };
  useEffect(check, []);
  useEffect(() => { api.codexCleanupPreview(days).then(setPre).catch(() => setPre(null)); }, [days, busy]);

  // Rough estimate: deleted rows' share of the file, plus pages already free.
  const logsSave = pre ? Math.round((pre.logsBytes - pre.logsFreeBytes) * (pre.logsOldRows / Math.max(1, pre.logsRows))) + pre.logsFreeBytes : 0;
  const total = pre ? (tmp ? pre.tmpBytes : 0) + (logs ? logsSave : 0) + (wal ? pre.walBytes : 0) : 0;

  const clean = async () => {
    setBusy(true);
    try {
      flash(await api.codexCleanup(tmp, logs ? days : null, wal));
      check();
    } catch (e) {
      flash(String(e), true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="settings">
      <section className="sgroup">
        <h2 className="row between">健康检查<button className="link tiny" onClick={check}>重新检查</button></h2>
        {healthErr && <div className="srow"><span className="err grow">{healthErr}</span></div>}
        {!health && !healthErr && <div className="srow muted small">正在检查…</div>}
        {health?.map((h) => (
          <div key={h.key} className="srow">
            <span className={`hdot ${h.status}`} />
            <div className="grow minw0">
              <div className="slabel">{h.title}</div>
              <div className="muted small">{h.detail}</div>
            </div>
          </div>
        ))}
      </section>

      <section className="sgroup">
        <h2>一键安全清理</h2>
        <label className="srow check-row">
          <input type="checkbox" checked={tmp} onChange={(e) => setTmp(e.target.checked)} />
          <div className="grow">
            <div className="slabel">移走残留临时文件</div>
            <div className="muted small">{pre ? `${pre.tmpCount} 个中断写入留下的 .tmp 文件，${mb(pre.tmpBytes)}` : "…"}</div>
          </div>
        </label>
        <label className="srow check-row">
          <input type="checkbox" checked={logs} onChange={(e) => setLogs(e.target.checked)} />
          <div className="grow">
            <div className="slabel">裁剪日志并压缩</div>
            <div className="muted small">
              {pre ? `日志库 ${mb(pre.logsBytes)}，删除 ${pre.logsOldRows.toLocaleString()} / ${pre.logsRows.toLocaleString()} 条旧日志，预计省 ${mb(logsSave)}` : "…"}
              。只影响 /feedback 用的诊断日志。
            </div>
          </div>
          <div className="chips">
            {DAYS.map((d) => (
              <button key={d} type="button" className={`chip${days === d ? " on" : ""}`} onClick={(e) => { e.preventDefault(); setDays(d); }}>保留 {d} 天</button>
            ))}
          </div>
        </label>
        <label className="srow check-row">
          <input type="checkbox" checked={wal} onChange={(e) => setWal(e.target.checked)} />
          <div className="grow">
            <div className="slabel">截断数据库预写日志（WAL）</div>
            <div className="muted small">{pre ? `共 ${mb(pre.walBytes)}` : "…"}</div>
          </div>
        </label>
        <div className="srow">
          <div className="grow muted small">
            {pre?.codexRunning ? "Codex 正在运行（包括 CLI），请先退出再清理。" : `预计释放约 ${mb(total)}。清理前自动备份到 ~/.agentplus/backups。`}
          </div>
          <button className="btn primary" disabled={busy || !pre || pre.codexRunning || (!tmp && !logs && !wal)} onClick={clean}>
            {busy ? "清理中…" : "清理"}
          </button>
        </div>
      </section>
    </div>
  );
}
