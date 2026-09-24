import type zh from "../zh/providersHub";

const en: typeof zh = {
  intro: "Providers are grouped into relays by service URL; a relay can have several groups (different protocols, paths or API keys). Manage URLs and API keys here, and set models in each agent's \"Model list\". Current environment: {env}",
  testLatency: "Test latency",
  filterAll: "All",
  filterUsed: "In use",
  filterIdle: "Unused",
  searchPlaceholder: "Search by name, URL or group",
  notAdded: "Not added",
  agentState: "{agent}: {state}",
  moreGroups: "{n} more group|{n} more groups",
  groupCount: "{n} group|{n} groups",
  inLibraryTitle: "Saved in the provider library; can be added to any agent",
  inLibrary: "In library: {n}",
  notInLibraryTitle: "Only in agent configs; editing it saves it to the provider library",
  notInLibrary: "Not in library",
  addCardHint: "Enter the URL and API key once, then add it to {agents} as needed",
  eachAgent: "any agent",
  noMatch: "No matching providers",
  accounts: "Account sign-in · built into each agent",
};

export default en;
