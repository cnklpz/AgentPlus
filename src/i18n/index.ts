// UI language. Chinese (zh/) is the source dictionary; every other language must have
// exactly the same keys (en/ is type-checked against zh/). The backend renders its own
// text in the same language: `setLang` tells it via `set_locale`, and every `api` call
// waits for that first.
import { createElement, Fragment, useSyncExternalStore, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import zh from "./zh";
import en from "./en";

export type Lang = "zh" | "en";
/** The user's choice; "auto" follows the system language. */
export type LangPref = "auto" | Lang;

/** Language names, each written in its own language (never translated). */
export const LANGS: { id: Lang; label: string }[] = [
  { id: "zh", label: "简体中文" },
  { id: "en", label: "English" },
];

type Dict = typeof zh;
const DICTS: Record<Lang, Dict> = { zh, en };

/** "namespace.key", checked against the Chinese dictionary. */
export type TKey = { [N in keyof Dict]: `${N & string}.${keyof Dict[N] & string}` }[keyof Dict];
export type Vars = Record<string, string | number>;

let lang: Lang = "zh";
let started = false;
let ready: Promise<void> = Promise.resolve();
const subs = new Set<() => void>();
const inTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

export function resolveLang(p: LangPref): Lang {
  if (p !== "auto") return p;
  const sys = (navigator.languages?.[0] ?? navigator.language ?? "").toLowerCase();
  return sys.startsWith("zh") ? "zh" : "en";
}

/** Switch the UI (and the backend's text) to `p`. Call once before the first render. */
export function setLang(p: LangPref): Promise<void> {
  const next = resolveLang(p);
  if (started && next === lang) return ready;
  started = true;
  lang = next;
  document.documentElement.lang = next === "zh" ? "zh-CN" : "en";
  ready = inTauri ? invoke<void>("set_locale", { lang: next }).catch(() => undefined) : Promise.resolve();
  subs.forEach((f) => f());
  return ready;
}

/** Resolves once the backend speaks the current language (`api` calls wait for it). */
export const whenReady = () => ready;

export const getLang = () => lang;

/** BCP 47 tag for `toLocaleString` / `Intl`. */
export const locale = () => (lang === "zh" ? "zh-CN" : "en-US");

/** Re-render on language change. Call it in any component that caches translated text
 * (useMemo deps) or backend-rendered text (reload when it changes). */
export function useLang(): Lang {
  return useSyncExternalStore(
    (f) => { subs.add(f); return () => { subs.delete(f); }; },
    () => lang,
  );
}

function lookup(key: string): string {
  const dot = key.indexOf(".");
  const ns = key.slice(0, dot), k = key.slice(dot + 1);
  const get = (d: Dict) => (d as unknown as Record<string, Record<string, string> | undefined>)[ns]?.[k];
  return get(DICTS[lang]) ?? get(zh) ?? key;
}

const fill = (s: string, vars?: Vars) =>
  vars ? s.replace(/\{(\w+)\}/g, (m, n: string) => (n in vars ? String(vars[n]) : m)) : s;

/** Translated text; `{name}` placeholders are filled from `vars`. */
export function t(key: TKey, vars?: Vars): string {
  return fill(lookup(key), vars);
}

/** Count-dependent text: the entry is "one|other" ("{n} model|{n} models"); a single form
 * (all Chinese entries) is used for every count. `{n}` is filled with `n`. */
export function tn(key: TKey, n: number, vars?: Vars): string {
  const forms = lookup(key).split("|");
  const s = forms.length > 1 && n !== 1 ? forms[1] : forms[0];
  return fill(s, { n, ...vars });
}

/** Like `t`, but placeholders can be React nodes: tx("x.saved", { name: <b>{name}</b> }). */
export function tx(key: TKey, vars: Record<string, ReactNode>): ReactNode {
  const parts = lookup(key).split(/\{(\w+)\}/);
  return createElement(
    Fragment,
    null,
    ...parts.map((p, i) => (i % 2 ? createElement(Fragment, { key: i }, p in vars ? vars[p] : `{${p}}`) : p)),
  );
}
