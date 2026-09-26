import { describe, expect, it } from "vitest";
import { type WheelTrack, isMomentum } from "./wheel";

const track = (): WheelTrack => ({ at: -1000, last: 0, decaying: 0, coasting: false });
/** Feeds events 16ms apart; returns what each was judged to be. */
const feed = (t: WheelTrack, sizes: number[], start = 0) => sizes.map((s, i) => isMomentum(t, start + i * 16, s));

describe("isMomentum", () => {
  it("takes a steady push for the fingers", () => {
    expect(feed(track(), [20, 24, 22, 25, 23])).toEqual([false, false, false, false, false]);
  });

  it("takes a mouse wheel's equal notches for the user", () => {
    expect(feed(track(), [100, 100, 100, 100])).toEqual([false, false, false, false]);
  });

  it("spots the decaying stream after the fingers lift", () => {
    expect(feed(track(), [40, 30, 22, 16, 11, 11, 7])).toEqual([false, false, false, true, true, true, true]);
  });

  it("gives the fingers back on a harder push", () => {
    const t = track();
    feed(t, [40, 30, 22, 16]);
    expect(isMomentum(t, 64 + 16, 30)).toBe(false);
  });

  it("gives the fingers back after a pause", () => {
    const t = track();
    feed(t, [40, 30, 22, 16]);
    expect(isMomentum(t, 64 + 200, 10)).toBe(false);
  });
});
