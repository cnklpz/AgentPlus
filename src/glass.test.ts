import { describe, expect, it } from "vitest";
import { easeFilter, glassMap } from "./glass";

describe("glassMap", () => {
  const [w, h] = [200, 50];
  const { data, scale } = glassMap(w, h, 10);
  /** The backdrop offset (px) a pixel reaches for, as feDisplacementMap reads it. */
  const at = (x: number, y: number) => {
    const i = (y * w + x) * 4;
    return { dx: scale * (data[i] / 255 - 0.5), dy: scale * (data[i + 1] / 255 - 0.5), a: data[i + 3] };
  };

  it("is opaque and fills every pixel", () => {
    expect(data.length).toBe(w * h * 4);
    for (let i = 3; i < data.length; i += 4) expect(data[i]).toBe(255);
    expect(scale).toBeGreaterThan(0);
  });

  it("leaves the middle alone", () => {
    const { dx, dy } = at(w / 2, h / 2);
    expect(Math.abs(dx)).toBeLessThan(0.2);
    expect(Math.abs(dy)).toBeLessThan(0.2);
  });

  it("bends the rim inward", () => {
    expect(at(w / 2, 1).dy).toBeGreaterThan(2); // top edge reaches down
    expect(at(w / 2, h - 2).dy).toBeLessThan(-2); // bottom edge reaches up
    expect(at(2, h / 2).dx).toBeGreaterThan(2); // left end reaches right
    expect(at(w - 3, h / 2).dx).toBeLessThan(-2); // right end reaches left
  });

  it("magnifies a little: away from the rim it reaches toward the centre", () => {
    expect(at(w / 2 - 60, h / 2).dx).toBeGreaterThan(3);
    expect(at(w / 2 + 60, h / 2).dx).toBeLessThan(-3);
    expect(Math.abs(at(w / 2 - 60, h / 2).dy)).toBeLessThan(0.5);
  });

  it("bends hardest at the edge, easing off across the bezel", () => {
    const edge = at(w / 2, 0).dy;
    const mid = at(w / 2, 8).dy;
    const past = at(w / 2, 17).dy;
    expect(edge).toBeGreaterThan(mid);
    expect(mid).toBeGreaterThan(past);
    // Past the bezel only the gentle magnification is left.
    expect(past).toBeLessThan(2);
  });

  it("reaches no further than the bezel plus the magnification", () => {
    for (let y = 0; y < h; y++) {
      for (let x = 0; x < w; x++) {
        const { dx, dy } = at(x, y);
        expect(Math.hypot(dx, dy)).toBeLessThanOrEqual(16 + 0.07 * w / 2 + 0.5);
      }
    }
  });

  it("leaves the clipped corners alone", () => {
    const { dx, dy } = at(0, 0);
    expect(Math.abs(dx)).toBeLessThan(0.2);
    expect(Math.abs(dy)).toBeLessThan(0.2);
  });
});

describe("easeFilter", () => {
  const list = 'url("#ap-glass") blur(0.25px) contrast(1.05) brightness(1.01) saturate(1.4)';
  it("keeps the filter as given at full strength", () => {
    expect(easeFilter(list, 1)).toBe('url("#ap-glass") blur(0.250px) contrast(1.050) brightness(1.010) saturate(1.400)');
  });
  it("eases every function toward doing nothing, keeping the url", () => {
    expect(easeFilter(list, 0.5)).toBe('url("#ap-glass") blur(0.125px) contrast(1.025) brightness(1.005) saturate(1.200)');
    expect(easeFilter(list, 0)).toBe('url("#ap-glass") blur(0.000px) contrast(1.000) brightness(1.000) saturate(1.000)');
  });
  it("handles the frosted fallback without a url", () => {
    expect(easeFilter("blur(3px) saturate(1.6)", 0.5)).toBe("blur(1.500px) saturate(1.300)");
  });
});
