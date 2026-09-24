// Copies package.json's version into the Tauri/Cargo manifests. Runs as npm's `version`
// hook, so `npm version 0.2.0` bumps every file, commits and tags v0.2.0 in one go.
import { readFileSync, writeFileSync } from "node:fs";

const version = JSON.parse(readFileSync("package.json", "utf8")).version;

function edit(file, re, to) {
  const text = readFileSync(file, "utf8");
  if (!re.test(text)) throw new Error(`${file}: version not found`);
  writeFileSync(file, text.replace(re, to));
}

edit("src-tauri/tauri.conf.json", /("version":\s*")[^"]+(")/, `$1${version}$2`);
edit("src-tauri/Cargo.toml", /^(version\s*=\s*")[^"]+(")/m, `$1${version}$2`);
edit("src-tauri/Cargo.lock", /(\[\[package\]\]\r?\nname = "agentplus"\r?\nversion = ")[^"]+(")/, `$1${version}$2`);
console.log(`version ${version}`);
