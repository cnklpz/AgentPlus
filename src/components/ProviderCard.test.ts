import { describe, expect, it } from "vitest";
import { testUrl } from "./ProviderCard";

describe("testUrl", () => {
  it("prefers the provider's own address", () => {
    expect(testUrl({ baseUrl: "https://relay.example.com/v1", probeUrl: "https://api.openai.com/v1" })).toBe("https://relay.example.com/v1");
  });
  it("falls back to a built-in provider's official endpoint", () => {
    expect(testUrl({ baseUrl: null, probeUrl: "https://chatgpt.com/backend-api/codex" })).toBe("https://chatgpt.com/backend-api/codex");
  });
  it("is null when there is nothing to measure", () => {
    expect(testUrl({ baseUrl: null, probeUrl: null })).toBeNull();
    expect(testUrl({ baseUrl: null })).toBeNull();
  });
});
