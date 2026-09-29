import { describe, expect, it } from "vitest";
import type { SkillRoot, SkillsOverview } from "../api";
import { groups, seenBy } from "./SkillsPage";

const root = (kind: SkillRoot["kind"]): SkillRoot => ({
  path: `/${kind}`, kind, exists: true, owner: kind === "shared" ? null : "kimi",
  readers: kind === "off" ? [] : ["kimi"],
  skills: [{ id: "pdf", name: "pdf", description: "PDF", dir: `/${kind}/pdf`, files: 1, bytes: 10, sig: "same", problem: null }],
});
const overview = (roots: SkillRoot[], disabled: string[] = []): SkillsOverview => ({
  roots,
  agents: [{ agent: "kimi", supported: true, roots: roots.filter((r) => r.kind !== "off").map((r) => r.path), disabled, switchable: disabled.length > 0 }],
});
const copy = (r: SkillRoot) => ({ root: r, s: r.skills[0] });

describe("skill availability", () => {
  it("keeps the shared fallback active when the own copy is parked", () => {
    const shared = root("shared"), parked = root("off");
    const o = overview([shared, parked]);
    expect(groups(o)[0].agents).toEqual([{ agent: "kimi", off: false }]);
    expect(seenBy(o, "kimi", copy(shared))).toBe("active");
    expect(seenBy(o, "kimi", copy(parked))).toBe("disabled");
  });

  it("shows a parked-only skill as disabled", () => {
    const parked = root("off");
    const o = overview([parked]);
    expect(groups(o)[0].agents).toEqual([{ agent: "kimi", off: true }]);
    expect(seenBy(o, "kimi", copy(parked))).toBe("disabled");
  });

  it("respects native disabled switches and root precedence", () => {
    const own = root("own"), shared = root("shared");
    const on = overview([own, shared]);
    expect(seenBy(on, "kimi", copy(own))).toBe("active");
    expect(seenBy(on, "kimi", copy(shared))).toBe("shadowed");
    const off = overview([own, shared], ["pdf"]);
    expect(groups(off)[0].agents).toEqual([{ agent: "kimi", off: true }]);
    expect(seenBy(off, "kimi", copy(own))).toBe("disabled");
    expect(seenBy(off, "kimi", copy(shared))).toBe("shadowed");
  });
});
