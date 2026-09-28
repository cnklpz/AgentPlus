// The TypeScript port against the ORIGINAL ModelTrace JavaScript core: fixtures/reference.json
// holds fixed answers and the results the reference computed for them
// (scripts/attribution-fixtures.mjs), so these tests don't compare the port with itself.
import { describe, expect, it } from "vitest";
import raw from "./unified_bank.json?raw";
import reference from "./fixtures/reference.json";
import { parseBank } from "./bank";
import { AttributionError, analyzeOutputs, candidateStanding, minimumNumbers, parseNumbers } from "./core";

const bank = parseBank(raw);
type RefResult = { model: string; display_name: string; probability: number; profile_similarity: number; score: number; family: string; family_name: string; conditional_probability: number };
type RefCase = { name: string; outputs: { text: string; expected_count: number }[]; result: { error: string } | {
  results: RefResult[]; used_outputs: number; prediction: string; probability: number;
  diagnostics: { index: number; parsed_numbers: number; minimum_numbers: number; accepted: boolean }[];
  calibration: { queries: string; beta: number };
  family_probabilities: { family: string; display_name: string; probability: number }[];
} };
const cases = reference.cases as RefCase[];
const outputsOf = (c: RefCase) => c.outputs.map((o) => ({ text: o.text, expectedCount: o.expected_count }));
const scored = cases.filter((c): c is RefCase & { result: Exclude<RefCase["result"], { error: string }> } => !("error" in c.result));

describe("reference fixture", () => {
  it("comes from the pinned commit and covers the calibration keys", () => {
    expect(reference.source.commit).toBe("df3a0f9d3e054c0dc02d6d586686db8daf8fa7c8");
    expect(new Set(scored.map((c) => c.result.calibration.queries))).toEqual(new Set(["1", "2", "3"]));
  });
});

describe("parseNumbers", () => {
  it.each(reference.parse.map((p) => [p.text, p.numbers] as const))("parses %j like the reference", (text, numbers) => {
    expect(parseNumbers(text)).toEqual(numbers);
  });
});

describe("minimumNumbers", () => {
  it("is max(80, ceil(0.55 × expected)), 80 without a count", () => {
    expect([0, 100, 145, 146, 292, 300, 332].map(minimumNumbers)).toEqual([80, 80, 80, 81, 161, 165, 183]);
  });
});

describe("analyzeOutputs", () => {
  it.each(scored.map((c) => [c.name, c] as const))("matches the reference: %s", (_name, c) => {
    const want = c.result;
    const got = analyzeOutputs(outputsOf(c), bank);
    expect(got.usedOutputs).toBe(want.used_outputs);
    expect(got.calibration).toEqual({ queries: want.calibration.queries, beta: want.calibration.beta });
    expect(got.diagnostics).toEqual(want.diagnostics.map((d) => ({ index: d.index, parsedNumbers: d.parsed_numbers, minimumNumbers: d.minimum_numbers, accepted: d.accepted })));
    // Same ranking, and the same numbers (the arithmetic is done in the same order).
    expect(got.results.map((r) => r.model)).toEqual(want.results.map((r) => r.model));
    got.results.forEach((r, i) => {
      const w = want.results[i];
      expect(r.displayName).toBe(w.display_name);
      expect(r.family).toBe(w.family);
      expect(r.familyName).toBe(w.family_name);
      expect(r.probability).toBeCloseTo(w.probability, 14);
      expect(r.score).toBeCloseTo(w.score, 12);
      expect(r.profileSimilarity).toBeCloseTo(w.profile_similarity, 12);
      expect(r.conditionalProbability).toBeCloseTo(w.conditional_probability, 12);
    });
    expect(got.results[0].model).toBe(want.prediction);
    expect(got.families.map((f) => f.family)).toEqual(want.family_probabilities.map((f) => f.family));
    got.families.forEach((f, i) => expect(f.probability).toBeCloseTo(want.family_probabilities[i].probability, 14));
  });

  it("uses the temperature for the number of valid answers (capped at three)", () => {
    const byName = (n: string) => scored.find((c) => c.name === n)!;
    const beta = (n: string) => analyzeOutputs(outputsOf(byName(n)), bank).calibration;
    expect(beta("one answer")).toEqual({ queries: "1", beta: bank.calibration["1"].beta });
    expect(beta("two answers")).toEqual({ queries: "2", beta: bank.calibration["2"].beta });
    // Three requested, one too short: scored as two answers.
    expect(beta("one of three too short")).toEqual({ queries: "2", beta: bank.calibration["2"].beta });
    expect(beta("three answers, one candidate")).toEqual({ queries: "3", beta: bank.calibration["3"].beta });
    expect(beta("four answers use the three-query calibration")).toEqual({ queries: "3", beta: bank.calibration["3"].beta });
  });

  it("averages per-answer scores instead of scoring the concatenated text", () => {
    const c = scored.find((x) => x.name === "answers from different candidates")!;
    const apart = analyzeOutputs(outputsOf(c), bank);
    const joined = analyzeOutputs([{ text: c.outputs.map((o) => o.text).join(", "), expectedCount: 900 }], bank);
    expect(joined.usedOutputs).toBe(1);
    expect(joined.results[0].probability).not.toBeCloseTo(apart.results[0].probability, 6);
  });

  it("sums a family's global probabilities, and conditional shares stay within the family", () => {
    const c = scored.find((x) => x.name === "answers from different candidates")!;
    const a = analyzeOutputs(outputsOf(c), bank);
    for (const f of a.families) {
      const members = a.results.filter((r) => r.family === f.family);
      expect(f.probability).toBeCloseTo(members.reduce((s, r) => s + r.probability, 0), 14);
      expect(members.reduce((s, r) => s + r.conditionalProbability, 0)).toBeCloseTo(1, 12);
    }
    expect(a.results.reduce((s, r) => s + r.probability, 0)).toBeCloseTo(1, 12);
  });

  it("fails without a valid answer, and never makes up a score", () => {
    const none = cases.find((c) => c.name === "nothing usable")!;
    expect("error" in none.result).toBe(true);
    expect(() => analyzeOutputs(outputsOf(none), bank)).toThrow(AttributionError);
    expect(() => analyzeOutputs([], bank)).toThrowError(expect.objectContaining({ code: "noValidOutputs" }));
  });
});

describe("candidateStanding", () => {
  it("reports the tested model's own probability when it isn't the best match", () => {
    const c = scored.find((x) => x.name === "answers from different candidates")!;
    const a = analyzeOutputs(outputsOf(c), bank);
    const want = c.result.results.findIndex((r) => r.model === "gpt-5.4");
    expect(want).toBeGreaterThan(0);
    const s = candidateStanding(a, "gpt-5.4")!;
    expect(s.rank).toBe(want + 1);
    expect(s.result.model).toBe("gpt-5.4");
    expect(s.result.probability).toBeCloseTo(c.result.results[want].probability, 14);
    expect(s.result.probability).toBeLessThan(a.results[0].probability);
    expect(candidateStanding(a, "no-such-model")).toBeNull();
  });
});
