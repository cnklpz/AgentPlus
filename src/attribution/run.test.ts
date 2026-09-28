import { describe, expect, it, vi } from "vitest";
import raw from "./unified_bank.json?raw";
import reference from "./fixtures/reference.json";
import type { AttributionLive, AttributionSample } from "../api";
import { parseBank } from "./bank";
import type { Challenge } from "./challenge";
import { CancelToken, type RunReport, runAttribution } from "./run";

const bank = parseBank(raw);
const caseNamed = (name: string) => reference.cases.find((c) => c.name === name)!;
const challengesOf = (name: string): Challenge[] =>
  caseNamed(name).outputs.map((o, i) => ({ id: `c${i}`, expectedCount: o.expected_count, prompt: `prompt ${i} ${o.expected_count}` }));
const ok = (text: string, requests = 1): AttributionSample => ({ ok: true, text, kind: null, error: null, status: 200, requests, ms: 1000, usage: null });
const fail = (kind: string, error: string, requests = 1, status: number | null = null): AttributionSample =>
  ({ ok: false, text: null, kind, error, status, requests, ms: 10, usage: null });
/** A sampler answering the prompts in order with `replies`. */
const replying = (replies: AttributionSample[]) => {
  const fn = vi.fn(async (c: Challenge) => replies[Number(c.id.slice(1))]);
  return fn;
};
type RefResult = { results: { model: string; probability: number }[]; used_outputs: number; calibration: { queries: string } };

describe("runAttribution", () => {
  it("scores three independent answers like the reference", async () => {
    const c = caseNamed("three answers, one candidate");
    const sample = replying(c.outputs.map((o, i) => ok(o.text, i + 1)));
    const updates: RunReport[] = [];
    const r = await runAttribution({ runs: 3, bank, sample, token: new CancelToken(), challenges: challengesOf(c.name), onUpdate: (u) => updates.push(u) });
    expect(sample.mock.calls.map(([x]) => x.prompt)).toEqual(["prompt 0 300", "prompt 1 317", "prompt 2 329"]);
    expect([r.requested, r.valid, r.requests, r.requestsExact, r.cancelled]).toEqual([3, 3, 6, true, false]);
    expect(r.rows.map((x) => x.status)).toEqual(["valid", "valid", "valid"]);
    const want = c.result as RefResult;
    expect(r.analysis!.calibration.queries).toBe("3");
    expect(r.analysis!.results.map((x) => x.model)).toEqual(want.results.map((x) => x.model));
    r.analysis!.results.forEach((x, i) => expect(x.probability).toBeCloseTo(want.results[i].probability, 14));
    // Progress: each sample running, then done.
    expect(updates.map((u) => u.rows.map((x) => x.status[0]).join(""))).toEqual(["rpp", "vpp", "vrp", "vvp", "vvr", "vvv"]);
  });

  it("scores the valid answers of a partial run with their own calibration", async () => {
    const c = caseNamed("one of three too short");
    const texts = c.outputs.map((o) => o.text);
    // First valid, second too short, third valid: the reference's own case.
    const r = await runAttribution({ runs: 3, bank, sample: replying(texts.map((t) => ok(t))), token: new CancelToken(), challenges: challengesOf(c.name) });
    expect(r.rows.map((x) => [x.status, x.parsedNumbers, x.minimumNumbers])).toEqual([["valid", 310, 171], ["tooShort", 120, 176], ["valid", 301, 166]]);
    expect(r.rows[1].kind).toBe("tooShort");
    const want = c.result as RefResult;
    expect(r.valid).toBe(want.used_outputs);
    expect(r.analysis!.calibration.queries).toBe("2");
    r.analysis!.results.forEach((x, i) => expect(x.probability).toBeCloseTo(want.results[i].probability, 14));
  });

  it("keeps failures with their reason and counts every request, without topping up", async () => {
    const c = caseNamed("two answers");
    const sample = vi.fn()
      .mockResolvedValueOnce(fail("http", "Server error (HTTP 500)", 3, 500))
      .mockResolvedValueOnce(ok(c.outputs[1].text));
    const challenges = [...challengesOf(c.name)];
    const r = await runAttribution({ runs: 2, bank, sample, token: new CancelToken(), challenges });
    expect(sample).toHaveBeenCalledTimes(2);
    expect(r.rows[0]).toMatchObject({ status: "failed", kind: "http", error: "Server error (HTTP 500)", httpStatus: 500, requests: 3 });
    expect([r.requested, r.valid, r.requests]).toEqual([2, 1, 4]);
    expect(r.analysis!.calibration.queries).toBe("1");
    expect(r.analysis!.usedOutputs).toBe(1);
  });

  it("makes no score without a valid answer", async () => {
    const sample = vi.fn()
      .mockResolvedValueOnce(fail("truncated", "cut off"))
      .mockResolvedValueOnce(ok("I can't do that."))
      .mockRejectedValueOnce(new Error("IPC broke"));
    const r = await runAttribution({ runs: 3, bank, sample, token: new CancelToken(), errorText: (e) => `failed: ${(e as Error).message}` });
    expect(r.analysis).toBeNull();
    expect(r.valid).toBe(0);
    expect(r.rows.map((x) => [x.status, x.kind])).toEqual([["failed", "truncated"], ["tooShort", "tooShort"], ["failed", "error"]]);
    expect(r.rows[2].error).toBe("failed: IPC broke");
    expect(r.rows[2].requests).toBe(0);
  });

  it("sends a fresh challenge per sample", async () => {
    const sample = vi.fn(async () => fail("empty", "no text"));
    await runAttribution({ runs: 3, bank, sample, token: new CancelToken() });
    const sent = sample.mock.calls.map((args) => (args as unknown as [Challenge])[0]);
    expect(new Set(sent.map((c) => c.prompt)).size).toBe(3);
    expect(new Set(sent.map((c) => c.expectedCount)).size).toBe(3);
  });

  it("stops at cancel: the answer on its way and later samples are dropped", async () => {
    const c = caseNamed("three answers, one candidate");
    let release!: (s: AttributionSample) => void;
    const sample = vi.fn()
      .mockResolvedValueOnce(ok(c.outputs[0].text))
      .mockReturnValueOnce(new Promise<AttributionSample>((r) => { release = r; }))
      .mockResolvedValueOnce(ok(c.outputs[2].text));
    const token = new CancelToken();
    const updates: RunReport[] = [];
    const run = runAttribution({ runs: 3, bank, sample, token, challenges: challengesOf(c.name), onUpdate: (u) => updates.push(u) });
    await vi.waitFor(() => expect(sample).toHaveBeenCalledTimes(2));
    const before = updates.length;
    token.cancel();
    const r = await run;
    release(ok(c.outputs[1].text));
    await Promise.resolve();
    expect(sample).toHaveBeenCalledTimes(2);
    expect(updates.length).toBe(before);
    expect(r.cancelled).toBe(true);
    expect(r.rows.map((x) => x.status)).toEqual(["valid", "discarded", "skipped"]);
    // The dropped sample did go out: at least one request, so the total is a lower bound.
    expect([r.requests, r.requestsExact]).toEqual([2, false]);
    // What did come back is still scored, as one answer.
    expect(r.analysis!.calibration.queries).toBe("1");
  });

  it("passes each answer on as it streams, only while its sample is on its way", async () => {
    const c = caseNamed("two answers");
    const events: [number, AttributionLive][] = [];
    const lives: ((e: AttributionLive) => void)[] = [];
    const token = new CancelToken();
    let second!: (s: AttributionSample) => void;
    const run = runAttribution({
      runs: 2, bank, token, challenges: challengesOf(c.name), onLive: (i, e) => events.push([i, e]),
      sample: (x, live) => {
        lives.push(live);
        live({ type: "request", n: 1 });
        live({ type: "text", text: x.id });
        return x.id === "c0" ? Promise.resolve(ok(c.outputs[0].text)) : new Promise((r) => { second = r; });
      },
    });
    await vi.waitFor(() => expect(lives).toHaveLength(2));
    // The first sample is done: whatever it still reports is dropped.
    lives[0]({ type: "text", text: "late" });
    lives[1]({ type: "reasoning", text: "hm" });
    token.cancel();
    lives[1]({ type: "text", text: "after cancel" });
    await run;
    second(ok(c.outputs[1].text));
    expect(events).toEqual([
      [1, { type: "request", n: 1 }], [1, { type: "text", text: "c0" }],
      [2, { type: "request", n: 1 }], [2, { type: "text", text: "c1" }], [2, { type: "reasoning", text: "hm" }],
    ]);
  });

  it("keeps an old run's late answers away from the next one", async () => {
    const c = caseNamed("one answer");
    let late!: (s: AttributionSample) => void;
    // How the dialog uses it: the current run's token gates what is shown.
    let current: CancelToken | null = null;
    const shown: string[] = [];
    const start = (tag: string, sample: RunOptionsSample) => {
      const token = new CancelToken();
      current?.cancel();
      current = token;
      return runAttribution({ runs: 1, bank, sample, token, challenges: challengesOf(c.name), onUpdate: () => shown.push(tag) })
        .then((r) => { if (current === token) shown.push(`${tag}:done:${r.valid}`); return r; });
    };
    const a = start("a", () => new Promise((r) => { late = r; }));
    const b = start("b", async () => ok(c.outputs[0].text));
    await b;
    late(ok(c.outputs[0].text));
    const ra = await a;
    expect(ra.cancelled).toBe(true);
    expect(shown).toEqual(["a", "b", "b", "b:done:1"]);
  });
});

type RunOptionsSample = (c: Challenge) => Promise<AttributionSample>;
