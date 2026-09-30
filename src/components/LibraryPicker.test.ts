import { describe, expect, it } from "vitest";
import type { AgentState, ApiKind, LibEntry } from "../api";
import type { Group, UseState } from "../services";
import { USE_LABEL } from "../services";
import { libraryBlock } from "./LibraryPicker";

const agent = (id: string) => ({ id, name: id, installed: true, readonly: false }) as AgentState;
const group = (api: ApiKind, uses: [string, UseState][] = []): Group => ({
  key: `g-${api}`, name: "Relay", baseUrl: "https://relay.example.com/v1", api, keyFp: null, keyHint: null,
  lib: { id: "relay", name: "Relay", baseUrl: "https://relay.example.com/v1", api } as LibEntry,
  uses: uses.map(([a, state]) => ({ agent: agent(a), p: null, state, models: 0 })),
});

describe("libraryBlock", () => {
  it("lets a free entry be picked", () => {
    expect(libraryBlock(group("chat"), "opencode")).toBeNull();
    // Another agent using it doesn't matter.
    expect(libraryBlock(group("chat", [["kilo", "on"]]), "opencode")).toBeNull();
  });

  it("names the state of an entry the agent already has or is getting", () => {
    expect(libraryBlock(group("chat", [["opencode", "on"]]), "opencode")).toBe(USE_LABEL.on);
    expect(libraryBlock(group("chat", [["opencode", "adding"]]), "opencode")).toBe(USE_LABEL.adding);
  });

  it("keeps an entry whose removal is pending (undone on the Providers page)", () => {
    expect(libraryBlock(group("chat", [["opencode", "removing"]]), "opencode")).toBe(USE_LABEL.removing);
  });

  it("refuses a protocol the agent can't take", () => {
    expect(libraryBlock(group("chat"), "codex")).not.toBeNull();
    expect(libraryBlock(group("chat"), "workbuddy")).toBeNull();
    expect(libraryBlock(group("anthropic"), "workbuddy-ai")).not.toBeNull();
    expect(libraryBlock(group("gemini"), "gemini")).toBeNull();
    expect(libraryBlock(group("chat"), "gemini")).not.toBeNull();
  });
});
