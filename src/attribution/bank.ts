// The fingerprint bank shipped with the app (ModelTrace data/unified_bank.json at a pinned
// commit, see NOTICE.md): loading, strict validation, and which model ids it can attribute.
import { AttributionError, DIMENSION } from "./core";

/** Where the bundled bank comes from. `sha256` is of the upstream blob (LF line ends). */
export const BANK_SOURCE = {
  repository: "https://github.com/xqy2006/ModelTrace",
  commit: "df3a0f9d3e054c0dc02d6d586686db8daf8fa7c8",
  path: "data/unified_bank.json",
  sha256: "1c2cb74d372f9f0f30d0dabbb7b7a838660d2f769a88d0c8489e4c662e088c21",
} as const;

/** Length of the ordered-block feature: 4 blocks × 16 value bins + 10 final digits. */
export const ORDERED_DIMENSION = 4 * 16 + 10;

export interface BankModel {
  id: string;
  display_name: string;
  family?: string;
  family_name?: string;
  counts: number[];
}

interface Projection {
  feature_mean: number[];
  feature_scale: number[];
  nuisance_basis: number[][];
  centroids: number[][];
}

export interface Bank {
  models: BankModel[];
  robust: {
    hellinger: Projection;
    ordered_blocks?: Projection & { weight: number; environment_centroids: number[][][] };
  };
  calibration: Record<"1" | "2" | "3", { beta: number }>;
}

type Json = Record<string, unknown>;

const bad = (path: string): never => { throw new AttributionError("bankInvalid", path); };
const obj = (v: unknown, path: string): Json => (v && typeof v === "object" && !Array.isArray(v) ? (v as Json) : bad(path));
const finite = (v: unknown, path: string): number => (typeof v === "number" && Number.isFinite(v) ? v : bad(path));
const text = (v: unknown, path: string): string => (typeof v === "string" && v.length > 0 ? v : bad(path));

function vector(v: unknown, length: number, path: string): number[] {
  if (!Array.isArray(v) || v.length !== length) bad(path);
  (v as unknown[]).forEach((x, i) => finite(x, `${path}[${i}]`));
  return v as number[];
}

function matrix(v: unknown, rows: number | null, length: number, path: string): number[][] {
  if (!Array.isArray(v) || (rows !== null && v.length !== rows)) bad(path);
  return (v as unknown[]).map((row, i) => vector(row, length, `${path}[${i}]`));
}

function projection(v: unknown, models: number, length: number, path: string): Projection {
  const p = obj(v, path);
  vector(p.feature_mean, length, `${path}.feature_mean`);
  const scale = vector(p.feature_scale, length, `${path}.feature_scale`);
  // Every feature is divided by its scale.
  scale.forEach((x, i) => { if (x === 0) bad(`${path}.feature_scale[${i}]`); });
  matrix(p.nuisance_basis, null, length, `${path}.nuisance_basis`);
  matrix(p.centroids, models, length, `${path}.centroids`);
  return p as unknown as Projection;
}

/**
 * Checks everything scoring reads: candidates with 355 counts each, the projections sized for
 * them, and a finite positive temperature for 1, 2 and 3 answers. Anything missing or
 * malformed throws `bankInvalid` with the path; nothing is filled in with a default.
 */
export function validateBank(raw: unknown): Bank {
  const root = obj(raw, "(root)");
  if (!Array.isArray(root.models) || root.models.length < 2) bad("models");
  const models = root.models as unknown[];
  const ids = new Set<string>();
  models.forEach((m, i) => {
    const model = obj(m, `models[${i}]`);
    const id = text(model.id, `models[${i}].id`);
    if (ids.has(id)) bad(`models[${i}].id`);
    ids.add(id);
    text(model.display_name, `models[${i}].display_name`);
    vector(model.counts, DIMENSION, `models[${i}].counts`);
  });
  const robust = obj(root.robust, "robust");
  // Centroid rows follow `models`; a bank whose model_order disagrees would score the wrong rows.
  if (robust.model_order !== undefined) {
    const order = robust.model_order;
    if (!Array.isArray(order) || order.length !== models.length || order.some((id, i) => id !== (models[i] as Json).id)) bad("robust.model_order");
  }
  projection(robust.hellinger, models.length, DIMENSION, "robust.hellinger");
  if (robust.ordered_blocks !== undefined && robust.ordered_blocks !== null) {
    const ordered = obj(robust.ordered_blocks, "robust.ordered_blocks");
    projection(ordered, models.length, ORDERED_DIMENSION, "robust.ordered_blocks");
    const weight = finite(ordered.weight, "robust.ordered_blocks.weight");
    if (weight < 0 || weight > 1) bad("robust.ordered_blocks.weight");
    if (!Array.isArray(ordered.environment_centroids) || !ordered.environment_centroids.length) bad("robust.ordered_blocks.environment_centroids");
    (ordered.environment_centroids as unknown[]).forEach((env, i) => matrix(env, models.length, ORDERED_DIMENSION, `robust.ordered_blocks.environment_centroids[${i}]`));
  }
  const calibration = obj(root.calibration, "calibration");
  for (const key of ["1", "2", "3"]) {
    const beta = finite(obj(calibration[key], `calibration.${key}`).beta, `calibration.${key}.beta`);
    if (beta <= 0) bad(`calibration.${key}.beta`);
  }
  return root as unknown as Bank;
}

/** Parses and validates the bank file's text. */
export function parseBank(raw: string): Bank {
  let json: unknown;
  try {
    json = JSON.parse(raw);
  } catch {
    throw new AttributionError("bankInvalid", "JSON");
  }
  return validateBank(json);
}

let loading: Promise<Bank> | null = null;

/** The bundled bank, loaded (a separate chunk) and validated once. */
export function loadBank(): Promise<Bank> {
  loading ??= import("./unified_bank.json?raw")
    .catch(() => { throw new AttributionError("bankMissing"); })
    .then((m) => parseBank(m.default))
    .catch((e: unknown) => {
      loading = null;
      throw e;
    });
  return loading;
}

// ---------------------------------------------------------------- which models are supported

/**
 * Ids that name a candidate under another name, each with its source. Only these; a model
 * is never matched by a loose rule such as "contains gpt" or a dropped version suffix.
 */
export const ALIASES: Readonly<Record<string, string>> = {
  // Anthropic's alias for the dated snapshot ("Claude Haiku 4.5", Anthropic models overview).
  "claude-haiku-4-5": "claude-haiku-4-5-20251001",
};

/** Routers (OpenRouter, OpenCode) put the vendor before its own id: `openai/gpt-5.5`. The rest
 *  must then be a candidate of that vendor's family, exactly or through `ALIASES`. */
export const VENDOR_PREFIXES: Readonly<Record<string, string>> = { "openai/": "gpt", "anthropic/": "claude" };

export interface Match {
  /** The bank candidate the model is compared as. The request always keeps the model's own id. */
  candidate: string;
  via: "exact" | "alias" | "prefix";
}

/** The candidate a model id is attributed as, or null when the bank doesn't cover it. */
export function matchCandidate(modelId: string, models: readonly Pick<BankModel, "id" | "family">[]): Match | null {
  const byId = new Map(models.map((m) => [m.id, m]));
  if (byId.has(modelId)) return { candidate: modelId, via: "exact" };
  const alias = ALIASES[modelId];
  if (alias && byId.has(alias)) return { candidate: alias, via: "alias" };
  for (const [prefix, family] of Object.entries(VENDOR_PREFIXES)) {
    if (!modelId.startsWith(prefix)) continue;
    const rest = modelId.slice(prefix.length);
    const id = byId.has(rest) ? rest : ALIASES[rest];
    const m = id ? byId.get(id) : undefined;
    if (m && m.family === family) return { candidate: m.id, via: "prefix" };
  }
  return null;
}
