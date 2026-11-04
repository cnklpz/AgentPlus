import { describe, expect, it } from "vitest";
import type { AgentState, LibEntry, Provider } from "../api";
import { buckets, summarySources } from "./NotifyPage";

describe("notification chart buckets", () => {
  const now = new Date(2026, 10, 4, 15, 20).getTime();
  const hour = Math.floor(now / 3600_000) * 3600;

  it("puts the last 24 hours in order, ending with the current one", () => {
    const b = buckets([{ t: hour, turns: 3, failed: 1, agents: {}, lastMs: 0 }, { t: hour - 23 * 3600, turns: 2, failed: 0, agents: {}, lastMs: 0 }, { t: hour - 24 * 3600, turns: 9, failed: 0, agents: {}, lastMs: 0 }], now, "day");
    expect(b).toHaveLength(24);
    expect(b[23]).toEqual({ t: hour, turns: 3, failed: 1 });
    expect(b[0]).toEqual({ t: hour - 23 * 3600, turns: 2, failed: 0 });
    expect(b.reduce((n, x) => n + x.turns, 0)).toBe(5);
  });

  it("sums hours into the last 7 local days", () => {
    const today = new Date(2026, 10, 4).getTime() / 1000;
    const b = buckets([
      { t: today + 3600, turns: 2, failed: 1, agents: {}, lastMs: 0 },
      { t: today + 7200, turns: 1, failed: 0, agents: {}, lastMs: 0 },
      { t: today - 3600, turns: 4, failed: 0, agents: {}, lastMs: 0 },
      { t: today - 7 * 86400, turns: 8, failed: 0, agents: {}, lastMs: 0 },
    ], now, "week");
    expect(b).toHaveLength(7);
    expect(b[6]).toEqual({ t: today, turns: 3, failed: 1 });
    expect(b[5].turns).toBe(4);
    expect(b.reduce((n, x) => n + x.turns, 0)).toBe(7);
  });
});

describe("summary providers", () => {
  const prov = (id: string, baseUrl: string | null, enabled = true) => ({ id, name: id.toUpperCase(), baseUrl, host: "h", enabled, models: [{ id: `${id}-m` }] }) as unknown as Provider;
  it("lists agents' callable providers, then the library", () => {
    const agents = [{ id: "codex", name: "Codex", providers: [prov("klpz", "https://x/v1"), prov("openai", null), prov("off", "https://y", false)] }] as unknown as AgentState[];
    const lib = [{ id: "lib1", name: "Lib", hasKey: false, models: ["a", "b"] }] as unknown as LibEntry[];
    const s = summarySources(agents, lib);
    expect(s.map((x) => [x.value, x.group, x.models])).toEqual([
      ['["codex","klpz"]', "Codex", ["klpz-m"]],
      ['["library","lib1"]', "供应商库", ["a", "b"]],
    ]);
    expect(summarySources([], [])).toEqual([]);
  });
});
