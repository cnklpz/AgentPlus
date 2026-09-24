import type zh from "../zh/maintenanceTab";

const en: typeof zh = {
  healthTitle: "Health check",
  recheck: "Check again",
  checking: "Checking…",
  cleanupTitle: "Safe cleanup",
  tmpLabel: "Move leftover temp files away",
  tmpHint: "{n} .tmp file left by an interrupted write, {size}|{n} .tmp files left by interrupted writes, {size}",
  logsLabel: "Trim and compact logs",
  logsHint: "Log database {size}; deletes {old} of {total} old entries, saving about {save}.",
  logsNote: "Only affects the diagnostic logs used by /feedback.",
  keepDays: "Keep {n} day|Keep {n} days",
  walLabel: "Truncate the database write-ahead log (WAL)",
  walHint: "{size} total",
  codexRunning: "Codex is running (including the CLI). Quit it before cleaning up.",
  estimate: "Frees about {size}. Backed up to ~/.agentplus/backups before cleaning.",
  cleaning: "Cleaning…",
  clean: "Clean up",
};

export default en;
