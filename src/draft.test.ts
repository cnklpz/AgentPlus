import { describe, expect, it } from "vitest";
import type { AgentState, Issue, McpInput, McpServer, Model, Op, Provider, Setting } from "./api";
import {
  type Draft, type ViewProvider, agentsWithOps, codexDefaultModels, defaultModel, deleteModel, deleteProvider, draftAfterWrite, editProvider, fmtCtx, guessedModel, importProvider, issueStaged, keys, mergeExtra, opCount, opsToWrite, syncKeys,
  parseCtx, pendingTotal, providerModelCount, removeProvider, setDefaultModel, setModelVisible, setProviderEnabled, setSetting, setSettingIn, settingOn, excludedOn, settingValue, shouldAutoRestart, upsertModel, upsertProvider, viewModels, viewProviders,
  visibleCount, visibleModelCount, withOp, deleteMcp, mcpView, sameCore, setMcpEnabled, undoMcp, upsertMcp, writeOrder, pluginOn, setPluginEnabled,
} from "./draft";

const model = (id: string, over: Partial<Model> = {}): Model =>
  ({ id, visible: true, readonly: false, tags: [], ctx: null, name: null, context: null, deletable: true, ...over }) as Model;

const provider = (id: string, over: Partial<Provider> = {}): Provider =>
  ({
    id, name: id, baseUrl: `https://${id}.example.com/v1`, host: `${id}.example.com/v1`, apis: ["Chat"], builtin: false, enabled: true,
    compatible: true, reason: null, models: [], details: [], editable: true, api: "chat", hasKey: true, keyFp: "fp", keyHint: "••1234",
    officialAuth: false, ...over,
  }) as Provider;

const agent = (over: Partial<AgentState> = {}): AgentState =>
  ({ id: "opencode", providers: [], currentProvider: null, catalog: null, ...over }) as unknown as AgentState;

describe("fmtCtx", () => {
  it.each([
    [0, "0"],
    [999, "999"],
    [1000, "1K"],
    [131_072, "131K"],
    [200_000, "200K"],
    [999_499, "999K"],
    [999_500, "1M"],
    [999_999, "1M"],
    [1_000_000, "1M"],
    [1_048_576, "1M"],
    [1_500_000, "1.5M"],
    [2_097_152, "2M"],
    [10_000_000, "10M"],
  ])("%d → %s", (n, s) => expect(fmtCtx(n)).toBe(s));
});

describe("parseCtx", () => {
  it.each([
    ["128k", 128_000],
    ["128K", 128_000],
    [" 1m ", 1_000_000],
    ["1.5M", 1_500_000],
    ["200000", 200_000],
    ["200,000", 200_000],
    ["128 k", 128_000],
    ["0.5k", 500],
  ])("%j → %d", (s, n) => expect(parseCtx(s)).toBe(n));

  it.each(["", "   ", "abc", "-5", "1e6", "12kb", "1.2.3", "k", "1g", "∞"])("rejects %j", (s) => expect(parseCtx(s)).toBeNull());

  it("round-trips what fmtCtx prints (to its precision)", () => {
    for (const n of [1000, 32_000, 128_000, 200_000, 1_000_000, 2_000_000]) expect(parseCtx(fmtCtx(n))).toBe(n);
  });
});

describe("withOp", () => {
  it("adds, replaces and removes without mutating the input", () => {
    const d0 = {};
    const d1 = withOp(d0, "cur", { op: "set_current_provider", provider: "a" });
    expect(d0).toEqual({});
    const d2 = withOp(d1, "cur", null);
    expect(d2).toEqual({});
    expect(d1).toHaveProperty("cur");
  });

  it("removing a missing key is a no-op", () => {
    expect(withOp({}, "nope", null)).toEqual({});
  });
});

describe("codexDefaultModels", () => {
  const tag = (id: string) => [{ id, label: id }];
  it("leaves out custom models and the ones Codex hides", () => {
    const cat = [model("a"), model("daybreak", { tags: tag("codex-hidden") }), model("old", { tags: tag("codex-hidden"), visible: false }), model("mine", { tags: tag("custom") })];
    expect(codexDefaultModels(cat)).toEqual(["a"]);
  });
  it("falls back to what the catalog shows when Codex's hidden ones weren't noted", () => {
    expect(codexDefaultModels([model("a"), model("b", { visible: false }), model("mine", { tags: tag("custom") })])).toEqual(["a"]);
  });
  it("ticks every built-in model rather than none", () => {
    expect(codexDefaultModels([model("a", { visible: false }), model("b", { visible: false })])).toEqual(["a", "b"]);
    expect(codexDefaultModels([model("mine", { tags: tag("custom") })])).toEqual([]);
  });
});

describe("defaultModel / setDefaultModel", () => {
  const models = [model("a"), model("b", { tags: [{ id: "role:default", label: "Default" }] }), model("c")];

  it("reads the saved default from the tag, or none", () => {
    expect(defaultModel({}, "p", models)).toEqual({ id: "b", pending: false });
    expect(defaultModel({}, "p", [model("a")])).toEqual({ id: null, pending: false });
  });

  it("queues a new default and drops it when the saved one is picked again", () => {
    const d1 = setDefaultModel({}, "p", models, "c");
    expect(d1[keys.roles("p")]).toEqual({ op: "set_model_roles", provider: "p", roles: { default: "c" } });
    expect(defaultModel(d1, "p", models)).toEqual({ id: "c", pending: true });
    const d2 = setDefaultModel(d1, "p", models, "a");
    expect(defaultModel(d2, "p", models)).toEqual({ id: "a", pending: true });
    expect(setDefaultModel(d2, "p", models, "b")).toEqual({});
  });

  it("keeps other providers' changes", () => {
    const other = setDefaultModel({}, "q", models, "a");
    expect(Object.keys(setDefaultModel(other, "p", models, "c")).sort()).toEqual([keys.roles("p"), keys.roles("q")].sort());
  });
});

describe("setSetting", () => {
  const s = { key: "tags", kind: "chips", value: ["a", "b"] } as unknown as Setting;
  it("drops the op when the value goes back to the original, even reordered", () => {
    const d = setSetting({}, s, ["c"]);
    expect(Object.keys(d)).toEqual([keys.setting("tags")]);
    expect(setSetting(d, s, ["b", "a"])).toEqual({});
  });
  it("treats arrays of different length as different", () => {
    expect(setSetting({}, s, ["a"])).not.toEqual({});
    expect(setSetting({}, s, [])).not.toEqual({});
  });
  it("keeps a reordered list setting (one entry per line: order matters)", () => {
    const l = { key: "instructions", kind: "list", value: ["a", "b"] } as unknown as Setting;
    const d = setSetting({}, l, ["b", "a"]);
    expect(d[keys.setting("instructions")]).toEqual({ op: "set_setting", key: "instructions", value: ["b", "a"] });
    expect(setSetting(d, l, ["a", "b"])).toEqual({});
    expect(setSetting({}, l, ["a", "b", "c"])).not.toEqual({});
  });
});

describe("setSettingIn", () => {
  const sw = (key: string, value: boolean, excludes?: string[]) => ({ key, kind: "bool", value, excludes }) as unknown as Setting;
  const full = sw("full_names", true, ["short_names"]);
  const short = sw("short_names", false, ["full_names"]);
  const all = [full, short, sw("quota_unlock", true)];
  it("turns off the switch it excludes", () => {
    const d = setSettingIn({}, all, short, true);
    expect(settingValue(short, d)).toBe(true);
    expect(settingValue(full, d)).toBe(false);
    expect(settingValue(all[2], d)).toBe(true);
  });
  it("lists the excluded switches that are on", () => {
    expect(excludedOn({}, all, short)).toEqual([full]);
    expect(excludedOn(setSettingIn({}, all, full, false), all, short)).toEqual([]);
    expect(excludedOn({}, all, all[2])).toEqual([]);
  });
  it("switching back restores the pending state both ways", () => {
    const d = setSettingIn(setSettingIn({}, all, short, true), all, full, true);
    expect(d).toEqual({});
  });
  it("turning it off before applying puts the excluded switch back", () => {
    const d = setSettingIn(setSettingIn({}, all, short, true), all, short, false);
    expect(d).toEqual({});
  });
  it("turning an applied switch off leaves the others alone", () => {
    const d = setSettingIn({}, all, full, false);
    expect(Object.keys(d)).toEqual([keys.setting("full_names")]);
    const a = [sw("full_names", false, ["short_names"]), sw("short_names", true, ["full_names"])];
    expect(Object.keys(setSettingIn({}, a, a[1], false))).toEqual([keys.setting("short_names")]);
  });
  it("an already-off excluded switch gets no op", () => {
    const off = [sw("full_names", false, ["short_names"]), short];
    expect(Object.keys(setSettingIn({}, off, short, true))).toEqual([keys.setting("short_names")]);
  });
  it("preserves a manual disable made before toggling the other switch", () => {
    const manual = setSettingIn({}, all, full, false);
    const d = setSettingIn(setSettingIn(manual, all, short, true), all, short, false);
    expect(d).toEqual(manual);
    expect(settingValue(full, d)).toBe(false);
    expect(settingValue(short, d)).toBe(false);
  });
  it("restores a pending enable displaced by the other switch", () => {
    const off = [sw("full_names", false, ["short_names"]), short];
    const manual = setSettingIn({}, off, off[0], true);
    const d = setSettingIn(setSettingIn(manual, off, short, true), off, short, false);
    expect(d).toEqual(manual);
    expect(settingValue(off[0], d)).toBe(true);
  });
  it("keeps undo information across unrelated edits and repeated enables", () => {
    const enabled = setSettingIn({}, all, short, true);
    const repeated = setSettingIn(enabled, all, short, true);
    const edited = setSettingIn(repeated, all, all[2], false);
    expect(setSettingIn(edited, all, short, false)).toEqual(setSetting({}, all[2], false));
    expect(JSON.stringify(enabled)).not.toContain("before");
  });
  it("does not restore a linked setting after a later explicit edit", () => {
    const enabled = setSettingIn({}, all, short, true);
    const edited = setSetting(enabled, full, false);
    const d = setSettingIn(edited, all, short, false);
    expect(d).toEqual(setSetting({}, full, false));
  });
});

describe("settingOn / shouldAutoRestart", () => {
  const st = (auto: boolean, running = true) =>
    agent({ running, settings: [{ key: "auto_restart", value: auto } as Setting, { key: "x", value: "on" } as Setting] });
  const toggle: Op = { op: "set_setting", key: "auto_restart", value: true };
  const other: Op = { op: "set_current_provider", provider: "p" };
  it("reads only a boolean true", () => {
    expect(settingOn(st(true), "auto_restart")).toBe(true);
    expect(settingOn(st(false), "auto_restart")).toBe(false);
    expect(settingOn(st(true), "x")).toBe(false);
    expect(settingOn(st(true), "missing")).toBe(false);
  });
  it("restarts only when something besides the switch itself was written", () => {
    expect(shouldAutoRestart(st(true), [toggle])).toBe(false);
    expect(shouldAutoRestart(st(true), [])).toBe(false);
    expect(shouldAutoRestart(st(true), [toggle, other])).toBe(true);
    expect(shouldAutoRestart(st(true), [{ op: "set_setting", key: "fast", value: true }])).toBe(true);
  });
  it("never when the switch is off or the agent isn't running", () => {
    expect(shouldAutoRestart(st(false), [other])).toBe(false);
    expect(shouldAutoRestart(st(true, false), [other])).toBe(false);
  });
});

describe("guessedModel", () => {
  it("fills in what the catalogs know, and nothing for unknown models", () => {
    const g = { context: 64000, extra: { "/reasoning": true }, matched: "acme", source: "modelsDev" as const };
    expect(guessedModel("acme", g)).toEqual({ id: "acme", name: null, context: 64000, extra: { "/reasoning": true } });
    expect(guessedModel("x", undefined)).toEqual({ id: "x", name: null, context: null });
    expect(guessedModel("y", { ...g, extra: {} })).toEqual({ id: "y", name: null, context: 64000 });
  });
});

describe("op builders", () => {
  it("delete ops use the draft keys the views read", () => {
    expect(deleteProvider({}, "p")).toEqual({ [keys.deleteProvider("p")]: { op: "delete_provider", provider: "p" } });
    expect(deleteModel({}, "p", "m")).toEqual({ [keys.deleteModel("p", "m")]: { op: "delete_model", provider: "p", model: "m" } });
    expect(viewProviders(agent({ providers: [provider("p")] }), deleteProvider({}, "p"))[0].isDeleted).toBe(true);
  });
  it("setModelVisible drops the op when back to the applied value", () => {
    const m = model("m", { visible: true });
    const d = setModelVisible({}, "p", m, false);
    expect(d).toEqual({ [keys.visible("p", "m")]: { op: "set_model_visible", provider: "p", model: "m", visible: false } });
    expect(setModelVisible(d, "p", m, true)).toEqual({});
    expect(setModelVisible({}, "p", m, true)).toEqual({});
  });
  it("importProvider uses the import key and keeps only the given fields", () => {
    const d = importProvider({}, { fromAgent: "library", provider: "e1", api: "chat", name: "Relay" });
    expect(d).toEqual({ "pi:library:e1": { op: "import_provider", fromAgent: "library", provider: "e1", api: "chat", name: "Relay" } });
    const labeled = importProvider({}, { fromAgent: "claude", provider: "p", api: "anthropic", name: "P", label: "Claude Code" });
    expect(labeled[keys.importProvider("claude", "p")]).toEqual({ op: "import_provider", fromAgent: "claude", provider: "p", api: "anthropic", name: "P", label: "Claude Code" });
  });
  it("setProviderEnabled drops the op when back to the applied value", () => {
    const off = provider("p", { enabled: false });
    expect(setProviderEnabled({}, off, false)).toEqual({});
    const d = setProviderEnabled({}, provider("p"), false);
    expect(d).toEqual({ [keys.enabled("p")]: { op: "set_provider_enabled", provider: "p", enabled: false } });
    expect(setProviderEnabled(d, provider("p"), true)).toEqual({});
  });
  it("removeProvider drops a pending new entry instead of deleting its draft key", () => {
    const st = agent({ providers: [provider("old")] });
    let d = upsertProvider({}, { id: null, name: "GW", baseUrl: "http://127.0.0.1:1/v1", api: "anthropic", apiKey: "", models: [] }, keys.gatewayProvider("x"));
    const fresh = viewProviders(st, d).find((p) => p.isNew)!;
    expect(fresh.id).toBe("pu:gw-x");
    d = removeProvider(d, fresh);
    expect(d).toEqual({});
    expect(Object.values(d).some((op) => op.op === "delete_provider")).toBe(false);
    // An applied provider gets a pending delete.
    const old = viewProviders(st, d)[0];
    expect(removeProvider(d, old)).toEqual(deleteProvider({}, "old"));
  });
  it("key formats", () => {
    expect(keys.importProvider("library", "e1")).toBe("pi:library:e1");
    expect(keys.gatewayProvider("relay")).toBe("pu:gw-relay");
  });
});

describe("pending counts", () => {
  const drafts: Record<string, Draft> = { a: { x: { op: "set_current_provider", provider: "p" }, y: { op: "delete_provider", provider: "q" } }, b: {} };
  it("counts ops per draft and in total", () => {
    expect(opCount(drafts.a)).toBe(2);
    expect(opCount(drafts.b)).toBe(0);
    expect(opCount(undefined)).toBe(0);
    expect(pendingTotal(drafts)).toBe(2);
    expect(pendingTotal({})).toBe(0);
  });
  it("lists the agents with changes, in order", () => {
    expect(agentsWithOps([{ id: "c" }, { id: "b" }, { id: "a" }], drafts)).toEqual([{ id: "a" }]);
  });
});

describe("mergeExtra", () => {
  it("overlays edits and deletes null fields", () => {
    expect(mergeExtra({ a: 1, b: "x" }, { b: null, c: true })).toEqual({ a: 1, c: true });
  });
  it("handles missing base and edit", () => {
    expect(mergeExtra(undefined, undefined)).toEqual({});
    expect(mergeExtra(null as never, { a: 1 })).toEqual({ a: 1 });
  });
});

describe("viewProviders", () => {
  it("labels every protocol, including Gemini", () => {
    const st = agent({ providers: [provider("p")] });
    const d = upsertProvider({}, { id: "p", name: "P", baseUrl: "https://g.example.com/v1beta", api: "gemini", apiKey: null, models: [], keyFromLibrary: null, officialAuth: null });
    const [p] = viewProviders(st, d);
    expect(p.apis).toEqual(["Gemini"]);
    expect(p.isEdited).toBe(true);
  });

  it("shows unknown protocols of a copied provider as-is", () => {
    const d = { x: { op: "import_provider", fromAgent: "pi", provider: "b", api: "bedrock-converse" } } as never;
    const out = viewProviders(agent(), d, true);
    expect(out[0].apis).toEqual(["bedrock-converse"]);
  });

  it("a new key hides the old fingerprint until applied", () => {
    const st = agent({ providers: [provider("p")] });
    const d = upsertProvider({}, { id: "p", name: "p", baseUrl: "https://p.example.com/v1", api: "chat", apiKey: "sk-new", models: [], keyFromLibrary: null, officialAuth: null });
    const [p] = viewProviders(st, d);
    expect(p.keyFp).toBeNull();
    expect(p.keyHint).toBeNull();
  });

  it("keeps an unparsable base URL as the host", () => {
    const d = upsertProvider({}, { id: null, name: "n", baseUrl: "not a url", api: "chat", apiKey: null, models: ["m"], keyFromLibrary: null, officialAuth: null }, "pu:new-1");
    const [p] = viewProviders(agent(), d);
    expect(p.host).toBe("not a url");
    expect(p.isNew).toBe(true);
    expect(p.models.map((m) => m.id)).toEqual(["m"]);
  });

  it("marks a Codex provider that isn't Responses as incompatible", () => {
    const d = upsertProvider({}, { id: null, name: "n", baseUrl: "https://x/v1", api: "chat", apiKey: null, models: [], keyFromLibrary: null, officialAuth: null }, "k");
    expect(viewProviders(agent({ id: "codex" } as never), d)[0].compatible).toBe(false);
  });
});

describe("config issues", () => {
  const issue = (kind: Issue["kind"], p: string | null): Issue => ({ kind, provider: p, text: "" });

  it("edits a provider with its own values, merged into a pending edit", () => {
    const p = provider("p", { api: "responses", officialAuth: true });
    const d = editProvider({}, p, { officialAuth: false });
    expect(d[keys.upsertProvider("p")]).toEqual({ op: "upsert_provider", provider: { id: "p", name: "p", baseUrl: "https://p.example.com/v1", api: "responses", apiKey: null, models: [], officialAuth: false } });
    const typed = upsertProvider({}, { id: "p", name: "Renamed", baseUrl: "https://p2/v1", api: "responses", apiKey: "sk-new", models: [], officialAuth: null });
    const merged = editProvider(typed, p, { officialAuth: false });
    expect(merged[keys.upsertProvider("p")]).toMatchObject({ provider: { name: "Renamed", baseUrl: "https://p2/v1", apiKey: "sk-new", officialAuth: false } });
    // A provider without an address is edited with an empty one (the backend then says why it can't).
    expect(editProvider({}, provider("q", { baseUrl: null }))[keys.upsertProvider("q")]).toMatchObject({ provider: { baseUrl: "" } });
  });

  it("knows which pending change deals with which issue", () => {
    const p = provider("p", { api: "responses" });
    const edit = editProvider({}, p);
    expect(issueStaged({}, issue("key-elsewhere", "p"))).toBe(false);
    expect(issueStaged(edit, issue("key-elsewhere", "p"))).toBe(true);
    expect(issueStaged(edit, issue("key-elsewhere", "other"))).toBe(false);
    expect(issueStaged(edit, issue("key-missing", "p"))).toBe(false);
    expect(issueStaged(editProvider({}, p, { apiKey: "sk" }), issue("key-missing", "p"))).toBe(true);
    expect(issueStaged(editProvider({}, p, { keyFromLibrary: "lib-1" }), issue("key-missing", "p"))).toBe(true);
    expect(issueStaged(edit, issue("api-key-sign-in", "p"))).toBe(false);
    expect(issueStaged(editProvider({}, p, { officialAuth: false }), issue("api-key-sign-in", "p"))).toBe(true);
    expect(issueStaged(deleteProvider({}, "p"), issue("key-missing", "p"))).toBe(true);
    expect(issueStaged(edit, issue("not-signed-in", null))).toBe(false);
  });

  it("syncs only the providers whose key is kept elsewhere, once", () => {
    const st = agent({
      id: "codex",
      providers: [provider("a", { api: "responses" }), provider("b", { api: "responses" }), provider("c", { api: "responses" })],
      issues: [issue("key-elsewhere", "a"), issue("key-missing", "b"), issue("key-elsewhere", "c"), issue("key-elsewhere", "gone")],
    } as never);
    const pending = upsertProvider({}, { id: "c", name: "C2", baseUrl: "https://c2/v1", api: "responses", apiKey: null, models: [], officialAuth: null });
    const d = syncKeys(pending, st);
    expect(Object.keys(d).sort()).toEqual([keys.upsertProvider("a"), keys.upsertProvider("c")]);
    expect(d[keys.upsertProvider("c")]).toBe(pending[keys.upsertProvider("c")]);
    expect(syncKeys(d, st)).toEqual(d);
    expect(syncKeys({}, agent())).toEqual({});
  });
});

describe("viewModels / visibleCount", () => {
  it("adds new models, overlays edits, marks deletions", () => {
    let d = upsertModel({}, "p", { id: "m1", name: "M1", context: 128_000, extra: {} } as never);
    d = upsertModel(d, "p", { id: "new", name: null, context: null, extra: {} } as never);
    d = withOp(d, keys.deleteModel("p", "m2"), { op: "delete_model", provider: "p", model: "m2" } as never);
    const out = viewModels("p", [model("m1"), model("m2")], d);
    expect(out.map((m) => [m.id, !!m.isEdited, !!m.isDeleted, !!m.isNew])).toEqual([
      ["m1", true, false, false],
      ["m2", false, true, false],
      ["new", false, false, true],
    ]);
    expect(out[0].ctx).toBe("128K");
  });

  it("ignores model edits for other providers", () => {
    const d = upsertModel({}, "other", { id: "x", name: null, context: null, extra: {} } as never);
    expect(viewModels("p", [], d)).toEqual([]);
  });

  it("counts visible models of enabled providers only", () => {
    const st = agent({
      providers: [
        provider("a", { models: [model("1"), model("2", { visible: false })] }),
        provider("b", { enabled: false, models: [model("3")] }),
      ],
    });
    expect(visibleCount(st, {})).toBe(1);
    expect(visibleCount(st, withOp({}, keys.enabled("b"), { op: "set_provider_enabled", provider: "b", enabled: true } as never))).toBe(2);
  });

  it("counts zero for an agent with nothing", () => {
    expect(visibleCount(agent(), {})).toBe(0);
  });

  it("per provider: leaves out hidden and pending-delete models, keeps pending shows and new models", () => {
    const models = [model("1"), model("2", { visible: false }), model("3")];
    let d = deleteModel({}, "p", "3");
    expect(visibleModelCount("p", models, d)).toBe(1);
    d = setModelVisible(d, "p", models[1], true);
    d = upsertModel(d, "p", { id: "4", name: null, context: null } as never);
    expect(visibleModelCount("p", models, d)).toBe(3);
    // A new provider (not applied) offers everything it lists.
    const fresh = { ...provider("n", { models: [model("a", { visible: false }), model("b")] }), isNew: true } as ViewProvider;
    expect(providerModelCount(fresh, {})).toBe(2);
    expect(providerModelCount(provider("p", { models }) as ViewProvider, d)).toBe(3);
  });
});

describe("opsToWrite", () => {
  const codex = agent({ id: "codex", fixedPrompt: true } as never);
  const switchTo = (p: string) => ({ [keys.cur()]: { op: "set_current_provider", provider: p } }) as Draft;

  it("adds the fixed id when Codex switches provider", () => {
    expect(opsToWrite(codex, switchTo("relay")).slice(-1)[0]).toEqual({ op: "set_setting", key: "fixed_id", value: true });
  });
  it("not for the official login", () => {
    expect(opsToWrite(codex, switchTo("openai"))).toHaveLength(1);
  });
  it("not when the user set it explicitly", () => {
    const d = { ...switchTo("relay"), [keys.setting("fixed_id")]: { op: "set_setting", key: "fixed_id", value: false } } as never;
    expect(opsToWrite(codex, d)).toHaveLength(2);
  });
  it("not for other agents or unknown state", () => {
    expect(opsToWrite(agent({ fixedPrompt: true } as never), switchTo("relay"))).toHaveLength(1);
    expect(opsToWrite(undefined, switchTo("relay"))).toHaveLength(1);
  });
});

describe("draftAfterWrite", () => {
  const a = { op: "delete_provider", provider: "a" } as const;
  const b = { op: "delete_provider", provider: "b" } as const;
  const c = { op: "delete_provider", provider: "c" } as const;

  it("drops everything that was written", () => {
    const sent: Draft = { a, b };
    expect(draftAfterWrite(sent, sent)).toEqual({});
  });
  it("keeps changes added or edited while writing", () => {
    const sent: Draft = { a, b };
    const b2 = { op: "delete_provider", provider: "b2" } as const;
    expect(draftAfterWrite({ a, b: b2, c }, sent)).toEqual({ b: b2, c });
  });
  it("does not bring back what was undone meanwhile", () => {
    expect(draftAfterWrite({ b }, { a, b })).toEqual({});
  });
  it("handles an empty draft", () => {
    expect(draftAfterWrite({}, {})).toEqual({});
    expect(draftAfterWrite({ c }, {})).toEqual({ c });
  });
});

describe("MCP drafts", () => {
  const srv = (name: string, over: Partial<McpServer> = {}): McpServer => ({
    name, transport: "stdio", command: "npx", args: ["-y", name], cwd: null, url: null, env: [], headers: [], enabled: true, stashed: false, extra: {}, sig: `s-${name}`, ...over,
  });
  const input = (name: string, over: Partial<McpInput> = {}): McpInput => ({
    name, transport: "stdio", command: "npx", args: ["-y", name], cwd: null, url: null, env: [], headers: [], enabled: true, ...over,
  });
  const none = () => undefined;

  it("toggling back to the config's state drops the change", () => {
    let d = setMcpEnabled({}, "fs", false, true);
    expect(d[keys.mcpEnabled("fs")]).toEqual({ op: "set_mcp_enabled", name: "fs", enabled: false });
    d = setMcpEnabled(d, "fs", true, true);
    expect(d).toEqual({});
  });

  it("a pending server carries its own switch and just goes away when removed", () => {
    let d = upsertMcp({}, input("new"));
    d = setMcpEnabled(d, "new", false, null);
    expect(Object.keys(d)).toEqual([keys.mcpUpsert("new")]);
    const op = d[keys.mcpUpsert("new")];
    expect(op.op === "upsert_mcp" && op.server.enabled).toBe(false);
    expect(deleteMcp(d, "new", false)).toEqual({});
  });

  it("removing a pending rename removes the old name", () => {
    const d = upsertMcp(setMcpEnabled({}, "old", false, true), input("new", { replaces: "old", from: ["claude", "old"] }));
    expect(Object.keys(d)).toEqual([keys.mcpUpsert("new")]);
    expect(deleteMcp(d, "new", false)).toEqual({ [keys.mcpDelete("old")]: { op: "delete_mcp", name: "old" } });
    expect(undoMcp(d, "new")).toEqual({});
  });

  it("the view shows pending changes and keeps a copy with its source", () => {
    const servers = [srv("a"), srv("b"), srv("c"), srv("old")];
    const src = srv("x", { sig: "s-src" });
    let d = deleteMcp({}, "a", true);
    d = setMcpEnabled(d, "b", false, true);
    d = upsertMcp(d, input("x", { from: ["codex", "x"] }));
    d = upsertMcp(d, input("renamed", { replaces: "old", command: "uvx" }));
    const v = mcpView(servers, d, (from) => (from[0] === "codex" ? src : undefined));
    const by = Object.fromEntries(v.map((x) => [x.s.name, x]));
    expect(by.a.pending).toBe("deleted");
    expect([by.b.pending, by.b.s.enabled]).toEqual(["toggled", false]);
    expect(by.c.pending).toBeNull();
    expect(by.old).toBeUndefined();
    expect([by.renamed.pending, by.renamed.exists]).toEqual(["edited", false]);
    // Copied unchanged: groups with the server it came from.
    expect([by.x.pending, by.x.s.sig]).toEqual(["new", "s-src"]);
    expect(mcpView([], upsertMcp({}, input("y", { command: "other" })), none)[0].s.sig).toMatch(/^draft:/);
  });
});

describe("writeOrder", () => {
  it("writes agents that remove MCP servers last", () => {
    const a = [{ id: "codex" }, { id: "claude" }, { id: "zcode" }];
    const drafts: Record<string, Draft> = {
      codex: deleteMcp({}, "fs", true),
      zcode: upsertMcp({}, { name: "fs", transport: "stdio", command: "x", args: [], cwd: null, url: null, env: [], headers: [], enabled: true, from: ["codex", "fs"] }),
    };
    expect(writeOrder(a, drafts).map((x) => x.id)).toEqual(["claude", "zcode", "codex"]);
  });
});

describe("sameCore", () => {
  it("compares what runs on the values the page shows", () => {
    const s: McpServer = { name: "g", transport: "stdio", command: "npx", args: ["-y", "vk"], cwd: null, url: null, env: [{ key: "T", value: "••••1234", secret: true }], headers: [], enabled: true, stashed: false, extra: { timeout: 1 }, sig: "x" };
    const i: McpInput = { name: "g", transport: "stdio", command: "npx", args: ["-y", "vk"], cwd: null, url: null, env: [{ key: "T", value: "••••1234" }], headers: [], enabled: true };
    expect(sameCore(i, s)).toBe(true);
    expect(sameCore({ ...i, args: ["vk"] }, s)).toBe(false);
  });
});

describe("plugin switches", () => {
  const p = { id: "latex@openai-bundled", name: "LaTeX", description: null, version: null, source: null, enabled: false, locked: null };
  it("a switch back to the config's state drops the change", () => {
    const d = setPluginEnabled({}, p, true);
    expect(d[keys.plugin(p.id)]).toEqual({ op: "set_plugin_enabled", plugin: p.id, enabled: true });
    expect(pluginOn(p, d)).toBe(true);
    expect(setPluginEnabled(d, p, false)).toEqual({});
  });
});
