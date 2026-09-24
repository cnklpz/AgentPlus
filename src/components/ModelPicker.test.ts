import { describe, expect, it } from "vitest";
import { growPool } from "./ModelPicker";

describe("growPool", () => {
  it("appends only new ids, once each, keeping the order", () => {
    expect(growPool(["a", "b"], ["b", "c", "c", "d"])).toEqual(["a", "b", "c", "d"]);
  });
  it("returns the same list when nothing is new (no re-render)", () => {
    const pool = ["a", "b"];
    expect(growPool(pool, ["b", "a"])).toBe(pool);
  });
  it("starting from empty dedupes (a reset to another template's models)", () => {
    expect(growPool([], ["x", "y", "x"])).toEqual(["x", "y"]);
  });
});
