import type zh from "../zh/projectsPage";

const en: typeof zh = {
  justNow: "Just now",
  minutesAgo: "{n} minute ago|{n} minutes ago",
  hoursAgo: "{n} hour ago|{n} hours ago",
  yesterday: "Yesterday",
  daysAgo: "{n} day ago|{n} days ago",
  intro: "Configure OpenCode for a single project folder: the project's opencode.json is merged with the global config, and keys defined in the project take precedence.",
  pickFolder: "Choose folder…",
  or: "or",
  pathPlaceholder: "Paste a folder path, e.g. D:\\xm\\my-app",
  pathLabel: "Folder path",
  recent: "Recent",
  empty: "No projects opened yet. Pick a project folder to give it its own providers, default model and permissions.",
  unapplied: "{n} unapplied|{n} unapplied",
  folderMissing: "Folder not found",
  providerCount: "{n} provider|{n} providers",
  notConfigured: "Not configured · uses global",
  revealTitle: "Show in Explorer",
  revealLabel: "Open the {name} folder",
  forgetTitle: "Remove from list (files are kept)",
  forgetLabel: "Remove {name} from the list",
  noteMerge: "OpenCode looks for opencode.json / opencode.jsonc starting from the launch directory (up to the repository root in a git repo) and merges it with ~/.config/opencode/opencode.json: objects are merged key by key, arrays are replaced as a whole (except instructions, which are merged).",
  noteAuth: "API keys for a project are stored in ~/.local/share/opencode/auth.json, just like global ones, and never written to project files, so opencode.json is safe to commit.",
  backToList: "Back to OpenCode projects",
  hasConfig: "Has opencode.json",
  notCreated: "Not created · uses global",
  subtitle: "OpenCode project config · {dir}",
  switchProject: "Switch project",
  openFolder: "Open folder",
};

export default en;
