import { describe, expect, it } from "vitest";
import { movedAny, usableEasing } from "./dragSort";

describe("usableEasing", () => {
  const spring = "linear(0, 0.8, 1.1, 1)";
  it("keeps an easing the engine can parse", () => {
    expect(usableEasing(spring, () => true)).toBe(spring);
  });
  it("falls back when it can't (WebKit before linear()) or there is none", () => {
    expect(usableEasing(spring, () => false)).toBe("ease-out");
    expect(usableEasing("", () => true)).toBe("ease-out");
  });
});

describe("movedAny", () => {
  const [a, b, glider, other] = [{}, {}, {}, {}] as Node[];
  it("is true when one of the rows was re-inserted", () => {
    expect(movedAny([{ addedNodes: [] }, { addedNodes: [b] }], [a, b])).toBe(true);
  });
  it("ignores other changes to the list: the selection glider, new rows, removals", () => {
    expect(movedAny([{ addedNodes: [glider] }, { addedNodes: [other] }, { addedNodes: [] }], [a, b])).toBe(false);
    expect(movedAny([], [a, b])).toBe(false);
  });
});
