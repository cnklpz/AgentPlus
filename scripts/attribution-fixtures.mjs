// Regenerates src/attribution/fixtures/reference.json: fixed model answers scored by the
// ORIGINAL ModelTrace JavaScript core, so the TypeScript port is checked against the
// reference implementation rather than against itself.
//
//   node scripts/attribution-fixtures.mjs <path to a ModelTrace checkout>
//
// The checkout must be at the pinned commit (see src/attribution/NOTICE.md).
import { execFileSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const COMMIT = "df3a0f9d3e054c0dc02d6d586686db8daf8fa7c8";
const root = process.argv[2];
if (!root) {
  console.error("usage: node scripts/attribution-fixtures.mjs <ModelTrace checkout>");
  process.exit(1);
}
const head = execFileSync("git", ["-C", root, "rev-parse", "HEAD"], { encoding: "utf8" }).trim();
if (head !== COMMIT) {
  console.error(`ModelTrace checkout is at ${head}, expected ${COMMIT}`);
  process.exit(1);
}
const { analyzeGlobalOutputs, parseNumbers } = await import(pathToFileURL(join(root, "static", "fingerprint-core.js")).href);
const { generateChallenges } = await import(pathToFileURL(join(root, "static", "challenge-browser.js")).href);
// The blob as committed (the working tree may have CRLF line ends).
const bank = JSON.parse(execFileSync("git", ["-C", root, "show", `${COMMIT}:data/unified_bank.json`], { encoding: "utf8", maxBuffer: 64 << 20 }));

// Deterministic PRNG (mulberry32), so the fixture never changes between runs.
function rng(seed) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

/** `n` values drawn from a candidate's enrolled number distribution. */
function draw(modelId, n, seed) {
  const counts = bank.models.find((m) => m.id === modelId).counts;
  const total = counts.reduce((s, c) => s + c, 0);
  const next = rng(seed);
  const out = [];
  for (let i = 0; i < n; i += 1) {
    let pick = next() * total;
    let v = 0;
    while (pick >= counts[v]) pick -= counts[v++];
    out.push(v + 1);
  }
  return out;
}

const commas = (xs) => xs.join(", ");
const lines = (xs) => xs.join("\n");
const out = (text, expected) => ({ text, expected_count: expected });

const PARSE = [
  "",
  "no numbers here",
  "1, 2, 3",
  "0, 356, 1, 355, 1000, 42",
  "好的，以下是 310 个整数：\n12, 45, 300",
  "12 45 67 then 8 9",
  "```\n5,6,7\n8\n```",
  "7-8-9 / 10;11",
  "3.14, 2.71",
  "first 1 2 second 3 4 5 third 6",
  "１２, 12, 13",
  "a1b2c3 10 20 30",
  "x 99999999999999999999 5 6",
  "value: 17\tvalue: 18",
  "12，34、56 和 78",
];

const opus = draw("claude-opus-4-7", 329, 1);
const CASES = [
  {
    name: "three answers, one candidate",
    outputs: [
      out(commas(draw("claude-opus-4-7", 300, 11)), 300),
      out(`好的，以下是 317 个整数：\n${commas(draw("claude-opus-4-7", 317, 12))}`, 317),
      out("```\n" + lines(draw("claude-opus-4-7", 329, 13)) + "\n```", 329),
    ],
  },
  { name: "two answers", outputs: [out(commas(draw("gpt-5.5", 305, 21)), 305), out(draw("gpt-5.5", 312, 22).join(" "), 312)] },
  { name: "one answer", outputs: [out(commas(draw("claude-haiku-4-5-20251001", 296, 31)), 296)] },
  {
    name: "one of three too short",
    outputs: [
      out(commas(draw("gpt-6-sol", 310, 41)), 310),
      out(commas(draw("gpt-6-sol", 120, 42)), 320),
      out(commas(draw("gpt-6-sol", 301, 43)), 301),
    ],
  },
  {
    name: "four answers use the three-query calibration",
    outputs: [301, 302, 303, 304].map((n, i) => out(commas(draw("claude-sonnet-5", n, 51 + i)), n)),
  },
  {
    name: "answers from different candidates",
    outputs: [
      out(commas(draw("gpt-5.4", 300, 61)), 300),
      out(commas(draw("claude-opus-5", 300, 62)), 300),
      out(commas(draw("claude-opus-5", 300, 63)), 300),
    ],
  },
  {
    name: "words split a run; the longest run counts",
    outputs: [out(`${commas(opus.slice(0, 100))} and then ${commas(opus.slice(100))}`, 329)],
  },
  { name: "no expected count: minimum is 80", outputs: [out(commas(draw("gpt-5.6-luna", 90, 71)), 0)] },
  { name: "nothing usable", outputs: [out("I can't help with that.", 300), out(commas(draw("gpt-5.5", 150, 81)), 300)] },
];

function run(outputs) {
  try {
    return analyzeGlobalOutputs(outputs, bank);
  } catch (e) {
    return { error: String(e.message) };
  }
}

/** Challenges from the reference generator under a seeded stand-in for `crypto`. */
function challenges(seed, count) {
  const next = rng(seed);
  let n = 0;
  const stub = {
    getRandomValues: (buffer) => { buffer[0] = Math.floor(next() * 0x100000000); return buffer; },
    randomUUID: () => `uuid-${seed}-${n++}`,
  };
  const real = Object.getOwnPropertyDescriptor(globalThis, "crypto");
  Object.defineProperty(globalThis, "crypto", { value: stub, configurable: true });
  try {
    return { seed, count, challenges: generateChallenges(count) };
  } finally {
    Object.defineProperty(globalThis, "crypto", real);
  }
}

const fixture = {
  source: { repository: "https://github.com/xqy2006/ModelTrace", commit: COMMIT, generator: "scripts/attribution-fixtures.mjs" },
  parse: PARSE.map((text) => ({ text, numbers: parseNumbers(text) })),
  cases: CASES.map((c) => ({ ...c, result: run(c.outputs) })),
  challenges: [challenges(101, 1), challenges(202, 2), challenges(303, 3)],
};
const target = new URL("../src/attribution/fixtures/reference.json", import.meta.url);
writeFileSync(target, JSON.stringify(fixture) + "\n");
console.log(`wrote ${target.pathname} (${readFileSync(target).length} bytes)`);
