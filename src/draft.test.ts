import { describe, expect, it } from "vitest";
import type { AgentState, Model, Op, Provider, Setting } from "./api";
import {
  type Draft, type ViewProvider, agentsWithOps, deleteModel, deleteProvider, draftAfterWrite, fmtCtx, guessedModel, importProvider, keys, mergeExtra, opCount, opsToWrite,
  parseCtx, pendingTotal, providerModelCount, removeProvider, setModelVisible, setProviderEnabled, setSetting, settingOn, shouldAutoRestart, upsertModel, upsertProvider, viewModels, viewProviders,
  visibleCount, visibleModelCount, withOp,
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
