// Dictionary consistency: zh/ must say the same thing as the English source en/, with the
// same placeholders. (The key sets themselves are enforced by `const zh: typeof en`.)
import { describe, expect, it } from "vitest";
import en from "./en";
import zh from "./zh";

type Flat = Record<string, string>;
const flat = (d: object): Flat =>
  Object.fromEntries(Object.entries(d).flatMap(([ns, keys]) => Object.entries(keys as Flat).map(([k, v]) => [`${ns}.${k}`, v])));
const EN = flat(en);
const ZH = flat(zh);
const vars = (s: string) => [...new Set([...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]))].sort();

/** Source files outside i18n/ (tests excluded), as text; loaded by Vite, so no Node types are needed. */
const SOURCES = import.meta.glob<string>(["../**/*.{ts,tsx}", "!../i18n/**", "!../**/*.test.ts"], { query: "?raw", import: "default", eager: true });

/** Every `tn("ns.key"` call in the sources. */
function tnKeys(): Set<string> {
  const out = new Set<string>();
  for (const text of Object.values(SOURCES)) {
    for (const m of text.matchAll(/\btn\(\s*"([\w.]+)"/g)) out.add(m[1]);
  }
  return out;
}

describe("dictionaries", () => {
  it("have the same keys", () => {
    expect(Object.keys(EN).length).toBeGreaterThan(500);
    expect(tnKeys().size).toBeGreaterThan(20);
    expect(Object.keys(ZH).sort()).toEqual(Object.keys(EN).sort());
  });

  it("use the same placeholders in both languages", () => {
    const bad = Object.keys(EN).filter((k) => {
      const z = vars(ZH[k]);
      // Plural entries: every English form uses {n} plus a subset of the placeholders of the
      // (single-form) Chinese translation. Other entries must match it exactly.
      return EN[k].split("|").some((form, _, all) => {
        const e = vars(form);
        return all.length > 1 ? e.some((v) => v !== "n" && !z.includes(v)) : e.join() !== z.join();
      });
    });
    expect(bad).toEqual([]);
  });

  it("have no empty entries", () => {
    expect(Object.keys(EN).filter((k) => !EN[k].trim() || !ZH[k].trim())).toEqual([]);
  });

  it("have no Chinese left in English", () => {
    expect(Object.keys(EN).filter((k) => /[㐀-鿿　-〿！-～]/.test(EN[k]))).toEqual([]);
  });

  it("write Chinese plurals in a single form", () => {
    expect(Object.keys(ZH).filter((k) => ZH[k].includes("|"))).toEqual([]);
  });

  it("give English plurals exactly two forms", () => {
    expect(Object.keys(EN).filter((k) => EN[k].split("|").length > 2)).toEqual([]);
  });

  it("have no unused keys", () => {
    // Keys are always written out whole ("ns.key"), also in TKey tables; none are built from parts.
    const all = Object.values(SOURCES).join("\n");
    expect(Object.keys(EN).filter((k) => !all.includes(`"${k}"`))).toEqual([]);
  });

  it("only count-format keys that exist", () => {
    expect([...tnKeys()].filter((k) => !(k in EN))).toEqual([]);
  });

  it("leave no unbalanced braces", () => {
    const unbalanced = (s: string) => s.replace(/\{\w+\}/g, "").match(/[{}]/);
    expect(Object.keys(EN).filter((k) => unbalanced(EN[k]) || unbalanced(ZH[k]))).toEqual([]);
  });
});
