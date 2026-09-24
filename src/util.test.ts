import { afterEach, describe, expect, it, vi } from "vitest";
import { copyText, errText, isHttpUrl, onActivateKey, toggled, toggledIn } from "./util";

describe("errText", () => {
  it("keeps backend rejections (strings) as they are", () => {
    expect(errText("找不到供应商 x")).toBe("找不到供应商 x");
  });
  it("drops the \"Error: \" prefix of thrown errors", () => {
    expect(errText(new Error("该分组没有可用的来源"))).toBe("该分组没有可用的来源");
  });
  it("stringifies anything else", () => {
    expect(errText(42)).toBe("42");
    expect(errText(null)).toBe("null");
  });
});

describe("toggled / toggledIn", () => {
  it("adds a missing value and removes a present one, without touching the input", () => {
    const s = new Set(["a"]);
    expect([...toggled(s, "b")]).toEqual(["a", "b"]);
    expect([...toggled(s, "a")]).toEqual([]);
    expect([...s]).toEqual(["a"]);
    const a = ["a", "b"];
    expect(toggledIn(a, "c")).toEqual(["a", "b", "c"]);
    expect(toggledIn(a, "a")).toEqual(["b"]);
    expect(a).toEqual(["a", "b"]);
  });
});

describe("isHttpUrl", () => {
  it.each([
    ["https://api.example.com/v1", true],
    ["http://127.0.0.1:18650", true],
    ["  https://x.io  ", true],
    ["ftp://x.io", false],
    ["api.example.com", false],
    ["https://", false],
    ["https://a b", false],
    ["", false],
  ])("%s → %s", (s, ok) => expect(isHttpUrl(s)).toBe(ok));
});

describe("onActivateKey", () => {
  const ev = (key: string, self = true) => {
    const el = {};
    return { key, target: el, currentTarget: self ? el : {}, preventDefault: vi.fn() };
  };
  it("activates on Enter and Space on the element itself", () => {
    const fn = vi.fn();
    const h = onActivateKey(fn);
    for (const k of ["Enter", " "]) {
      const e = ev(k);
      h(e as never);
      expect(e.preventDefault).toHaveBeenCalled();
    }
    expect(fn).toHaveBeenCalledTimes(2);
  });
  it("ignores other keys and keys meant for a child", () => {
    const fn = vi.fn();
    const h = onActivateKey(fn);
    const other = ev("a");
    const child = ev("Enter", false);
    h(other as never);
    h(child as never);
    expect(fn).not.toHaveBeenCalled();
    expect(child.preventDefault).not.toHaveBeenCalled();
  });
});

describe("copyText", () => {
  afterEach(() => vi.unstubAllGlobals());
  it("says it copied, or that it failed", async () => {
    const flash = vi.fn();
    vi.stubGlobal("navigator", { clipboard: { writeText: vi.fn().mockResolvedValue(undefined) } });
    await copyText("x", flash);
    expect(flash).toHaveBeenLastCalledWith("已复制");
    await copyText("x", flash, "已复制地址");
    expect(flash).toHaveBeenLastCalledWith("已复制地址");
    vi.stubGlobal("navigator", { clipboard: { writeText: vi.fn().mockRejectedValue(new Error("denied")) } });
    await copyText("x", flash);
    expect(flash).toHaveBeenLastCalledWith("复制失败", true);
  });
});
