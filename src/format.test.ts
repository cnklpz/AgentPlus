import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";
import { fmtAgo, fmtNum, fmtSecs, fmtSize, joinList } from "./format";
import { setLang } from "./i18n";

describe("fmtSize", () => {
  it.each([
    [0, "0 KB"],
    [-5, "0 KB"],
    [Number.NaN, "0 KB"],
    [Number.POSITIVE_INFINITY, "0 KB"],
    [1, "1 KB"],
    [1023, "1 KB"],
    [1024, "1 KB"],
    [1536, "2 KB"],
    [1_048_575, "1,024 KB"],
    [1_048_576, "1.0 MB"],
    [2_711_230, "2.6 MB"],
    [1024 ** 3, "1.0 GB"],
    [5.5 * 1024 ** 4, "5,632.0 GB"],
  ])("%d → %s", (b, s) => expect(fmtSize(b)).toBe(s));
});

describe("fmtNum / fmtSecs", () => {
  it("formats with a fixed number of decimals and grouping", () => {
    expect(fmtNum(1234.56, 1)).toBe("1,234.6");
    expect(fmtNum(0.5, 0)).toBe("1");
    expect(fmtNum(99.5, 1)).toBe("99.5");
  });
  it("turns milliseconds into seconds", () => {
    expect(fmtSecs(1234)).toBe("1.2");
    expect(fmtSecs(1234, 2)).toBe("1.23");
    expect(fmtSecs(12_600, 0)).toBe("13");
    expect(fmtSecs(0, 2)).toBe("0.00");
  });
});

describe("joinList", () => {
  it("joins with the Chinese list comma", () => {
    expect(joinList(["Codex", "OpenCode"])).toBe("Codex、OpenCode");
    expect(joinList([])).toBe("");
  });
});

describe("fmtAgo", () => {
  const now = new Date(2026, 8, 24, 12, 0, 0).getTime();
  const ago = (secs: number) => fmtAgo(now - secs * 1000, now);

  it("is empty for missing or unreadable times", () => {
    expect(fmtAgo(null, now)).toBe("");
    expect(fmtAgo(undefined, now)).toBe("");
    expect(fmtAgo("", now)).toBe("");
    expect(fmtAgo("not a date", now)).toBe("");
    expect(fmtAgo(Number.NaN, now)).toBe("");
  });

  it("counts the future (clock skew) as just now", () => {
    expect(ago(-3600)).toBe("刚刚");
  });

  it.each([
    [0, "刚刚"],
    [59, "刚刚"],
    [60, "1 分钟前"],
    [3599, "59 分钟前"],
    [3600, "1 小时前"],
    [86399, "23 小时前"],
    [86400, "昨天"],
    [2 * 86400 - 1, "昨天"],
    [2 * 86400, "2 天前"],
    [7 * 86400 - 1, "6 天前"],
  ])("%d s ago → %s", (s, text) => expect(ago(s)).toBe(text));

  it("reads ISO strings", () => {
    expect(fmtAgo(new Date(now - 5 * 60_000).toISOString(), now)).toBe("5 分钟前");
  });

  it("shows a date after a week, with the year when it differs", () => {
    expect(ago(10 * 86400)).toBe(new Date(now - 10 * 86400_000).toLocaleDateString("zh-CN", { month: "short", day: "numeric" }));
    const old = new Date(2024, 0, 5).getTime();
    expect(fmtAgo(old, now)).toContain("2024");
  });

  describe("in English", () => {
    beforeAll(() => {
      vi.stubGlobal("document", { documentElement: { lang: "" } });
      void setLang("en");
    });
    afterAll(() => {
      void setLang("zh");
      vi.unstubAllGlobals();
    });

    it("uses singular and plural forms", () => {
      expect(ago(30)).toBe("Just now");
      expect(ago(60)).toBe("1 minute ago");
      expect(ago(120)).toBe("2 minutes ago");
      expect(ago(3600)).toBe("1 hour ago");
      expect(ago(86400)).toBe("Yesterday");
      expect(ago(3 * 86400)).toBe("3 days ago");
    });

    it("joins lists with commas", () => {
      expect(joinList(["Codex", "OpenCode", "Kilo"])).toBe("Codex, OpenCode, Kilo");
    });

    it("formats sizes with English separators", () => {
      expect(fmtSize(1_048_575)).toBe("1,024 KB");
      expect(fmtSize(3.25 * 1024 * 1024)).toBe("3.3 MB");
    });
  });
});
