import { describe, expect, it } from "vitest";
import { dropIndex, mergeOrder, moveItem, sortByOrder } from "./order";

const ids = (xs: { id: string }[]) => xs.map((x) => x.id);
const items = (...xs: string[]) => xs.map((id) => ({ id }));

describe("sortByOrder", () => {
  it("keeps the default order when nothing was dragged", () => {
    expect(ids(sortByOrder(items("a", "b", "c"), []))).toEqual(["a", "b", "c"]);
  });
  it("follows the saved order", () => {
    expect(ids(sortByOrder(items("a", "b", "c"), ["c", "a", "b"]))).toEqual(["c", "a", "b"]);
  });
  it("puts ids the order doesn't know after the known ones, in their own order", () => {
    expect(ids(sortByOrder(items("a", "new1", "b", "new2", "c"), ["c", "b", "a"]))).toEqual(["c", "b", "a", "new1", "new2"]);
  });
  it("ignores saved ids that aren't listed", () => {
    expect(ids(sortByOrder(items("a", "b"), ["gone", "b", "a"]))).toEqual(["b", "a"]);
  });
});

describe("mergeOrder", () => {
  it("remembers hidden ids after the visible ones", () => {
    expect(mergeOrder(["b", "a"], ["a", "hidden", "b"])).toEqual(["b", "a", "hidden"]);
  });
  it("doesn't repeat ids", () => {
    expect(mergeOrder(["a", "b"], ["b", "a"])).toEqual(["a", "b"]);
  });
});

describe("moveItem", () => {
  it("moves down and up", () => {
    expect(moveItem(["a", "b", "c", "d"], 0, 2)).toEqual(["b", "c", "a", "d"]);
    expect(moveItem(["a", "b", "c", "d"], 3, 1)).toEqual(["a", "d", "b", "c"]);
  });
  it("leaves the list as it was for the same slot, and doesn't mutate it", () => {
    const list = ["a", "b", "c"];
    expect(moveItem(list, 1, 1)).toEqual(list);
    moveItem(list, 0, 2);
    expect(list).toEqual(["a", "b", "c"]);
  });
});

describe("dropIndex", () => {
  // Four 50px rows 6px apart: midpoints 25, 81, 137, 193.
  const mids = [25, 81, 137, 193];
  it("stays put until the centre passes a neighbour's midpoint", () => {
    expect(dropIndex(mids, 1, 81)).toBe(1);
    expect(dropIndex(mids, 1, 136)).toBe(1);
    expect(dropIndex(mids, 1, 26)).toBe(1);
  });
  it("takes the slot of the farthest neighbour passed", () => {
    expect(dropIndex(mids, 1, 138)).toBe(2);
    expect(dropIndex(mids, 1, 400)).toBe(3);
    expect(dropIndex(mids, 2, 24)).toBe(0);
    expect(dropIndex(mids, 3, 80)).toBe(1);
  });
  it("handles the ends", () => {
    expect(dropIndex(mids, 0, -100)).toBe(0);
    expect(dropIndex(mids, 3, 999)).toBe(3);
  });
});
