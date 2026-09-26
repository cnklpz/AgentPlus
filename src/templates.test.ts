import { describe, expect, it } from "vitest";
import { TEMPLATES, modelsAfter, modelsFor, modelsOn } from "./templates";

describe("TEMPLATES", () => {
  it("offers the default protocol and has an address for every protocol with models", () => {
    for (const tpl of TEMPLATES) {
      expect(tpl.endpoints[tpl.api], tpl.id).toBeTruthy();
      for (const k of Object.keys(tpl.apiModels ?? {}) as (keyof typeof tpl.endpoints)[]) {
        expect(tpl.endpoints[k], `${tpl.id} ${k}`).toBeTruthy();
        expect(k, `${tpl.id}: the default protocol's models belong in models`).not.toBe(tpl.api);
      }
    }
  });
  it("warns about the session header only for OpenCode Go", () => {
    expect(TEMPLATES.filter((x) => x.session).map((x) => x.id)).toEqual(["opencode-go"]);
  });
  it("has unique ids", () => {
    expect(new Set(TEMPLATES.map((x) => x.id)).size).toBe(TEMPLATES.length);
  });
});

describe("modelsFor", () => {
  const go = TEMPLATES.find((x) => x.id === "opencode-go")!;
  const glm = TEMPLATES.find((x) => x.id === "glm")!;
  it("follows the protocol when the vendor serves each model on one protocol", () => {
    expect(modelsFor(go, "chat")).toBe(go.models);
    expect(modelsFor(go, "anthropic")).toContain("minimax-m3");
    expect(modelsFor(go, "responses")).toContain("gpt-6-luna");
    expect(modelsFor(go, "anthropic")).not.toContain("glm-5.3");
  });
  it("is the one list for every protocol otherwise", () => {
    expect(modelsFor(glm, "anthropic")).toBe(glm.models);
    expect(modelsFor(go, "gemini")).toBe(go.models);
  });
});

describe("modelsOn / modelsAfter", () => {
  const go = TEMPLATES.find((x) => x.id === "opencode-go")!;
  const glm = TEMPLATES.find((x) => x.id === "glm")!;
  it("splits the ticked models by the protocol that serves them; hand-added ones go to every protocol", () => {
    const checked = ["glm-5.3", "minimax-m3", "gpt-6-luna", "my-model"];
    expect(modelsOn(go, "chat", checked)).toEqual(["glm-5.3", "my-model"]);
    expect(modelsOn(go, "anthropic", checked)).toEqual(["minimax-m3", "my-model"]);
    expect(modelsOn(go, "responses", checked)).toEqual(["gpt-6-luna", "my-model"]);
  });
  it("gives every protocol everything without a template, or when the template has one list", () => {
    expect(modelsOn(null, "anthropic", ["a", "b"])).toEqual(["a", "b"]);
    expect(modelsOn(glm, "anthropic", ["glm-5.3", "x"])).toEqual(["glm-5.3", "x"]);
  });
  it("ticks an added protocol's models and unticks the dropped one's", () => {
    const both = modelsAfter(go, ["chat"], ["chat", "anthropic"], ["glm-5.3", "my-model"]);
    expect(both).toEqual(["glm-5.3", "my-model", ...modelsFor(go, "anthropic")]);
    const back = modelsAfter(go, ["chat", "anthropic"], ["anthropic"], both);
    expect(back).toEqual(["my-model", ...modelsFor(go, "anthropic")]);
  });
});

describe("modelsAfter with one list for every protocol", () => {
  it("leaves an unticked model unticked when another protocol is added", () => {
    const glm = TEMPLATES.find((x) => x.id === "glm")!;
    expect(modelsAfter(glm, ["chat"], ["chat", "anthropic"], ["glm-5.3"])).toEqual(["glm-5.3"]);
  });
});
