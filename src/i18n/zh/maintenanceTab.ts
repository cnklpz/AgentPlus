// src/components/MaintenanceTab.tsx
export default {
  healthTitle: "健康检查",
  recheck: "重新检查",
  checking: "正在检查…",
  cleanupTitle: "一键安全清理",
  tmpLabel: "移走残留临时文件",
  tmpHint: "{n} 个中断写入留下的 .tmp 文件，{size}",
  logsLabel: "裁剪日志并压缩",
  logsHint: "日志库 {size}，删除 {old} / {total} 条旧日志，预计省 {save}",
  logsNote: "。只影响 /feedback 用的诊断日志。",
  keepDays: "保留 {n} 天",
  walLabel: "截断数据库预写日志（WAL）",
  walHint: "共 {size}",
  codexRunning: "Codex 正在运行（包括 CLI），请先退出再清理。",
  estimate: "预计释放约 {size}。清理前自动备份到 ~/.agentplus/backups。",
  cleaning: "清理中…",
  clean: "清理",
};
