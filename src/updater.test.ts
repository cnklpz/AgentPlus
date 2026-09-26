import { beforeEach, describe, expect, it, vi } from "vitest";
import type { UpdateInfo, UpdateProgress } from "./api";

const mock = vi.hoisted(() => ({
  updateCheck: vi.fn<() => Promise<UpdateInfo | null>>(),
  updateInstall: vi.fn<(on: (p: UpdateProgress) => void) => Promise<void>>(),
}));
vi.mock("./api", () => ({ api: mock }));

const { checkUpdate, installUpdate, resetUpdate, takeJustUpdated, updateState } = await import("./updater");

const INFO: UpdateInfo = { version: "0.2.0", current: "0.1.0", notes: null, date: null };

const storage = new Map<string, string>();
const fakeStorage = {
  getItem: (k: string) => storage.get(k) ?? null,
  setItem: (k: string, v: string) => void storage.set(k, v),
  removeItem: (k: string) => void storage.delete(k),
};
vi.stubGlobal("localStorage", fakeStorage);

beforeEach(() => {
  storage.clear();
  resetUpdate();
  mock.updateCheck.mockReset();
  mock.updateInstall.mockReset();
});

describe("checkUpdate", () => {
  it("reports a newer release, or that this is the latest", async () => {
    mock.updateCheck.mockResolvedValueOnce(INFO);
    expect(await checkUpdate()).toEqual(INFO);
    expect(updateState()).toEqual({ kind: "available", info: INFO });
    mock.updateCheck.mockResolvedValueOnce(null);
    expect(await checkUpdate()).toBeNull();
    expect(updateState()).toEqual({ kind: "latest" });
  });

  it("shows a failure only when asked by the user", async () => {
    mock.updateCheck.mockRejectedValue("offline");
    await checkUpdate(true);
    expect(updateState()).toEqual({ kind: "idle" });
    await checkUpdate();
    expect(updateState()).toEqual({ kind: "error", error: "offline", info: null });
  });

  it("does not start a second check while one runs", async () => {
    let done: (v: UpdateInfo | null) => void = () => undefined;
    mock.updateCheck.mockReturnValueOnce(new Promise((r) => { done = r; }));
    const first = checkUpdate();
    expect(await checkUpdate()).toBeNull();
    expect(mock.updateCheck).toHaveBeenCalledTimes(1);
    done(null);
    await first;
  });
});

describe("installUpdate", () => {
  it("does nothing before an update was found", async () => {
    await installUpdate();
    expect(mock.updateInstall).not.toHaveBeenCalled();
  });

  it("follows progress, and keeps the update after a failure so it can be retried", async () => {
    mock.updateCheck.mockResolvedValueOnce(INFO);
    await checkUpdate();
    const seen: string[] = [];
    mock.updateInstall.mockImplementationOnce(async (on) => {
      on({ kind: "download", done: 5, total: 10 });
      seen.push(JSON.stringify(updateState()));
      on({ kind: "install" });
      seen.push(updateState().kind);
      throw "bad signature";
    });
    await installUpdate();
    expect(seen).toEqual([JSON.stringify({ kind: "downloading", info: INFO, done: 5, total: 10 }), "installing"]);
    expect(updateState()).toEqual({ kind: "error", error: "bad signature", info: INFO });
    mock.updateInstall.mockResolvedValueOnce(undefined);
    await installUpdate();
    expect(mock.updateInstall).toHaveBeenCalledTimes(2);
  });
});

describe("takeJustUpdated", () => {
  const install = async () => {
    mock.updateCheck.mockResolvedValueOnce(INFO);
    await checkUpdate();
    mock.updateInstall.mockResolvedValueOnce(undefined);
    await installUpdate();
  };

  it("names the new version once, on the first start after the install", async () => {
    await install();
    expect(takeJustUpdated("0.2.0")).toBe("0.2.0");
    expect(takeJustUpdated("0.2.0")).toBeNull();
  });

  it("says nothing when the old version started again (installer cancelled)", async () => {
    await install();
    expect(takeJustUpdated("0.1.0")).toBeNull();
    expect(takeJustUpdated("0.2.0")).toBeNull();
  });

  it("ignores a leading v", async () => {
    await install();
    expect(takeJustUpdated("v0.2.0")).toBe("0.2.0");
  });

  it("forgets a failed install", async () => {
    mock.updateCheck.mockResolvedValueOnce(INFO);
    await checkUpdate();
    mock.updateInstall.mockRejectedValueOnce("offline");
    await installUpdate();
    expect(takeJustUpdated("0.2.0")).toBeNull();
  });

  it("is quiet on a normal start or without storage", () => {
    expect(takeJustUpdated("0.2.0")).toBeNull();
    vi.stubGlobal("localStorage", undefined);
    try {
      expect(takeJustUpdated("0.2.0")).toBeNull();
    } finally {
      vi.stubGlobal("localStorage", fakeStorage);
    }
  });
});
