// Writes the updater's latest.json for a tag from the .sig files in a directory (downloaded from
// the draft release) and the CHANGELOG.md section. The release workflow runs it once after all
// build jobs: letting each job merge its own entry into latest.json races when two finish together.
// Usage: node scripts/updater-json.mjs v0.2.0 <sig dir> <owner/repo>
import { execFileSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

const [tag, dir, repo] = process.argv.slice(2);
if (!tag || !dir || !repo) {
  console.error("usage: updater-json.mjs <tag> <sig dir> <owner/repo>");
  process.exit(1);
}

// Update package (signature file minus ".sig") → the updater's platform keys. The plain
// "<os>-<arch>" key is what older updater versions look up.
const PLATFORMS = [
  [/_x64-setup\.exe$/, ["windows-x86_64", "windows-x86_64-nsis"]],
  [/_aarch64\.app\.tar\.gz$/, ["darwin-aarch64", "darwin-aarch64-app"]],
  [/_x64\.app\.tar\.gz$/, ["darwin-x86_64", "darwin-x86_64-app"]],
];

const platforms = {};
const files = readdirSync(dir).filter((f) => f.endsWith(".sig"));
for (const [pattern, keys] of PLATFORMS) {
  const sig = files.find((f) => pattern.test(f.slice(0, -4)));
  if (!sig) {
    console.error(`No signature matching ${pattern} among: ${files.join(", ") || "(none)"}`);
    process.exit(1);
  }
  const entry = {
    signature: readFileSync(join(dir, sig), "utf8").trim(),
    // releases/latest keeps working after the draft is published, like tauri-action's own links.
    url: `https://github.com/${repo}/releases/latest/download/${sig.slice(0, -4)}`,
  };
  for (const key of keys) platforms[key] = entry;
}

const notes = execFileSync(process.execPath, ["scripts/release-notes.mjs", tag], { encoding: "utf8" }).trim();
const latest = { version: tag.replace(/^v/, ""), notes, pub_date: new Date().toISOString(), platforms };
console.log(JSON.stringify(latest, null, 2));
