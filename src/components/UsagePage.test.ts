import { describe, expect, it } from "vitest";
import { bucketTimes, cacheRate, niceMax, parseDraft, rangeWindow } from "./UsagePage";

const sec = (d: Date) => Math.floor(d.getTime() / 1000);

describe("rangeWindow", () => {
  const now = new Date(2026, 10, 11, 15, 30, 0);
  it("today runs from local midnight in hours", () => {
    const w = rangeWindow("today", now);
    expect(w.from).toBe(sec(new Date(2026, 10, 11)));
    expect(w.bucket).toBe("hour");
    expect(w.to).toBeGreaterThan(sec(now));
  });
  it("7 days includes today and the six days before", () => {
    expect(rangeWindow("7d", now).from).toBe(sec(new Date(2026, 10, 5)));
    expect(rangeWindow("7d", now).bucket).toBe("day");
  });
  it("all starts at 0", () => {
    expect(rangeWindow("all", now).from).toBe(0);
  });
});

describe("bucketTimes", () => {
  it("steps by local days and hours", () => {
    const from = sec(new Date(2026, 2, 1));
    const days = bucketTimes(from, sec(new Date(2026, 2, 4)), "day", []);
    expect(days).toEqual([1, 2, 3].map((d) => sec(new Date(2026, 2, d))));
    expect(bucketTimes(from, from + 3 * 3600, "hour", [])).toHaveLength(3);
  });
  it("starts at the first cell when the range is open, and groups by month", () => {
    const first = sec(new Date(2026, 0, 15, 9));
    const months = bucketTimes(0, sec(new Date(2026, 3, 2)), "month", [{ t: first }, { t: first + 86400 * 40 }]);
    expect(months).toEqual([0, 1, 2, 3].map((m) => sec(new Date(2026, m, 1))));
  });
  it("is empty for an empty range", () => {
    expect(bucketTimes(100, 100, "day", [])).toEqual([]);
  });
});

describe("helpers", () => {
  it("niceMax rounds up to 1, 2 or 5 × 10^k", () => {
    expect(niceMax(0)).toBe(0);
    expect(niceMax(3)).toBe(5);
    expect(niceMax(12)).toBe(20);
    expect(niceMax(50)).toBe(50);
    expect(niceMax(51)).toBe(100);
    expect(niceMax(0.003)).toBe(0.005);
  });
  it("cacheRate is the cached share of the prompt", () => {
    const s = { requests: 1, input: 10, output: 999, cacheRead: 80, cacheWrite: 10, cost: 0, unpriced: 0 };
    expect(cacheRate(s)).toBeCloseTo(0.8);
    expect(cacheRate({ ...s, input: 0, cacheRead: 0, cacheWrite: 0 })).toBeNull();
  });
  it("parseDraft accepts empty fields as 0 and rejects negatives and text", () => {
    expect(parseDraft({ input: "1.5", output: "", cacheRead: " 0.1 ", cacheWrite: "0" })).toEqual({ input: 1.5, output: 0, cacheRead: 0.1, cacheWrite: 0 });
    expect(parseDraft({ input: "-1", output: "", cacheRead: "", cacheWrite: "" })).toBeNull();
    expect(parseDraft({ input: "abc", output: "", cacheRead: "", cacheWrite: "" })).toBeNull();
  });
});

describe("custom range", () => {
  const now = new Date(2026, 10, 11, 15, 30);
  it("covers whole days, the last one included, cut at now", async () => {
    const { customWindow } = await import("./UsagePage");
    const w = customWindow({ from: "2026-11-01", to: "2026-11-05" }, now)!;
    expect(w.from).toBe(sec(new Date(2026, 10, 1)));
    expect(w.to).toBe(sec(new Date(2026, 10, 6)));
    expect(w.bucket).toBe("day");
    expect(customWindow({ from: "2026-11-11", to: "2026-11-11" }, now)!.to).toBe(sec(now) + 60);
  });
  it("uses hours for up to two days and rejects bad input", async () => {
    const { customWindow } = await import("./UsagePage");
    expect(customWindow({ from: "2026-11-09", to: "2026-11-10" }, now)!.bucket).toBe("hour");
    expect(customWindow({ from: "2026-11-05", to: "2026-11-01" }, now)).toBeNull();
    expect(customWindow({ from: "", to: "2026-11-01" }, now)).toBeNull();
    expect(customWindow({ from: "2026-02-30", to: "2026-03-01" }, now)).toBeNull();
    expect(customWindow({ from: "2026-12-01", to: "2026-12-02" }, now)).toBeNull();
  });
  it("normalizes model ids like the backend", async () => {
    const { normalizeModel } = await import("./UsagePage");
    expect(normalizeModel("Anthropic/Claude-Sonnet-4-5-20250929")).toBe("claude-sonnet-4-5");
    expect(normalizeModel("claude-opus-5-5[1m]")).toBe("claude-opus-5-5");
    expect(normalizeModel("gpt-5.1-codex")).toBe("gpt-5.1-codex");
  });
});

describe("tipLeft", () => {
  it("centres the tooltip on the bar but keeps it inside the chart", async () => {
    const { tipLeft } = await import("./UsagePage");
    expect(tipLeft(300, 100, 600)).toBe(250);
    expect(tipLeft(10, 100, 600)).toBe(0);
    expect(tipLeft(590, 100, 600)).toBe(500);
    expect(tipLeft(50, 800, 600)).toBe(0);
  });
});

describe("agentsWithUsage", () => {
  it("keeps the sidebar's agents that have usage, in sidebar order", async () => {
    const { agentsWithUsage } = await import("./UsagePage");
    const cells = [{ agent: "codex" }, { agent: "claude" }, { agent: "hermes" }, { agent: "agentplus" }, { agent: "claude" }];
    expect(agentsWithUsage(cells, ["claude", "gemini", "codex"])).toEqual(["claude", "codex"]);
    expect(agentsWithUsage(cells, [])).toEqual([]);
  });
});

describe("findPrice", () => {
  const e = (model: string) => ({ model, input: 1, output: 1, cacheRead: 0, cacheWrite: 0, origin: "builtin" as const, hasDefault: false });
  it("matches like the backend: exact, then the longest entry extended by a non-version suffix", async () => {
    const { findPrice } = await import("./UsagePage");
    const list = [e("gpt-5"), e("gpt-5.1"), e("claude-opus-5"), e("gpt-4o"), e("gpt-4o-mini")];
    expect(findPrice(list, "gpt-5.1-codex")?.model).toBe("gpt-5.1");
    expect(findPrice(list, "GPT-5")?.model).toBe("gpt-5");
    expect(findPrice(list, "claude-opus-5-7")).toBeNull();
    expect(findPrice(list, "gpt-4o-mini-2024-07-18")?.model).toBe("gpt-4o-mini");
    expect(findPrice(list, "glm-5")).toBeNull();
  });
});

describe("isStale", () => {
  it("re-reads logs scanned more than 15 seconds ago", async () => {
    const { isStale } = await import("./UsagePage");
    const now = 1_800_000_000_000;
    expect(isStale(now / 1000 - 5, now)).toBe(false);
    expect(isStale(now / 1000 - 15, now)).toBe(false);
    expect(isStale(now / 1000 - 16, now)).toBe(true);
  });
});
