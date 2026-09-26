import { describe, expect, it, vi } from "vitest";

// motion.ts installs nothing on import, but reads matchMedia at module level.
vi.stubGlobal("window", { matchMedia: () => ({ matches: false }) });
const { scraps } = await import("./motion");

/** Shoelace area of a polygon. */
const area = (pts: [number, number][]) => Math.abs(pts.reduce((s, [x, y], i) => {
  const [nx, ny] = pts[(i + 1) % pts.length];
  return s + x * ny - nx * y;
}, 0)) / 2;

describe("scraps", () => {
  it("tears a piece into scraps that fit back together exactly", () => {
    for (const [w, h] of [[640, 62], [300, 34], [720, 96], [90, 20]]) {
      const bits = scraps(w, h);
      const total = bits.reduce((s, b) => s + area(b.pts), 0);
      expect(total).toBeCloseTo(w * h, 3);
      for (const b of bits) {
        for (const [x, y] of b.pts) {
          expect(x).toBeGreaterThanOrEqual(0);
          expect(x).toBeLessThanOrEqual(w);
          expect(y).toBeGreaterThanOrEqual(0);
          expect(y).toBeLessThanOrEqual(h);
        }
      }
    }
  });

  it("makes more scraps from bigger pieces, within bounds", () => {
    expect(scraps(90, 20).length).toBe(2);
    expect(scraps(640, 62).length).toBe(12);
    expect(scraps(2000, 400).length).toBe(24);
  });

  it("keeps to the limit it is given, still covering the whole piece", () => {
    for (const max of [2, 3, 5, 8]) {
      const bits = scraps(640, 96, max);
      expect(bits.length).toBeLessThanOrEqual(max);
      expect(bits.reduce((s, b) => s + area(b.pts), 0)).toBeCloseTo(640 * 96, 3);
    }
  });
});
