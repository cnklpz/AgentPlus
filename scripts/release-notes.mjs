// Prints the CHANGELOG.md section for a version (`## 0.2.0` up to the next `## `).
// The release workflow uses it as the GitHub release text, which also ends up in
// latest.json and is shown in AgentPlus's update prompt.
import { readFileSync } from "node:fs";

const version = (process.argv[2] ?? "").replace(/^v/, "");
const lines = readFileSync("CHANGELOG.md", "utf8").split(/\r?\n/);
const heading = new RegExp(`^##\\s+\\[?v?${version.replace(/\./g, "\\.")}\\]?(\\s|$)`);
const start = lines.findIndex((l) => heading.test(l));
if (!version || start < 0) {
  console.error(`CHANGELOG.md has no "## ${version}" section`);
  process.exit(1);
}
const end = lines.findIndex((l, i) => i > start && /^##\s/.test(l));
console.log(lines.slice(start + 1, end < 0 ? undefined : end).join("\n").trim());
