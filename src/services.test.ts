import { describe, expect, it } from "vitest";
import type { GatewayRouteView, SyncSuggestion } from "./api";
import { API_LABEL, apiFor, gatewayCapable, gatewayPoolBase, gatewayPoolIds, gatewayRouteId, groupKey, hostKey, movedGatewayUrl, plainRoute, syncSuggestionId, syncSuggestionIds } from "./services";

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
  it("ignores trailing slashes, case and surrounding spaces", () => {
    expect(groupKey(" https://X.com/v1/// ", "chat", null)).toBe(groupKey("https://x.com/v1", "chat", null));
  });
  it("separates protocols and keys", () => {
    expect(groupKey("https://x.com/v1", "chat", "a")).not.toBe(groupKey("https://x.com/v1", "chat", "b"));
    expect(groupKey("https://x.com/v1", "chat", null)).not.toBe(groupKey("https://x.com/v1", "responses", null));
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
    ({ agent, title, detail: "", ops: keys.map((k) => [k, { op: "delete_provider", provider: k }]) });
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
  const sug = (keys: string[]): SyncSuggestion => ({ agent: "codex", title: "t", detail: "", ops: keys.map((k) => [k, { op: "delete_provider", provider: k }]) });
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
