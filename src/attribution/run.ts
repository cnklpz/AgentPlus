// One attribution run: N independent challenges (N = the runs picked, never topped up or
// retried), each sent once through `sample`, then the valid answers scored together.
import type { AttributionLive, AttributionSample } from "../api";
import type { Bank } from "./bank";
import { type Challenge, generateChallenges } from "./challenge";
import { type Analysis, analyzeOutputs, minimumNumbers, parseNumbers } from "./core";

export const RUN_CHOICES = [1, 2, 3] as const;
export type Runs = (typeof RUN_CHOICES)[number];

/** Per sample: queued, waiting for the answer, counted, too short to count, failed, sent but its
 *  answer dropped by a cancel, or never sent. */
export type SampleStatus = "pending" | "running" | "valid" | "tooShort" | "failed" | "discarded" | "skipped";

export interface SampleRow {
  /** 1-based. */
  index: number;
  expectedCount: number;
  status: SampleStatus;
  /** The backend's stable failure kind ("http", "truncated"…), or "error" when the call itself failed. */
  kind: string | null;
  /** Display text of a failure. */
  error: string | null;
  httpStatus: number | null;
  /** HTTP requests the backend sent for this sample. */
  requests: number;
  ms: number;
  parsedNumbers: number | null;
  minimumNumbers: number;
}

export interface RunReport {
  /** Samples picked. */
  requested: number;
  rows: SampleRow[];
  /** HTTP requests actually sent, over all samples. */
  requests: number;
  /** False when a sample was cancelled on its way: `requests` counts it as one, the least it sent. */
  requestsExact: boolean;
  /** Answers that count. */
  valid: number;
  cancelled: boolean;
  /** Null when no answer counts: no score is made up then. */
  analysis: Analysis | null;
}

/** Stops a run: the sample on its way is no longer waited for, later ones aren't sent, and
 *  the run reports nothing more through `onUpdate`. */
export class CancelToken {
  cancelled = false;
  private listeners: (() => void)[] = [];

  cancel(): void {
    if (this.cancelled) return;
    this.cancelled = true;
    this.listeners.splice(0).forEach((f) => f());
  }

  /** Resolves on cancel. */
  wait(): Promise<void> {
    return new Promise((resolve) => (this.cancelled ? resolve() : this.listeners.push(resolve)));
  }
}

export interface RunOptions {
  runs: Runs;
  bank: Bank;
  /** Sends one challenge, reporting its answer to `live` as it streams in; the backend reports
   *  failures in the result, a rejection is an error too. */
  sample: (c: Challenge, live: (e: AttributionLive) => void) => Promise<AttributionSample>;
  token: CancelToken;
  /** Progress, as fresh copies; never called once the token is cancelled. */
  onUpdate?: (r: RunReport) => void;
  /** The sample on its way (1-based index) as it streams; never after cancel, nor for a sample already done. */
  onLive?: (index: number, e: AttributionLive) => void;
  challenges?: Challenge[];
  /** Text for a call that failed outright. */
  errorText?: (e: unknown) => string;
}

function report(requested: number, rows: SampleRow[], cancelled: boolean, analysis: Analysis | null): RunReport {
  return {
    requested,
    rows: rows.map((r) => ({ ...r })),
    requests: rows.reduce((s, r) => s + r.requests, 0),
    requestsExact: !rows.some((r) => r.status === "discarded"),
    valid: rows.filter((r) => r.status === "valid").length,
    cancelled,
    analysis,
  };
}

export async function runAttribution(o: RunOptions): Promise<RunReport> {
  const challenges = o.challenges ?? generateChallenges(o.runs);
  const rows: SampleRow[] = challenges.map((c, i) => ({
    index: i + 1, expectedCount: c.expectedCount, status: "pending", kind: null, error: null, httpStatus: null,
    requests: 0, ms: 0, parsedNumbers: null, minimumNumbers: minimumNumbers(c.expectedCount),
  }));
  const emit = () => { if (!o.token.cancelled) o.onUpdate?.(report(challenges.length, rows, false, null)); };
  const outputs: { text: string; expectedCount: number }[] = [];
  const cancelled = o.token.wait().then(() => null);
  for (const [i, c] of challenges.entries()) {
    if (o.token.cancelled) break;
    const row = rows[i];
    row.status = "running";
    emit();
    let current = true;
    const live = (e: AttributionLive) => { if (current && !o.token.cancelled) o.onLive?.(row.index, e); };
    const reply = await Promise.race([
      o.sample(c, live).catch((e: unknown): AttributionSample => ({
        ok: false, text: null, kind: "error", error: o.errorText ? o.errorText(e) : String(e), status: null, requests: 0, ms: 0, usage: null,
      })),
      cancelled,
    ]);
    current = false;
    // Cancelled while it was on its way: its answer, when it comes, is dropped.
    if (!reply) {
      row.status = "discarded";
      row.requests = 1;
      break;
    }
    row.requests = reply.requests;
    row.ms = reply.ms;
    row.httpStatus = reply.status;
    if (reply.ok && reply.text != null) {
      const parsed = parseNumbers(reply.text).length;
      row.parsedNumbers = parsed;
      if (parsed >= row.minimumNumbers) {
        row.status = "valid";
        outputs.push({ text: reply.text, expectedCount: c.expectedCount });
      } else {
        row.status = "tooShort";
        row.kind = "tooShort";
      }
    } else {
      row.status = "failed";
      row.kind = reply.kind ?? "error";
      row.error = reply.error;
    }
    emit();
  }
  for (const row of rows) if (row.status === "pending" || row.status === "running") row.status = "skipped";
  // Each answer is scored on its own inside `analyzeOutputs`, which counts exactly the
  // answers marked valid above (same parser, same minimum).
  const analysis = outputs.length ? analyzeOutputs(outputs, o.bank) : null;
  return report(challenges.length, rows, o.token.cancelled, analysis);
}
