// src/components/RestartDialog.tsx
import type zh from "../zh/restartDialog";

const en: typeof zh = {
  titleRestart: "Restarting {name}",
  titleStart: "Starting {name}",
  doneRestart: "{name} restarted",
  doneStart: "{name} started",
  failedRestart: "Couldn't restart {name}",
  failedStart: "Couldn't start {name}",
  preparing: "Preparing…",
  stepStop: "Close {name}",
  stepStart: "Start {name}",
  stepPort: "Connect to the debug port",
  stepPatch: "Patch the UI",
  skipped: "Skipped",
  elapsed: "{s}s elapsed",
  took: "Took {s}s",
  background: "Run in background",
  settingsHint: "Switch to a brief notice in Settings › Interface",
};

export default en;
