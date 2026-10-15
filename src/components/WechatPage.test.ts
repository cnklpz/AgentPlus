import { describe, expect, it } from "vitest";
import { aboutSession } from "./WechatPage";

describe("aboutSession", () => {
  it("matches the session's tag and number, not longer numbers", () => {
    expect(aboutSession("【#3 Fix updater】\n\nDone", 3)).toBe(true);
    expect(aboutSession("#3 run the tests", 3)).toBe(true);
    expect(aboutSession("Stopping #3…", 3)).toBe(true);
    expect(aboutSession("【#31 other】", 3)).toBe(false);
    expect(aboutSession("【#13 other】", 3)).toBe(false);
    expect(aboutSession("plain text", 3)).toBe(false);
  });
});
