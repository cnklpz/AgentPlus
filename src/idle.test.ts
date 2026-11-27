import { describe, expect, it } from "vitest";
import { installIdle, isIdle } from "./idle";

/** A minimal event target that records listeners and fires them by name. */
function target() {
  const on = new Map<string, Set<() => void>>();
  return {
    addEventListener: (k: string, f: () => void) => { if (!on.has(k)) on.set(k, new Set()); on.get(k)!.add(f); },
    removeEventListener: (k: string, f: () => void) => { on.get(k)?.delete(f); },
    fire: (k: string) => on.get(k)?.forEach((f) => f()),
    count: () => [...on.values()].reduce((n, s) => n + s.size, 0),
  };
}

function setup(hidden: boolean, focused: boolean) {
  const win = target();
  const state = { hidden, focused };
  // A getter in a literal: Object.assign would copy `hidden` once instead of reading it live.
  const doc = {
    ...target(),
    get hidden() { return state.hidden; },
    hasFocus: () => state.focused,
    documentElement: { dataset: {} as DOMStringMap },
  };
  return { win, doc, state, idle: () => "idle" in doc.documentElement.dataset };
}

describe("isIdle", () => {
  it("is idle when hidden or unfocused, active only when shown and focused", () => {
    expect(isIdle({ hidden: false, hasFocus: () => true })).toBe(false);
    expect(isIdle({ hidden: false, hasFocus: () => false })).toBe(true);
    expect(isIdle({ hidden: true, hasFocus: () => true })).toBe(true);
    expect(isIdle({ hidden: true, hasFocus: () => false })).toBe(true);
  });
});

describe("installIdle", () => {
  it("sets the initial state right away", () => {
    const a = setup(false, true);
    installIdle(a.win, a.doc);
    expect(a.idle()).toBe(false);
    const b = setup(false, false);
    installIdle(b.win, b.doc);
    expect(b.idle()).toBe(true);
  });

  it("follows blur, focus and visibility changes", () => {
    const s = setup(false, true);
    installIdle(s.win, s.doc);
    s.state.focused = false; s.win.fire("blur");
    expect(s.idle()).toBe(true);
    s.state.focused = true; s.win.fire("focus");
    expect(s.idle()).toBe(false);
    // Hidden to the tray while still holding focus.
    s.state.hidden = true; s.doc.fire("visibilitychange");
    expect(s.idle()).toBe(true);
    s.state.hidden = false; s.doc.fire("visibilitychange");
    expect(s.idle()).toBe(false);
  });

  it("removes its listeners and the flag when stopped", () => {
    const s = setup(false, false);
    const stop = installIdle(s.win, s.doc);
    expect(s.win.count() + s.doc.count()).toBe(3);
    stop();
    expect(s.win.count() + s.doc.count()).toBe(0);
    expect(s.idle()).toBe(false);
    s.win.fire("blur");
    expect(s.idle()).toBe(false);
  });
});
