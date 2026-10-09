import { describe, expect, it } from "vitest";
import raw from "./unified_bank.json?raw";
import reference from "./fixtures/reference.json";
import type { AttributionSample, AttributionTarget } from "../api";
import { BANK_SOURCE, parseBank } from "./bank";
import type { Challenge } from "./challenge";
import { type HistoryEntry, historyEntry, outcome, parseHistory, summarize } from "./history";
import { CancelToken, runAttribution } from "./run";

const bank = parseBank(raw);
const c = reference.cases.find((x) => x.name === "answers from different candidates")!;
const challenges: Challenge[] = c.outputs.map((o, i) => ({ id: `c${i}`, expectedCount: o.expected_count, prompt: `p${i}` }));
const ok = (text: string): AttributionSample => ({ ok: true, text, kind: null, error: null, status: 200, requests: 1, ms: 5, usage: null });
const target: AttributionTarget = {
  agent: "codex", provider: "relay", entry: "agentplus", model: "openai/gpt-5.4", api: "responses", url: "https://relay.example.com/v1/responses",
  hasKey: true, gateway: null, blocked: null, fp: "f",
};

async function report(replies: AttributionSample[]) {
  return runAttribution({ runs: 3, bank, sample: async (x) => replies[Number(x.id.slice(1))], token: new CancelToken(), challenges });
}

/** As the backend returns it: stamped, and through JSON. */
const stored = (e: object, at = 1_700_000_000_000): unknown => JSON.parse(JSON.stringify({ ...e, id: String(at), at }));

describe("history entries", () => {
  it("keep the tested model's own probability, not the winner's", async () => {
    const r = await report(c.outputs.map((o) => ok(o.text)));
    const e = historyEntry(target, "Relay", "gpt-5.4", r, BANK_SOURCE.commit);
    const want = (c.result as { results: { model: string; probability: number }[] }).results;
    const rank = want.findIndex((x) => x.model === "gpt-5.4") + 1;
    expect(rank).toBeGreaterThan(1);
    expect(e.scores!.own).toEqual({ probability: want[rank - 1].probability, rank });
    expect(e.scores!.top.map((x) => x.model)).toEqual(want.slice(0, 3).map((x) => x.model));
    expect(e.scores!.candidates).toBe(18);
    expect(e.scores!.calibration.queries).toBe("3");
    expect([e.model, e.candidate, e.provider, e.forward, e.bank]).toEqual(["openai/gpt-5.4", "gpt-5.4", "relay", null, BANK_SOURCE.commit]);
    expect(outcome(e)).toBe("complete");
    // No answer text is kept.
    expect(JSON.stringify(e)).not.toContain(c.outputs[0].text.slice(0, 40));
  });

  it("record partial, failed and cancelled runs as such", async () => {
    const fail: AttributionSample = { ok: false, text: null, kind: "timeout", error: "timed out", status: null, requests: 1, ms: 300_000, usage: null };
    const partial = summarize(await report([ok(c.outputs[0].text), fail, ok(c.outputs[2].text)]), "gpt-5.4");
    expect([outcome(partial), partial.valid, partial.scores!.calibration.queries]).toEqual(["incomplete", 2, "2"]);
    const failed = summarize(await report([fail, fail, fail]), "gpt-5.4");
    expect([outcome(failed), failed.scores]).toEqual(["failed", null]);
    const token = new CancelToken();
    token.cancel();
    const cancelled = summarize(await runAttribution({ runs: 1, bank, sample: async () => fail, token, challenges: challenges.slice(0, 1) }), "gpt-5.4");
    expect(outcome(cancelled)).toBe("cancelled");
  });

  it("read back only whole entries of this version, newest first", async () => {
    const e = historyEntry(target, null, "gpt-5.4", await report(c.outputs.map((o) => ok(o.text))), BANK_SOURCE.commit);
    const older = stored(e, 1000), newer = stored({ ...e, model: "gpt-5.4" }, 2000);
    const got = parseHistory([older, "junk", null, stored({ ...e, v: 2 }), stored({ ...e, rows: [{ index: 1 }] }),
      stored({ ...e, scores: { ...e.scores, own: { probability: 2, rank: 1 } } }), newer]);
    expect(got.map((x: HistoryEntry) => x.at)).toEqual([2000, 1000]);
    expect(got[1]).toEqual(older);
  });
});
