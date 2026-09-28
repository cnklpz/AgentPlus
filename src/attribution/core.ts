/*! Adapted from ModelTrace static/fingerprint-core.js (commit df3a0f9d3e054c0dc02d6d586686db8daf8fa7c8),
 * https://github.com/xqy2006/ModelTrace. Copyright (c) 2026 xqy2006. MIT License; the full
 * notice is in src/attribution/LICENSE-ModelTrace.txt and shown in the attribution dialog. */

// Number fingerprint scoring, a faithful port of the reference JavaScript core: the same
// parser, features, per-answer scores, averaging, single global softmax and calibration.
// Pure functions without any I/O or text for the user; errors carry a stable `code`.

import type { Bank } from "./bank";

export const VALUE_MIN = 1;
export const VALUE_MAX = 355;
export const DIMENSION = VALUE_MAX - VALUE_MIN + 1;
export const ALPHA = 0.5;
export const ORDERED_BLOCK_WEIGHT = 0.25;
/** Answers shorter than this never count, whatever the requested length. */
export const MIN_NUMBERS = 80;

export type AttributionErrorCode = "noValidOutputs" | "bankInvalid" | "bankMissing";

/** A failure the UI words itself; `detail` is technical (a path in the bank), not translated. */
export class AttributionError extends Error {
  constructor(readonly code: AttributionErrorCode, readonly detail = "") {
    super(detail ? `${code}: ${detail}` : code);
    this.name = "AttributionError";
  }
}

/**
 * The longest run of in-range integers. A letter (any script) between two numbers splits
 * runs; numbers outside 1..355 are skipped without splitting. Only ASCII digits count.
 */
export function parseNumbers(text: string): number[] {
  const source = String(text);
  const runs: number[][] = [];
  let current: number[] = [];
  let previousEnd = 0;
  for (const match of source.matchAll(/\d+/g)) {
    const at = match.index ?? 0;
    const separator = source.slice(previousEnd, at);
    const value = Number(match[0]);
    if (current.length && /\p{L}/u.test(separator)) {
      runs.push(current);
      current = [];
    }
    if (value >= VALUE_MIN && value <= VALUE_MAX) current.push(value);
    previousEnd = at + match[0].length;
  }
  if (current.length) runs.push(current);
  return runs.reduce<number[]>((best, run) => (run.length > best.length ? run : best), []);
}

/** How many parsed numbers an answer needs to count: max(80, ceil(0.55 × requested)). */
export function minimumNumbers(expectedCount: number): number {
  return expectedCount ? Math.max(MIN_NUMBERS, Math.ceil(expectedCount * 0.55)) : MIN_NUMBERS;
}

export function countNumbers(numbers: readonly number[]): number[] {
  const counts = Array<number>(DIMENSION).fill(0);
  numbers.forEach((number) => { counts[number - VALUE_MIN] += 1; });
  return counts;
}

function mean(values: readonly number[]): number {
  return values.reduce((total, value) => total + value, 0) / values.length;
}

function standardize(values: readonly number[]): number[] {
  const center = mean(values);
  const variance = mean(values.map((value) => (value - center) ** 2));
  const scale = Math.max(Math.sqrt(variance), 1e-12);
  return values.map((value) => (value - center) / scale);
}

function dot(left: readonly number[], right: readonly number[]): number {
  let value = 0;
  for (let index = 0; index < left.length; index += 1) value += left[index] * right[index];
  return value;
}

function norm(values: readonly number[]): number {
  return Math.sqrt(dot(values, values));
}

function normalized(values: readonly number[]): number[] {
  const scale = Math.max(norm(values), 1e-12);
  return values.map((value) => value / scale);
}

function subtractBasis(values: readonly number[], basis: readonly (readonly number[])[] | undefined): number[] {
  const output = values.slice();
  for (const vector of basis || []) {
    const projection = dot(output, vector);
    for (let index = 0; index < output.length; index += 1) output[index] -= projection * vector[index];
  }
  return output;
}

function hellingerFeature(counts: readonly number[]): number[] {
  const total = counts.reduce((sum, value) => sum + value, 0) + ALPHA * DIMENSION;
  return counts.map((value) => Math.sqrt((value + ALPHA) / total));
}

function splitIntoFour<T>(values: readonly T[]): T[][] {
  const base = Math.floor(values.length / 4);
  const remainder = values.length % 4;
  const chunks: T[][] = [];
  let start = 0;
  for (let index = 0; index < 4; index += 1) {
    const size = base + (index < remainder ? 1 : 0);
    chunks.push(values.slice(start, start + size));
    start += size;
  }
  return chunks;
}

function orderedBlockFeature(numbers: readonly number[]): number[] {
  const pieces: number[] = [];
  for (const chunk of splitIntoFour(numbers)) {
    const bins = Array<number>(16).fill(0.5);
    for (const value of chunk) {
      const index = Math.min(15, Math.floor(((value - 1) / 355) * 16));
      bins[index] += 1;
    }
    const total = bins.reduce((sum, value) => sum + value, 0);
    pieces.push(...bins.map((value) => Math.sqrt(value / total)));
  }
  const lastDigits = Array<number>(10).fill(0.5);
  numbers.forEach((value) => { lastDigits[value % 10] += 1; });
  const lastTotal = lastDigits.reduce((sum, value) => sum + value, 0);
  pieces.push(...lastDigits.map((value) => Math.sqrt(value / lastTotal)));
  return pieces;
}

function robustScoreCounts(counts: readonly number[], bank: Bank): number[] {
  const artifact = bank.robust.hellinger;
  const feature = hellingerFeature(counts);
  let projected = feature.map((value, index) => (value - artifact.feature_mean[index]) / artifact.feature_scale[index]);
  projected = subtractBasis(projected, artifact.nuisance_basis);
  projected = normalized(projected);
  const scores = artifact.centroids.map((centroid) => dot(projected, centroid));
  return standardize(scores);
}

function orderedBlockScores(numbers: readonly number[], bank: Bank): number[] {
  const artifact = bank.robust.ordered_blocks!;
  const feature = orderedBlockFeature(numbers);
  const standardizedFeature = feature.map((value, index) => (value - artifact.feature_mean[index]) / artifact.feature_scale[index]);
  const unit = normalized(standardizedFeature);
  // Environment interference: each candidate's best match over the enrolled environments.
  const environmentScores = artifact.environment_centroids.map((centroids) => centroids.map((centroid) => dot(unit, centroid)));
  const template = standardize(artifact.centroids.map((_, modelIndex) => Math.max(...environmentScores.map((scores) => scores[modelIndex]))));
  // The same feature with the shared nuisance directions projected out.
  const projected = normalized(subtractBasis(standardizedFeature, artifact.nuisance_basis));
  const nuisance = standardize(artifact.centroids.map((centroid) => dot(projected, centroid)));
  return standardize(template.map((value, index) => 0.5 * value + 0.5 * nuisance[index]));
}

/** One answer's standardized score per candidate, in `bank.models` order. */
export function robustScoreNumbers(numbers: readonly number[], bank: Bank): number[] {
  const marginal = robustScoreCounts(countNumbers(numbers), bank);
  const artifact = bank.robust.ordered_blocks;
  const weight = artifact ? Number(artifact.weight || 0) : 0;
  if (!artifact || weight === 0) return marginal;
  const ordered = orderedBlockScores(numbers, bank);
  return marginal.map((value, index) => (1 - weight) * value + weight * ordered[index]);
}

function softmax(values: readonly number[]): number[] {
  const maximum = Math.max(...values);
  const weights = values.map((value) => Math.exp(value - maximum));
  const total = weights.reduce((sum, value) => sum + value, 0);
  return weights.map((value) => value / total);
}

/** 1 − Jensen–Shannon distance between the pooled answers and a candidate's enrolled counts. */
function jsSimilarity(left: readonly number[], right: readonly number[]): number {
  const leftTotal = left.reduce((sum, value) => sum + value, 0);
  const rightTotal = right.reduce((sum, value) => sum + value, 0) + ALPHA * DIMENSION;
  const p = left.map((value) => value / leftTotal);
  const q = right.map((value) => (value + ALPHA) / rightTotal);
  const midpoint = p.map((value, index) => (value + q[index]) / 2);
  const divergence = (values: number[]) => values.reduce((total, value, index) => total + (value ? value * Math.log(value / midpoint[index]) : 0), 0);
  const js = (divergence(p) + divergence(q)) / 2;
  return 1 - Math.sqrt(js / Math.log(2));
}

/** One answer to score: its full text and the count the challenge asked for. */
export interface Output {
  text: string;
  expectedCount: number;
}

export interface Diagnostic {
  index: number;
  parsedNumbers: number;
  minimumNumbers: number;
  accepted: boolean;
}

export interface CandidateResult {
  model: string;
  displayName: string;
  /** Global attribution probability over the whole bank (one softmax over every candidate). */
  probability: number;
  /** Distribution similarity of the pooled answers to the candidate's enrolled numbers; not a probability. */
  profileSimilarity: number;
  /** Mean standardized score over the valid answers (before calibration). */
  score: number;
  family: string;
  familyName: string;
  /** probability ÷ the family's total: the share within its family only, not a global probability. */
  conditionalProbability: number;
}

export interface FamilyResult {
  family: string;
  displayName: string;
  /** Sum of the global probabilities of the family's candidates. */
  probability: number;
}

export interface Analysis {
  /** Candidates, most probable first. */
  results: CandidateResult[];
  usedOutputs: number;
  diagnostics: Diagnostic[];
  /** The calibration entry used ("1" | "2" | "3", by valid answers) and its temperature. */
  calibration: { queries: string; beta: number };
  families: FamilyResult[];
}

/**
 * Scores each valid answer against every candidate on its own, averages the scores, and
 * turns the average into probabilities with one softmax at the calibrated temperature for
 * the number of valid answers. Throws `noValidOutputs` when no answer is long enough.
 */
export function analyzeOutputs(outputs: readonly Output[], bank: Bank): Analysis {
  const modelIds = bank.models.map((model) => model.id);
  const valid: { counts: number[]; scores: number[] }[] = [];
  const diagnostics: Diagnostic[] = [];
  outputs.forEach((item, index) => {
    const expected = Number(item.expectedCount || 0);
    const numbers = parseNumbers(item.text || "");
    const minimum = minimumNumbers(expected);
    const accepted = numbers.length >= minimum;
    diagnostics.push({ index, parsedNumbers: numbers.length, minimumNumbers: minimum, accepted });
    if (accepted) valid.push({ counts: countNumbers(numbers), scores: robustScoreNumbers(numbers, bank) });
  });
  if (!valid.length) throw new AttributionError("noValidOutputs");

  const combinedScores = modelIds.map((_, modelIndex) => mean(valid.map((item) => item.scores[modelIndex])));
  const calibrationKey = String(Math.min(valid.length, 3)) as "1" | "2" | "3";
  const beta = Number(bank.calibration[calibrationKey].beta);
  const probabilities = softmax(combinedScores.map((value) => beta * value));
  const pooledCounts = Array.from({ length: DIMENSION }, (_, index) => valid.reduce((sum, item) => sum + item.counts[index], 0));
  const familyOf = (model: Bank["models"][number]) => model.family || "models";
  const familyOrder = [...new Set(bank.models.map(familyOf))];
  const familyNames = Object.fromEntries(familyOrder.map((family) => [family, bank.models.find((model) => familyOf(model) === family)?.family_name || family]));
  const results = bank.models.map((model, index) => ({
    model: model.id,
    displayName: model.display_name,
    probability: probabilities[index],
    profileSimilarity: jsSimilarity(pooledCounts, model.counts),
    score: combinedScores[index],
    family: familyOf(model),
    familyName: familyNames[familyOf(model)],
    conditionalProbability: 0,
  })).sort((left, right) => right.probability - left.probability);
  const familyProbabilities = Object.fromEntries(familyOrder.map((family) => [
    family,
    results.filter((item) => item.family === family).reduce((sum, item) => sum + item.probability, 0),
  ]));
  results.forEach((item) => { item.conditionalProbability = item.probability / familyProbabilities[item.family]; });
  return {
    results,
    usedOutputs: valid.length,
    diagnostics,
    calibration: { queries: calibrationKey, beta },
    families: familyOrder.map((family) => ({ family, displayName: familyNames[family], probability: familyProbabilities[family] })),
  };
}

/** A candidate's own global probability and 1-based rank (not the winner's), or null when absent. */
export function candidateStanding(a: Analysis, model: string): { result: CandidateResult; rank: number } | null {
  const index = a.results.findIndex((r) => r.model === model);
  return index < 0 ? null : { result: a.results[index], rank: index + 1 };
}
