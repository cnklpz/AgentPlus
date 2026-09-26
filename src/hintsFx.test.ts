import { describe, expect, it } from "vitest";
import { levelStyle, rgba, shardPolys, stagger } from "./hintsFx";

describe("stagger", () => {
  it("steps evenly while the text is short", () => {
    expect(stagger(4, false, 10, 300)).toEqual([0, 10, 20, 30]);
  });

  it("squeezes long text into the spread", () => {
    const d = stagger(101, false, 10, 300);
    expect(d[0]).toBe(0);
    expect(d[100]).toBe(300);
    expect(d.every((x, i) => i === 0 || x >= d[i - 1])).toBe(true);
  });

  it("runs from the end when reversed", () => {
    expect(stagger(3, true, 10, 300)).toEqual([20, 10, 0]);
  });

  it("handles no characters and a single one", () => {
    expect(stagger(0)).toEqual([]);
    expect(stagger(1)).toEqual([0]);
  });
});

describe("rgba", () => {
  it("reads computed colours", () => {
    expect(rgba("rgb(10, 20, 30)")).toEqual([10, 20, 30, 1]);
    expect(rgba("rgba(10, 20, 30, 0.5)")).toEqual([10, 20, 30, 0.5]);
    expect(rgba("rgb(10 20 30 / 40%)")).toEqual([10, 20, 30, 0.4]);
  });

  it("gives up on anything else", () => {
    expect(rgba("color(srgb 1 0 0)")).toBeNull();
  });
});

describe("levelStyle", () => {
  it("starts invisible and ends close to the colour, sharp", () => {
    expect(levelStyle(0, [0, 0, 0, 1])).toContain("color: rgba(0, 0, 0, 0.000)");
    expect(levelStyle(0, [0, 0, 0, 1])).toContain("rgba(0, 0, 0, 0.000);");
    expect(levelStyle(9, [0, 0, 0, 1])).toContain("color: rgba(0, 0, 0, 0.875)");
    expect(levelStyle(9, [0, 0, 0, 1])).toContain("0 0 0.6px");
  });

  it("scales with the colour's own alpha", () => {
    expect(levelStyle(9, [0, 0, 0, 0.5])).toContain("color: rgba(0, 0, 0, 0.437)");
  });
});

describe("shardPolys", () => {
  const half = () => 0.5;
  const area = (poly: [number, number][]) =>
    Math.abs(poly.reduce((a, [x, y], i) => { const [u, v] = poly[(i + 1) % poly.length]; return a + x * v - u * y; }, 0)) / 2;

  it("cuts as many pieces as asked", () => {
    expect(shardPolys(10, 20, 1, half)).toHaveLength(1);
    expect(shardPolys(10, 20, 2, half)).toHaveLength(2);
    expect(shardPolys(10, 20, 3, half)).toHaveLength(3);
  });

  it("covers the whole glyph box, without overlap", () => {
    for (const n of [1, 2, 3]) {
      const total = shardPolys(10, 20, n, Math.random).reduce((a, p) => a + area(p), 0);
      expect(total).toBeCloseTo(200, 6);
    }
  });
});
