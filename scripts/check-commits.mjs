// Checks commit messages against the format in AGENTS.md ("Commit messages"). CI runs it on
// pull requests for the commits base..head and for the PR title (from $PR_TITLE), which
// becomes the subject when the pull request is squash-merged.
//
//   node scripts/check-commits.mjs <base> <head>
//
// Merge commits are skipped.
import { execFileSync } from "node:child_process";
import { pathToFileURL } from "node:url";

export const TYPES = ["feat", "fix", "perf", "refactor", "test", "docs", "build", "ci", "chore", "revert"];
export const MAX_SUBJECT = 72;

const KEBAB = /^[a-z0-9]+(-[a-z0-9]+)*$/;
const AI_TRAILER = /^(co-authored-by:.*\b(claude|anthropic|copilot|openai|codex|chatgpt|cursor|gemini|devin)\b|.*generated (with|by) .*\b(claude|codex|copilot|cursor|chatgpt)\b)/im;

/** Problems with a subject line; empty when it follows the format. */
export function subjectProblems(subject) {
  const problems = [];
  if ([...subject].length > MAX_SUBJECT) problems.push(`longer than ${MAX_SUBJECT} characters`);
  const m = /^([^(:\s]+)\(([^)]*)\): (.*)$/.exec(subject);
  if (!m) {
    problems.push("not in the form `type(scope): description`");
    return problems;
  }
  const [, type, scope, description] = m;
  if (!TYPES.includes(type)) problems.push(`type \`${type}\` is not one of ${TYPES.join(", ")}`);
  if (!KEBAB.test(scope)) problems.push(`scope \`${scope}\` is not lower-case kebab-case`);
  if (!description.trim()) problems.push("description is empty");
  else {
    if (/^\s/.test(description)) problems.push("extra space before the description");
    if (/^[A-Z]/.test(description)) problems.push("description starts with an upper-case letter");
    if (/\.$/.test(description)) problems.push("description ends with a period");
  }
  return problems;
}

/** Problems with a whole commit message (subject, blank line, body). */
export function messageProblems(message) {
  const lines = message.replace(/\r\n/g, "\n").trimEnd().split("\n");
  const problems = subjectProblems(lines[0] ?? "");
  if (lines.length > 1 && lines[1].trim() !== "") problems.push("no blank line between subject and body");
  if (AI_TRAILER.test(lines.slice(1).join("\n"))) problems.push("body has AI attribution (Co-Authored-By / Generated with)");
  return problems;
}

function commits(base, head) {
  const out = execFileSync("git", ["log", "--no-merges", "--format=%H%x00%B%x1e", `${base}..${head}`], {
    encoding: "utf8",
    maxBuffer: 64 << 20,
  });
  return out
    .split("\x1e")
    .map((c) => c.replace(/^\s+/, ""))
    .filter(Boolean)
    .map((c) => {
      const [sha, message] = c.split("\0");
      return { sha, message };
    });
}

function main() {
  const [base, head] = process.argv.slice(2);
  if (!base || !head) {
    console.error("usage: node scripts/check-commits.mjs <base> <head>");
    process.exit(2);
  }
  let failed = false;
  const report = (what, problems) => {
    if (!problems.length) return;
    failed = true;
    console.error(`${what}\n${problems.map((p) => `  - ${p}`).join("\n")}`);
  };
  for (const { sha, message } of commits(base, head)) {
    report(`${sha.slice(0, 7)} ${message.split(/\r?\n/)[0]}`, messageProblems(message));
  }
  const title = process.env.PR_TITLE;
  if (title !== undefined) report(`Pull request title: ${title}`, subjectProblems(title));
  if (failed) {
    console.error(
      "\nSubjects are `type(scope): description`, e.g. `fix(gateway): keep tool call ids`." +
        "\nSee CONTRIBUTING.md. Reword commits with `git rebase -i` and force-push your branch.",
    );
    process.exit(1);
  }
  console.log("Commit messages OK");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) main();
