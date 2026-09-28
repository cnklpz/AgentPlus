// Attribution results kept across runs. The backend only stores the entries (see
// attribution::history); they are built here from a finished run, and every entry read back
// is checked: one that doesn't fit (another version, a damaged file) is left out.
import type { AttributionTarget } from "../api";
import { candidateStanding } from "./core";
import type { RunReport, SampleRow, SampleStatus } from "./run";

export const HISTORY_VERSION = 1;
/** Candidates kept per result. */
const TOP = 3;

/** What a run came to, as shown live and as kept. */
export interface ResultSummary {
  requested: number;
  requests: number;
  requestsExact: boolean;
  valid: number;
  cancelled: boolean;
  rows: SampleRow[];
  /** Null when no answer counted. */
  scores: {
    /** The tested model's own global probability and rank (not the winner's). */
    own: { probability: number; rank: number } | null;
    candidates: number;
    top: { model: string; displayName: string; probability: number }[];
    families: { family: string; displayName: string; probability: number }[];
    calibration: { queries: string; beta: number };
  } | null;
}

export interface HistoryEntry extends ResultSummary {
  /** Set by the backend. */
  id: string;
  /** Unix milliseconds, set by the backend. */
  at: number;
  v: typeof HISTORY_VERSION;
  agent: string;
  /** The provider tested (for Codex's catalog, the one it was on). */
  provider: string;
  providerName: string | null;
  /** The model id requested. */
  model: string;
  /** The bank candidate it was compared as. */
  candidate: string;
  api: string;
  url: string;
  /** The gateway forward the requests went through. */
  forward: string | null;
  /** Commit of the bank used. */
  bank: string;
}

export type Outcome = "complete" | "incomplete" | "cancelled" | "failed";

export function summarize(r: RunReport, candidate: string): ResultSummary {
  const a = r.analysis;
  const own = a ? candidateStanding(a, candidate) : null;
  return {
    requested: r.requested,
    requests: r.requests,
    requestsExact: r.requestsExact,
    valid: r.valid,
    cancelled: r.cancelled,
    rows: r.rows.map((x) => ({ ...x })),
    scores: a && {
      own: own && { probability: own.result.probability, rank: own.rank },
      candidates: a.results.length,
      top: a.results.slice(0, TOP).map((x) => ({ model: x.model, displayName: x.displayName, probability: x.probability })),
      families: a.families.map((f) => ({ ...f })),
      calibration: { ...a.calibration },
    },
  };
}

/** The entry to keep for a run of `target` (the backend adds `id` and `at`). */
export function historyEntry(target: AttributionTarget, providerName: string | null, candidate: string, r: RunReport, bank: string): Omit<HistoryEntry, "id" | "at"> {
  return {
    v: HISTORY_VERSION,
    agent: target.agent,
    provider: target.provider,
    providerName,
    model: target.model,
    candidate,
    api: target.api,
    url: target.url,
    forward: target.gateway?.route ?? null,
    bank,
    ...summarize(r, candidate),
  };
}

export function outcome(s: ResultSummary): Outcome {
  if (!s.scores) return s.cancelled ? "cancelled" : "failed";
  if (s.cancelled) return "cancelled";
  return s.valid < s.requested ? "incomplete" : "complete";
}

// ---------------------------------------------------------------- reading back

type Loose = Record<string, unknown>;
const STATUSES: readonly SampleStatus[] = ["pending", "running", "valid", "tooShort", "failed", "discarded", "skipped"];
const isObj = (v: unknown): v is Loose => !!v && typeof v === "object" && !Array.isArray(v);
const str = (v: unknown) => typeof v === "string";
const num = (v: unknown) => typeof v === "number" && Number.isFinite(v);
const strOrNull = (v: unknown) => v === null || str(v);
const numOrNull = (v: unknown) => v === null || num(v);
const prob = (v: unknown) => num(v) && (v as number) >= 0 && (v as number) <= 1;

function isRow(v: unknown): boolean {
  return isObj(v) && num(v.index) && num(v.expectedCount) && STATUSES.includes(v.status as SampleStatus) && strOrNull(v.kind) && strOrNull(v.error)
    && numOrNull(v.httpStatus) && num(v.requests) && num(v.ms) && numOrNull(v.parsedNumbers) && num(v.minimumNumbers);
}

function isScores(v: unknown): boolean {
  if (v === null) return true;
  if (!isObj(v) || !num(v.candidates) || !Array.isArray(v.top) || !Array.isArray(v.families) || !isObj(v.calibration)) return false;
  const own = v.own;
  if (own !== null && !(isObj(own) && prob(own.probability) && num(own.rank))) return false;
  return v.top.every((x) => isObj(x) && str(x.model) && str(x.displayName) && prob(x.probability))
    && v.families.every((x) => isObj(x) && str(x.family) && str(x.displayName) && prob(x.probability))
    && str(v.calibration.queries) && num(v.calibration.beta);
}

function isEntry(v: unknown): v is HistoryEntry {
  return isObj(v) && v.v === HISTORY_VERSION && str(v.id) && num(v.at)
    && [v.agent, v.provider, v.model, v.candidate, v.api, v.url, v.bank].every(str) && strOrNull(v.providerName) && strOrNull(v.forward)
    && num(v.requested) && num(v.requests) && typeof v.requestsExact === "boolean" && num(v.valid) && typeof v.cancelled === "boolean"
    && Array.isArray(v.rows) && v.rows.every(isRow) && isScores(v.scores);
}

/** The entries the backend returned that are whole results of this version, newest first. */
export function parseHistory(list: unknown[]): HistoryEntry[] {
  return list.filter(isEntry).sort((a, b) => b.at - a.at);
}
