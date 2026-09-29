import { describe, expect, it } from "vitest";
import type { AgentState, GatewayRouteView, SyncSuggestion } from "./api";
import {
  API_LABEL, GATEWAY_KEY, type Group, type Use, agentLabel, apiFor, findRoute, freeAgents, gatewayCapable, gatewayEntry, gatewayPoolBase, gatewayPoolIds,
  gatewayRouteId, groupKey, hostKey, importKey, importMatches, keySource, movedHost, isGatewayHost, liveUses, mergeReplaced, movedGatewayUrl, newRouteId, plainRoute, splitStations, syncSuggestionId,
  syncSuggestionIds, tripped, useKey,
} from "./services";

describe("hostKey", () => {
  it.each([
    [null, ""],
    ["", ""],
    ["https://API.Example.com/v1", "api.example.com"],
    ["http://127.0.0.1:18650/x/v1", "127.0.0.1:18650"],
    ["https://例子.测试/v1", "xn--fsqu00a.xn--0zwm56d"],
    ["not a url", "not a url"],
    ["NOT A URL", "not a url"],
  ])("%j → %j", (u, k) => expect(hostKey(u)).toBe(k));
});

describe("groupKey", () => {
  it("ignores trailing path slashes, origin case, default ports and surrounding spaces", () => {
    expect(groupKey(" HTTPS://X.com:443/v1/// ", "chat", null)).toBe(groupKey("https://x.com/v1", "chat", null));
    expect(groupKey("https://X.com:443/v1///?team=A/", "chat", null)).toBe(groupKey("https://x.com/v1?team=A/", "chat", null));
  });
  it.each([
    ["https://x.com/TeamA/v1", "https://x.com/teama/v1"],
    ["https://x.com/v1?team=A", "https://x.com/v1?team=a"],
    ["https://x.com/v1?Team=A", "https://x.com/v1?team=A"],
    ["https://x.com/v1?team=A/", "https://x.com/v1?team=A"],
    ["https://User:Pass@x.com/v1", "https://user:Pass@x.com/v1"],
    ["https://User:Pass@x.com/v1", "https://User:pass@x.com/v1"],
    ["https://x.com/v1#A", "https://x.com/v1#a"],
    ["https://x.com/v1?", "https://x.com/v1"],
    ["https://x.com/v1#", "https://x.com/v1"],
  ])("keeps distinct endpoints separate: %s and %s", (a, b) => {
    expect(groupKey(a, "chat", "same-key")).not.toBe(groupKey(b, "chat", "same-key"));
  });
  it("separates protocols and keys", () => {
    expect(groupKey("https://x.com/v1", "chat", "a")).not.toBe(groupKey("https://x.com/v1", "chat", "b"));
    expect(groupKey("https://x.com/v1", "chat", null)).not.toBe(groupKey("https://x.com/v1", "responses", null));
  });
});

describe("movedHost", () => {
  it("names the new host when the origin changes", () => {
    expect(movedHost("https://relay-a.example/v1", "https://relay-b.example/v1")).toBe("relay-b.example");
    expect(movedHost("https://r.example/v1", "http://r.example/v1")).toBe("r.example");
    expect(movedHost("https://r.example/v1", "https://r.example:8443/v1")).toBe("r.example:8443");
  });
  it("counts a key with no address of its own (an official default) as moved", () => {
    expect(movedHost(null, "https://r.example")).toBe("r.example");
    expect(movedHost("", "https://r.example/v1")).toBe("r.example");
  });
  it("is null for the same origin, or a new address that can't be saved", () => {
    expect(movedHost("https://R.example/v1", " https://r.example:443/v2/ ")).toBeNull();
    expect(movedHost("https://r.example", "not a url")).toBeNull();
  });
});

describe("keySource", () => {
  const use = (id: string, over: Record<string, unknown> = {}) => ({ agent: { id: "claude" }, p: { id, editable: true, isNew: false, ...over } }) as unknown as Use;
  it("prefers a provider without a pending edit (its applied key belongs to its applied address)", () => {
    expect(keySource([use("edited", { isEdited: true }), use("plain")])?.p?.id).toBe("plain");
    expect(keySource([use("edited", { isEdited: true })])?.p?.id).toBe("edited");
    expect(keySource([use("new", { isNew: true }), use("ro", { editable: false })])).toBeUndefined();
  });
});

describe("importMatches", () => {
  const g = (url: string, api: string, fp: string | null, over: Partial<Group> = {}) =>
    ({ key: groupKey(url, api, fp), name: fp ?? "none", baseUrl: url, api, keyFp: fp, keyHint: null, lib: { id: "l" }, uses: [], ...over }) as Group;
  const stations = (...groups: Group[]) => [{ key: "s", name: "", host: "", baseUrl: null, builtin: false, groups }] as never;
  const url = "https://relay.example.com/v1";

  it("finds the group with the same address, protocol and key", () => {
    const same = g(url, "responses", "a");
    expect(importMatches(stations(g(url, "responses", "b"), same), "https://Relay.example.com/v1/", "responses", "a")).toEqual({ same, others: [] });
  });
  it("lists the groups at the address with another key, not other protocols or addresses", () => {
    const b = g(url, "responses", "b");
    const none = g(url, "responses", null);
    const r = importMatches(stations(b, none, g(url, "chat", "a"), g(`${url}/x`, "responses", "c")), url, "responses", "a");
    expect(r).toEqual({ same: null, others: [b, none] });
  });
  it("skips groups that are only being removed", () => {
    const gone = g(url, "responses", "a", { lib: null, uses: [{ state: "removing" } as Use] });
    expect(importMatches(stations(gone), url, "responses", "a")).toEqual({ same: null, others: [] });
  });
  it("does not match a different case-sensitive endpoint", () => {
    const original = g("https://relay.example/TeamA/v1", "responses", "a");
    const queried = g("https://relay.example/v1?team=A", "responses", "a");
    const all = stations(original, queried);
    expect(importMatches(all, "https://relay.example/teama/v1", "responses", "a")).toEqual({ same: null, others: [] });
    expect(importMatches(all, "https://relay.example/v1?team=a", "responses", "a")).toEqual({ same: null, others: [] });
    expect(importMatches(all, "https://RELAY.example:443/TeamA/v1/", "responses", "a")).toEqual({ same: original, others: [] });
  });
});

describe("gatewayRouteId", () => {
  const host = "127.0.0.1:18650";
  it.each([
    ["http://127.0.0.1:18650/relay/v1", "relay"],
    ["http://127.0.0.1:18650/relay/v1/", "relay"],
    ["http://localhost:18650/relay/v1", "relay"],
    ["HTTP://LOCALHOST:18650/relay/v1", "relay"],
  ])("%s → %s", (u, id) => expect(gatewayRouteId(u, host)).toBe(id));

  it.each([
    [null],
    ["http://127.0.0.1:18650/v1"], // unified entry, not a route
    ["http://127.0.0.1:18650/a+b/v1"], // combined address
    ["http://127.0.0.1:9999/relay/v1"], // other port
    ["http://127.0.0.1:18650/relay/v1/chat"],
    ["https://relay.example.com/relay/v1"],
  ])("%s → null", (u) => expect(gatewayRouteId(u, host)).toBeNull());

  it("accepts earlier ports from a list, and nothing without a gateway", () => {
    expect(gatewayRouteId("http://127.0.0.1:1000/r/v1", [host, "127.0.0.1:1000"])).toBe("r");
    expect(gatewayRouteId("http://127.0.0.1:18650/r/v1", null)).toBeNull();
  });
});

describe("gateway pools", () => {
  it("builds and parses pool addresses", () => {
    expect(gatewayPoolBase(18650, [])).toBe("http://127.0.0.1:18650/v1");
    expect(gatewayPoolBase(18650, ["a"])).toBe("http://127.0.0.1:18650/a/v1");
    expect(gatewayPoolBase(18650, ["a", "b"])).toBe("http://127.0.0.1:18650/a+b/v1");
    expect(gatewayPoolIds(gatewayPoolBase(18650, []))).toEqual([]);
    expect(gatewayPoolIds(gatewayPoolBase(18650, ["a", "b-2"]))).toEqual(["a", "b-2"]);
  });
  it("a single forward's address is not a pool", () => {
    expect(gatewayPoolIds("http://127.0.0.1:18650/a/v1")).toBeNull();
  });
  it.each([null, undefined, "", "https://example.com/v1", "http://127.0.0.1/v1", "http://127.0.0.1:1/a+/v1"])("%j is not a pool", (u) =>
    expect(gatewayPoolIds(u)).toBeNull(),
  );
});

describe("movedGatewayUrl", () => {
  it("moves forwards, the unified entry and pools to the new port", () => {
    expect(movedGatewayUrl("http://127.0.0.1:1000/r/v1", [1000], 2000)).toBe("http://127.0.0.1:2000/r/v1");
    expect(movedGatewayUrl("http://localhost:1000/v1", [1000], 2000)).toBe("http://127.0.0.1:2000/v1");
    expect(movedGatewayUrl("http://127.0.0.1:1000/a+b/v1", [1000], 2000)).toBe("http://127.0.0.1:2000/a+b/v1");
  });
  it("leaves other ports and other paths alone", () => {
    expect(movedGatewayUrl("http://127.0.0.1:3000/r/v1", [1000], 2000)).toBeNull();
    expect(movedGatewayUrl("http://127.0.0.1:1000/r/v1/models", [1000], 2000)).toBeNull();
    expect(movedGatewayUrl("https://relay.example.com:1000/r/v1", [1000], 2000)).toBeNull();
    expect(movedGatewayUrl(null, [1000], 2000)).toBeNull();
    expect(movedGatewayUrl(undefined, [], 2000)).toBeNull();
  });
});

describe("protocols", () => {
  it("labels known protocols and passes unknown ones through", () => {
    expect(API_LABEL.gemini).toBe("Gemini");
    expect((API_LABEL as Record<string, string>)["bedrock-converse"]).toBe("bedrock-converse");
  });
  it("single-protocol agents force their protocol", () => {
    expect(apiFor("codex", "chat")).toBe("responses");
    expect(apiFor("claude", "chat")).toBe("anthropic");
    expect(apiFor("opencode", "anthropic")).toBe("anthropic");
    expect(gatewayCapable("gemini")).toBe(false);
    expect(gatewayCapable("codex")).toBe(true);
  });
});

describe("gatewayEntry", () => {
  it("points a new entry at the gateway with the gateway key", () => {
    expect(gatewayEntry("http://127.0.0.1:18650/r/v1", "opencode", "chat", "Relay (gateway)", ["m1"])).toEqual({
      id: null, name: "Relay (gateway)", baseUrl: "http://127.0.0.1:18650/r/v1", api: "chat", apiKey: GATEWAY_KEY, models: ["m1"],
    });
  });
  it("gives Codex no model list (it shares one catalog)", () => {
    expect(gatewayEntry("http://127.0.0.1:18650/r/v1", "codex", "responses", "R", ["m1"]).models).toEqual([]);
  });
});

describe("gateway routes", () => {
  const r = (id: string, library: string, upstreamApi: "chat" | "responses", breaker: GatewayRouteView["breaker"] = null) =>
    ({ id, library, upstreamApi, breaker }) as GatewayRouteView;
  it("findRoute matches library and protocol", () => {
    const routes = [r("a", "l1", "chat"), r("b", "l1", "responses"), r("c", "l2", "chat")];
    expect(findRoute(routes, "l1", "responses")?.id).toBe("b");
    expect(findRoute(routes, "l2", "responses")).toBeUndefined();
    expect(findRoute(routes, undefined, "chat")).toBeUndefined();
    expect(findRoute(routes, null, "chat")).toBeUndefined();
  });
  it.each([
    ["OpenCode Zen", [], "opencode-zen"],
    ["  Relay!! A ", [], "relay-a"],
    ["智谱", [], "route"],
    ["", [], "route"],
    ["Relay", ["relay"], "relay-2"],
    ["Relay", ["relay", "relay-2", "relay-3"], "relay-4"],
    ["智谱", ["route"], "route-2"],
  ])("newRouteId(%j, taken %j) = %j", (name, taken, id) => expect(newRouteId(name, taken.map((x) => ({ id: x })))).toBe(id));
  it("mergeReplaced appends new pairs once", () => {
    expect(mergeReplaced(undefined, [["codex", "a"]])).toEqual([["codex", "a"]]);
    expect(mergeReplaced([["codex", "a"]], [["codex", "a"], ["claude", "a"], ["codex", "b"]])).toEqual([["codex", "a"], ["claude", "a"], ["codex", "b"]]);
    expect(mergeReplaced(null, [])).toEqual([]);
  });
  it("tripped: open or probing breakers only", () => {
    const b = (state: "closed" | "open" | "probe") => ({ state }) as NonNullable<GatewayRouteView["breaker"]>;
    expect(tripped(r("a", "l", "chat"))).toBeNull();
    expect(tripped(r("a", "l", "chat", b("closed")))).toBeNull();
    expect(tripped(r("a", "l", "chat", b("open")))?.state).toBe("open");
    expect(tripped(r("a", "l", "chat", b("probe")))?.state).toBe("probe");
  });
  it("isGatewayHost: localhost counts, other ports don't", () => {
    expect(isGatewayHost("localhost:18650", ["127.0.0.1:18650"])).toBe(true);
    expect(isGatewayHost("127.0.0.1:18651", ["127.0.0.1:18650", "127.0.0.1:18651"])).toBe(true);
    expect(isGatewayHost("127.0.0.1:1", "127.0.0.1:18650")).toBe(false);
    expect(isGatewayHost("127.0.0.1:18650", null)).toBe(false);
  });
});

describe("groups and uses", () => {
  const agent = (id: string, over: Partial<AgentState> = {}) => ({ id, name: id, installed: true, readonly: false, ...over }) as AgentState;
  const use = (a: AgentState, state: Use["state"], pid = "p"): Use => ({ agent: a, p: { id: pid } as Use["p"], state, models: 0 });
  const [x, y, z, ro, gone] = [agent("opencode"), agent("claude"), agent("qwen"), agent("zcode", { readonly: true }), agent("kimi", { installed: false })];
  const g = { uses: [use(x, "on"), use(y, "removing")] } as Group;
  it("liveUses leaves out removals", () => expect(liveUses(g).map((u) => u.agent.id)).toEqual(["opencode"]));
  it("freeAgents: writable agents not using the group (a removal frees it)", () => {
    expect(freeAgents([x, y, z, ro, gone], g).map((a) => a.id)).toEqual(["claude", "qwen"]);
    expect(freeAgents([x, ro], null).map((a) => a.id)).toEqual(["opencode"]);
  });
  it("useKey is agent and provider", () => expect(useKey(use(x, "on", "relay"))).toBe("opencode:relay"));
  it("importKey: the draft key of the copy importOp queues", () => {
    expect(importKey({ lib: { id: "e1" }, uses: [] } as unknown as Group)).toBe("pi:library:e1");
    const src = { agent: x, p: { id: "relay", isNew: false, editable: true, baseUrl: "https://r/v1" }, state: "on", models: 0 } as unknown as Use;
    expect(importKey({ lib: null, uses: [src] } as unknown as Group)).toBe("pi:opencode:relay");
  });
  it("splitStations keeps order", () => {
    const s = (key: string, builtin: boolean) => ({ key, builtin }) as never;
    const { relays, accounts } = splitStations([s("a", false), s("b", true), s("c", false)]);
    expect(relays.map((v: { key: string }) => v.key)).toEqual(["a", "c"]);
    expect(accounts.map((v: { key: string }) => v.key)).toEqual(["b"]);
  });
  it("agentLabel: product names, other ids as is", () => {
    expect(agentLabel("claude")).toBe("Claude Code");
    expect(agentLabel("codex@wsl")).toBe("codex@wsl");
    expect(agentLabel("codex-cleanup")).toBe("codex-cleanup");
  });
});

describe("plainRoute", () => {
  const view: GatewayRouteView = {
    id: "relay", name: "Relay", library: "lib-1", upstreamApi: "chat", modelMap: [["a", "b"]], enabled: true, weight: 100,
    replaced: [["codex", "relay"]],
    localBase: "http://127.0.0.1:18650/relay/v1", upstreamName: "Relay", upstreamUrl: "https://relay.example.com/v1", upstreamMissing: false,
    models: ["glm-5"], breaker: null,
  };
  it("keeps only what is saved", () => {
    expect(plainRoute(view)).toEqual({
      id: "relay", name: "Relay", library: "lib-1", upstreamApi: "chat", modelMap: [["a", "b"]], enabled: true, weight: 100, replaced: [["codex", "relay"]],
    });
  });
  it("leaves the view untouched", () => {
    plainRoute(view);
    expect(view.localBase).toBe("http://127.0.0.1:18650/relay/v1");
  });
});

describe("syncSuggestionId", () => {
  const sug = (agent: SyncSuggestion["agent"], keys: string[], title = "t"): SyncSuggestion =>
    ({ agent, title, detail: "", ops: keys.map((k) => [k, { op: "delete_provider", provider: k }]), lib: null, mcp: null, skill: null });
  const lib = (key: string): SyncSuggestion =>
    ({ agent: "library", title: "t", detail: "", ops: [], lib: { key, id: null, name: "n", baseUrl: "https://x/v1", api: "chat", models: [], keyFp: null }, mcp: null, skill: null });
  it("tells library changes apart by their key", () => {
    expect(syncSuggestionId(lib("lib:a|chat"))).not.toBe(syncSuggestionId(lib("lib:b|chat")));
    expect(syncSuggestionId(lib("lib:a|chat"))).toBe(syncSuggestionId(lib("lib:a|chat")));
  });
  it("does not depend on the (translated) text", () => {
    expect(syncSuggestionId(sug("codex", ["a"], "新增"))).toBe(syncSuggestionId(sug("codex", ["a"], "Add")));
  });
  it("tells agents and draft keys apart", () => {
    expect(syncSuggestionId(sug("codex", ["a"]))).not.toBe(syncSuggestionId(sug("claude", ["a"])));
    expect(syncSuggestionId(sug("codex", ["a"]))).not.toBe(syncSuggestionId(sug("codex", ["a", "b"])));
    expect(syncSuggestionId(sug("codex", []))).not.toBe(syncSuggestionId(sug("codex", ["a"])));
  });
});

describe("syncSuggestionIds", () => {
  const sug = (keys: string[]): SyncSuggestion => ({ agent: "codex", title: "t", detail: "", ops: keys.map((k) => [k, { op: "delete_provider", provider: k }]), lib: null, mcp: null, skill: null });
  it("keeps ids unique when two suggestions touch the same keys", () => {
    const ids = syncSuggestionIds([sug(["a"]), sug(["b"]), sug(["a"]), sug(["a"])]);
    expect(new Set(ids).size).toBe(4);
    expect(ids[0]).toBe(syncSuggestionId(sug(["a"])));
    expect(ids.slice(2)).toEqual([`${ids[0]}#2`, `${ids[0]}#3`]);
  });
  it("is stable for the same list", () => {
    const list = [sug(["a"]), sug(["a"])];
    expect(syncSuggestionIds(list)).toEqual(syncSuggestionIds(list));
    expect(syncSuggestionIds([])).toEqual([]);
  });
});

describe("syncSuggestionId for MCP", () => {
  it("uses the MCP change's key", () => {
    const s: SyncSuggestion = { agent: "library", title: "t", detail: "", ops: [], lib: null, mcp: { key: "mcp:gh", name: "gh", server: {} }, skill: null };
    expect(syncSuggestionId(s)).toBe("library\nmcp:gh");
  });
});
