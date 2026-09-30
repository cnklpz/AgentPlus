import { describe, expect, it, vi } from "vitest";
import { api } from "./api";
import { normalizePrefs, syncTrayTheme } from "./prefs";

vi.mock("./api", () => ({ api: { setTrayDark: vi.fn() } }));

describe("syncTrayTheme", () => {
  it("tells the backend once per change and again after a failed call", async () => {
    const set = vi.mocked(api.setTrayDark);
    set.mockResolvedValue(undefined);
    syncTrayTheme(true);
    syncTrayTheme(true);
    expect(set.mock.calls).toEqual([[true]]);
    syncTrayTheme(false);
    expect(set.mock.calls).toEqual([[true], [false]]);

    set.mockRejectedValueOnce(new Error("store locked"));
    syncTrayTheme(true);
    // Past the failed call's handlers, however many steps they take.
    await new Promise((r) => setTimeout(r, 0));
    syncTrayTheme(true);
    expect(set.mock.calls).toEqual([[true], [false], [true], [true]]);
    syncTrayTheme(true);
    expect(set).toHaveBeenCalledTimes(4);
  });
});

describe("normalizePrefs", () => {
  it("fills in defaults for nothing stored", () => {
    for (const v of [null, undefined, 1, "x", []]) {
      const p = normalizePrefs(v);
      expect(p.hiddenAgents).toEqual([]);
      expect(p.lang).toBe("auto");
      expect(p.closeAction).toBe("ask");
    }
  });

  it("keeps valid values", () => {
    const p = normalizePrefs({ motion: "off", hiddenAgents: ["codex"], agentOrder: ["a", "b"], lang: "zh", theme: "dark", privacy: true, hints: "brief" });
    expect(p).toMatchObject({ motion: "off", hiddenAgents: ["codex"], agentOrder: ["a", "b"], lang: "zh", theme: "dark", privacy: true, hints: "brief" });
  });

  it("drops values of the wrong type instead of breaking the first render", () => {
    const p = normalizePrefs({ hiddenAgents: "codex", agentOrder: [1, "a", null], lang: "fr", theme: 3, privacy: "yes", closeAction: "minimize" });
    expect(p.hiddenAgents).toEqual([]);
    expect(p.agentOrder).toEqual(["a"]);
    expect(p.lang).toBe("auto");
    expect(p.theme).toBe("auto");
    expect(p.privacy).toBe(false);
    expect(p.closeAction).toBe("ask");
  });

  it("shows MCP, skills and plugins unless told not to", () => {
    expect(normalizePrefs(null)).toMatchObject({ showMcp: true, showSkills: true, showPlugins: true });
    expect(normalizePrefs({ showMcp: false, showSkills: false, showPlugins: false })).toMatchObject({ showMcp: false, showSkills: false, showPlugins: false });
    expect(normalizePrefs({ showMcp: "no", showSkills: 0, showPlugins: null })).toMatchObject({ showMcp: true, showSkills: true, showPlugins: true });
  });

  it("forgets keys it doesn't know", () => {
    expect(Object.keys(normalizePrefs({ stale: 1 }))).not.toContain("stale");
  });
});
