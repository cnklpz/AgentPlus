import type zh from "../zh/copyProviderDialog";

const en: typeof zh = {
  globalOpenCode: "Global OpenCode",
  library: "Provider library",
  ariaTitle: "Copy providers to project",
  title: "Copy providers to \"{name}\"",
  intro: "Copies the base URL, API key and visible models into a provider of this project's own, so you can edit its model list separately. The key goes into auth.json, not into project files.",
  filterPlaceholder: "Filter by name or URL",
  empty: "No providers to copy. You can \"Add provider\" instead.",
  inherited: "Inherited",
  modelCount: "{n} model|{n} models",
  noKey: "no API key",
  disableInherited: "Disable the copied global providers in this project (so the picker doesn't show them twice)",
  copyN: "Copy {n}",
};

export default en;
