import type zh from "../zh/serviceDialog";

const en: typeof zh = {
  hintResponses: "OpenAI Responses API (/v1/responses), the only one Codex supports",
  hintChat: "OpenAI-compatible Chat Completions (/v1/chat/completions)",
  hintAnthropic: "Anthropic Messages API (/v1/messages)",
  blockedGateway: "{agent} only supports the Google Gemini protocol, which the local gateway can't convert to",
  blockedProto: "{agent} only supports the {api} API; turn on \"Use local gateway\" to connect it with conversion",
  addGroupTo: "Add group to \"{station}\"",
  editGroup: "Edit group \"{name}\"",
  protocol: "Protocol",
  protoMissing: "{vendor} has no {api} endpoint; agents that need it can convert through the local gateway",
  tplProtocols: "This template supports {list}; agents that only support another protocol use the matching URL automatically.",
  groupHint: "Create a separate group for each protocol or API key of the same relay.",
  keyStorage: "Stored locally in ~/.agentplus and written to each agent in its own format.",
  commonModels: "Common models",
  commonModelsHint: "(the initial model list when adding to ZCode / MiMo)",
  noModels: "No models yet. Fetch them or add them manually; you can also change them later in each agent's \"Model list\".",
  syncTo: "Sync to agents already using this group",
  syncHint: "(only needed when the URL, protocol or API key changes)",
  addTo: "Add to",
  gatewayOn: "After saving, the gateway starts automatically with a route for this provider, and checked agents point to the gateway: protocols are converted automatically (Codex and Claude Code work too), and the API key stays in the provider library only.",
  gatewayOff: "Off: checked agents connect directly to the URL above.",
  needsApi: "· needs {api}",
  convertsTo: "· to {api}",
  usesAltUrl: "· uses {api} URL",
  noneRequired: "You can leave them all unchecked: it's saved to the provider library only, and you can add it anytime.",
  footNote: "The provider library is saved right away; changes to agents go to \"Pending changes\" first.",
};

export default en;
