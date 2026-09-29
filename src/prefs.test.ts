import { describe, expect, it } from "vitest";
import { normalizePrefs } from "./prefs";

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
