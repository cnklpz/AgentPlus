import type zh from "../zh/services";

const en: typeof zh = {
  gatewayStation: "Local gateway",
  geminiOnly: "{agent} only supports the Google Gemini protocol, which the local gateway can't convert to",
  apiOnly: "{agent} only supports the {only} API; this group uses {api} (the local gateway can convert it)",
  geminiGroup: "This group uses the Gemini protocol and only works with Gemini CLI",
  noSource: "No base URL and API key to copy; edit the group to fill them in first",
  useCurrent: "In use",
  useOn: "Connected",
  useOff: "Disabled",
  useAdding: "To be added",
  useRemoving: "To be removed",
  useNew: "New · not applied",
};

export default en;
