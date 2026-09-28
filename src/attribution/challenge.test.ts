import { afterEach, describe, expect, it, vi } from "vitest";
import reference from "./fixtures/reference.json";
import { ACTIONS, ENDINGS, FIXED_CLOSING, FIXED_RULES, LENGTH_MAX, LENGTH_MIN, OPENINGS, SEPARATORS, generateChallenges } from "./challenge";

/** The seeded stand-in for `crypto` the fixture generator gave the reference code. */
function seeded(seed: number) {
  let a = seed >>> 0;
  const next = () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  let n = 0;
  return {
    getRandomValues: (buffer: Uint32Array) => { buffer[0] = Math.floor(next() * 0x100000000); return buffer; },
    randomUUID: () => `uuid-${seed}-${n++}`,
  };
}

afterEach(() => { vi.unstubAllGlobals(); });

describe("generateChallenges", () => {
  it.each(reference.challenges.map((c) => [c.count, c] as const))("builds the reference's %i challenge(s) from the same random draws", (_n, c) => {
    vi.stubGlobal("crypto", seeded(c.seed));
    const got = generateChallenges(c.count);
    expect(got).toEqual(c.challenges.map((x) => ({ id: x.id, expectedCount: x.expected_count, prompt: x.prompt })));
  });

  it.each([1, 2, 3])("gives %i fresh challenges with distinct lengths in range", (count) => {
    const got = generateChallenges(count);
    expect(got).toHaveLength(count);
    expect(new Set(got.map((c) => c.expectedCount)).size).toBe(count);
    expect(new Set(got.map((c) => c.id)).size).toBe(count);
    for (const c of got) {
      expect(c.expectedCount).toBeGreaterThanOrEqual(LENGTH_MIN);
      expect(c.expectedCount).toBeLessThanOrEqual(LENGTH_MAX);
      // Built only from the reference's pieces, in its order.
      const opening = OPENINGS.find((o) => c.prompt.startsWith(`${o}。`))!;
      expect(opening).toBeDefined();
      const rest = c.prompt.slice(opening.length + 1);
      const action = ACTIONS.find((a) => rest.startsWith(`${a} ${c.expectedCount} 个 1 到 355（含端点）的整数。${FIXED_RULES}`))!;
      expect(action).toBeDefined();
      const tail = rest.slice(`${action} ${c.expectedCount} 个 1 到 355（含端点）的整数。${FIXED_RULES}`.length);
      expect(ENDINGS.some((e) => SEPARATORS.some((s) => tail === `${e}${s}${FIXED_CLOSING}`))).toBe(true);
    }
  });

  it("can use every length once", () => {
    const n = LENGTH_MAX - LENGTH_MIN + 1;
    const lengths = generateChallenges(n).map((c) => c.expectedCount).sort((a, b) => a - b);
    expect(lengths).toEqual(Array.from({ length: n }, (_, i) => LENGTH_MIN + i));
  });
});
