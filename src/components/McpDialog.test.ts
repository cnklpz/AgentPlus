import { describe, expect, it } from "vitest";
import { envRefs, mcpBlocked } from "./McpDialog";

describe("envRefs", () => {
  it("finds every variable syntax a Test fills in, once each", () => {
    expect(envRefs(["https://h/mcp?k=${API_KEY}", "Authorization: Bearer {env:TOKEN}", "-p $HOME %APPDATA% ${API_KEY:-x}"]))
      .toEqual(["API_KEY", "TOKEN", "HOME", "APPDATA"]);
  });
  it("ignores text without references", () => {
    expect(envRefs(["npx", "-y @m/fs", "price: 5$", "100%"])).toEqual([]);
  });
});

describe("mcpBlocked", () => {
  it("mirrors Antigravity's limits: stdio without a folder, or streamable HTTP", () => {
    expect(mcpBlocked("antigravity", "stdio", false)).toBeNull();
    expect(mcpBlocked("antigravity", "http", false)).toBeNull();
    expect(mcpBlocked("antigravity", "sse", false)).not.toBeNull();
    expect(mcpBlocked("antigravity", "stdio", true)).not.toBeNull();
  });
});
