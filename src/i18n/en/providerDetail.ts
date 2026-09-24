import type zh from "../zh/providerDetail";

const en: typeof zh = {
  statusDeleting: "To be deleted (on apply)",
  statusNew: "New (takes effect on apply)",
  statusIncompatible: "Incompatible",
  statusCurrent: "In use",
  statusSwitching: "Switch pending (takes effect on apply)",
  statusBuiltin: "Built-in",
  statusSwitchable: "Available",
  testAfterApply: "Apply the new provider before testing it",
  api: "API",
  connection: "Connection",
  viaGateway: "Via local gateway · {from} → {to}",
  gatewayMissing: "The gateway route it points to no longer exists. You can switch back to direct in Edit.",
  keyFilled: "Set",
  keyEmpty: "Not set",
  visibleOf: "{visible}/{total} visible",
  alsoIn: "Same service is also set up in",
  undoAdd: "Undo add",
  switchOnApply: "Switches to this provider on apply",
  setCurrent: "Make current provider",
  disableThis: "Disable this provider",
  enableThis: "Enable this provider",
  deleteInUse: "In use. Switch to another provider first.",
};

export default en;
