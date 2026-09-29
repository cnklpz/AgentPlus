import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "../api";
import type { Group, Use } from "../services";
import { fetchServiceModels } from "./ServiceDialog";

const url = "https://relay.example/v1";
const models = ["model-a", "model-b"];
const group = (over: Partial<Group> = {}): Group => ({
  key: "relay", name: "Relay", baseUrl: url, api: "responses", keyFp: "fp", keyHint: "••••test",
  lib: { id: "relay", name: "Relay", baseUrl: url, api: "responses", hasKey: true, keyFp: "fp", keyHint: "••••test", models: [] },
  uses: [], ...over,
});
const use = (over: Partial<Use> = {}): Use => ({
  agent: { id: "codex" }, p: { id: "relay", editable: true, isNew: false }, state: "on", models: 0, ...over,
} as Use);

beforeEach(() => {
  vi.spyOn(api, "fetchModelsLib").mockResolvedValue(models);
  vi.spyOn(api, "fetchModels").mockResolvedValue(models);
  vi.spyOn(api, "fetchModelsUrl").mockResolvedValue(models);
});
afterEach(() => vi.restoreAllMocks());

describe("fetchServiceModels", () => {
  it("uses the saved library key when a duplicate import leaves the key field empty", async () => {
    expect(await fetchServiceModels(group(), url, "responses", "")).toEqual(models);
    expect(api.fetchModelsLib).toHaveBeenCalledWith("relay");
    expect(api.fetchModelsUrl).not.toHaveBeenCalled();
    expect(api.fetchModels).not.toHaveBeenCalled();
  });

  it("reads an agent's credentials when the group has no library entry", async () => {
    await fetchServiceModels(group({ lib: null, uses: [use()] }), url, "responses", "");
    expect(api.fetchModels).toHaveBeenCalledWith("codex", "relay");
    expect(api.fetchModelsUrl).not.toHaveBeenCalled();
  });

  it("uses a newly entered key instead of the stored one", async () => {
    await fetchServiceModels(group(), url, "responses", " sk-new ");
    expect(api.fetchModelsUrl).toHaveBeenCalledWith(url, "sk-new", "responses");
    expect(api.fetchModelsLib).not.toHaveBeenCalled();
    expect(api.fetchModels).not.toHaveBeenCalled();
  });

  it("does not reuse credentials or fetch the old list after the address changes", async () => {
    await fetchServiceModels(group({ uses: [use()] }), "https://other.example/v1", "responses", "");
    expect(api.fetchModelsUrl).toHaveBeenCalledWith("https://other.example/v1", null, "responses");
    expect(api.fetchModelsLib).not.toHaveBeenCalled();
    expect(api.fetchModels).not.toHaveBeenCalled();
  });

  it("uses the requested protocol instead of the stored one after it changes", async () => {
    await fetchServiceModels(group({ uses: [use()] }), url, "chat", "");
    expect(api.fetchModelsUrl).toHaveBeenCalledWith(url, null, "chat");
    expect(api.fetchModelsLib).not.toHaveBeenCalled();
    expect(api.fetchModels).not.toHaveBeenCalled();
  });

  it("accepts a stored URL's trailing slashes", async () => {
    await fetchServiceModels(group({ baseUrl: `${url}/` }), url, "responses", "");
    expect(api.fetchModelsLib).toHaveBeenCalledWith("relay");
  });

  it("does not use an agent entry being removed", async () => {
    await fetchServiceModels(group({ lib: null, uses: [use({ state: "removing" })] }), url, "responses", "");
    expect(api.fetchModelsUrl).toHaveBeenCalledWith(url, null, "responses");
    expect(api.fetchModels).not.toHaveBeenCalled();
  });

  it.each([["sk-imported", "sk-imported"], ["", null]])("fetches a new provider with key %j", async (key, sent) => {
    await fetchServiceModels(null, url, "responses", key!);
    expect(api.fetchModelsUrl).toHaveBeenCalledWith(url, sent, "responses");
    expect(api.fetchModelsLib).not.toHaveBeenCalled();
  });
});
