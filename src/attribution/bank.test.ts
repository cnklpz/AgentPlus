import { describe, expect, it } from "vitest";
import raw from "./unified_bank.json?raw";
import { ALIASES, BANK_SOURCE, VENDOR_PREFIXES, loadBank, matchCandidate, parseBank, validateBank } from "./bank";
import { AttributionError } from "./core";

const bank = parseBank(raw);
// A loose shape, to break on purpose.
type Loose = Record<string, any>;
const clone = () => JSON.parse(raw) as Loose;

/** The path `validateBank` reports for a broken copy. */
function brokenAt(edit: (b: Loose) => void): string {
  const b = clone();
  edit(b);
  try {
    validateBank(b);
  } catch (e) {
    expect(e).toBeInstanceOf(AttributionError);
    expect((e as AttributionError).code).toBe("bankInvalid");
    return (e as AttributionError).detail;
  }
  throw new Error("accepted a broken bank");
}

describe("bundled bank", () => {
  it("is the pinned upstream file, byte for byte (ignoring CRLF from checkout)", async () => {
    const bytes = new TextEncoder().encode(raw.replace(/\r\n/g, "\n"));
    const digest = await crypto.subtle.digest("SHA-256", bytes);
    const hex = [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
    expect(hex).toBe(BANK_SOURCE.sha256);
  });

  it("loads and validates", async () => {
    const b = await loadBank();
    expect(b.models.map((m) => m.id)).toEqual(bank.models.map((m) => m.id));
    expect(b.models).toHaveLength(18);
    expect(new Set(b.models.map((m) => m.family))).toEqual(new Set(["gpt", "claude"]));
  });
});

describe("validateBank", () => {
  it("names what is missing or malformed", () => {
    expect(brokenAt((b) => delete b.calibration["2"])).toBe("calibration.2");
    expect(brokenAt((b) => { b.calibration["3"].beta = 0; })).toBe("calibration.3.beta");
    expect(brokenAt((b) => { b.calibration["1"].beta = "7"; })).toBe("calibration.1.beta");
    expect(brokenAt((b) => delete b.calibration)).toBe("calibration");
    expect(brokenAt((b) => { b.models[3].counts.pop(); })).toBe("models[3].counts");
    expect(brokenAt((b) => { b.models[1].id = b.models[0].id; })).toBe("models[1].id");
    expect(brokenAt((b) => { b.models = b.models.slice(0, 1); })).toBe("models");
    expect(brokenAt((b) => { b.robust.model_order.reverse(); })).toBe("robust.model_order");
    expect(brokenAt((b) => { b.robust.hellinger.centroids.pop(); })).toBe("robust.hellinger.centroids");
    expect(brokenAt((b) => { b.robust.hellinger.feature_scale[7] = 0; })).toBe("robust.hellinger.feature_scale[7]");
    expect(brokenAt((b) => { b.robust.hellinger.feature_mean[2] = null; })).toBe("robust.hellinger.feature_mean[2]");
    expect(brokenAt((b) => delete b.robust.hellinger)).toBe("robust.hellinger");
    expect(brokenAt((b) => { b.robust.ordered_blocks.weight = 2; })).toBe("robust.ordered_blocks.weight");
    expect(brokenAt((b) => { b.robust.ordered_blocks.environment_centroids[4].pop(); })).toBe("robust.ordered_blocks.environment_centroids[4]");
    expect(brokenAt((b) => { b.robust.ordered_blocks.nuisance_basis[0].push(1); })).toBe("robust.ordered_blocks.nuisance_basis[0]");
  });

  it("refuses text that isn't JSON", () => {
    expect(() => parseBank("{\"models\": [")).toThrowError(expect.objectContaining({ code: "bankInvalid", detail: "JSON" }));
  });
});

describe("matchCandidate", () => {
  const m = (id: string) => matchCandidate(id, bank.models);

  it("matches every candidate by its exact id", () => {
    for (const model of bank.models) expect(m(model.id)).toEqual({ candidate: model.id, via: "exact" });
  });

  it("maps only the listed aliases and vendor prefixes", () => {
    expect(m("claude-haiku-4-5")).toEqual({ candidate: "claude-haiku-4-5-20251001", via: "alias" });
    expect(m("openai/gpt-5.5")).toEqual({ candidate: "gpt-5.5", via: "prefix" });
    expect(m("anthropic/claude-opus-4-7")).toEqual({ candidate: "claude-opus-4-7", via: "prefix" });
    expect(m("anthropic/claude-haiku-4-5")).toEqual({ candidate: "claude-haiku-4-5-20251001", via: "prefix" });
  });

  it("leaves other models unsupported", () => {
    for (const id of [
      "", "gpt", "gpt-5", "gpt-5.5-codex", "gpt-5.5-mini", "GPT-5.5", " gpt-5.5", "gpt-5.5 ", "my-gpt-5.5",
      "claude", "claude-opus-4", "claude-opus-4-7-20260101", "claude-haiku-4-5-latest", "claude-sonnet-4-6-thinking",
      // A vendor prefix must name its own family.
      "openai/claude-opus-4-7", "anthropic/gpt-5.5", "openrouter/gpt-5.5", "openai/", "anthropic/claude",
      "deepseek-chat", "gemini-3-pro",
    ]) expect(m(id)).toBeNull();
  });

  it("only aliases candidates the bank has", () => {
    const ids = new Set(bank.models.map((x) => x.id));
    for (const [alias, id] of Object.entries(ALIASES)) {
      expect(ids.has(id)).toBe(true);
      expect(ids.has(alias)).toBe(false);
    }
    const families = new Set(bank.models.map((x) => x.family));
    for (const family of Object.values(VENDOR_PREFIXES)) expect(families.has(family)).toBe(true);
  });
});
